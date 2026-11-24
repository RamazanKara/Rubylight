//! Batched and coalesced UDP receive (`BatchReceiver`), segment splitting and control-message
//! parsing across Linux/Android (`recvmmsg` + `UDP_GRO`) and Windows (`WSARecvMsg` coalescing).

use std::io;
use std::net::UdpSocket;

#[cfg(any(target_os = "linux", target_os = "android"))]
use super::mmsg::{self, gro};
#[cfg(windows)]
use super::uro;
use super::{failure, Failure};

/// Datagrams one [`BatchReceiver::receive`] takes at most.
pub const RECEIVE_BATCH: usize = 32;
/// Large enough that no datagram is truncated: Windows fails a truncating receive. It holds a
/// coalesced read too, which is at most `uro::MAX_COALESCED` bytes.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const WHOLE_DATAGRAM: usize = 65536;

/// Receives every datagram already queued on a media socket with one system call, so a high
/// packet rate does not pay a call per datagram: `recvmmsg` with `MSG_WAITFORONE` on Linux and
/// Android, which waits (up to the socket's poll timeout) for the first datagram only and
/// returns at once with whatever else is queued. On Windows, UDP receive coalescing
/// (`UDP_RECV_MAX_COALESCED_SIZE`): the stack hands over consecutive datagrams of one sender
/// in one buffer, which `WSARecvMsg` reads with their size (`UDP_COALESCED_INFO`), and the
/// receiver splits it again. Where the kernel refuses `recvmmsg` or the system refuses
/// coalescing (older Windows versions), one datagram per receive as `receive`; where the
/// adapter does not coalesce, each read is one datagram all the same. (Windows' Registered I/O takes batches too, but its sockets
/// refuse the plain sends and the clone the video socket needs.)
pub struct BatchReceiver {
    /// Bytes per datagram slot of `buffer`.
    slot: usize,
    buffer: Vec<u8>,
    /// The length of each datagram of the last receive on the wire, which may exceed `slot`.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    lengths: [usize; RECEIVE_BATCH],
    /// Where the last receive's first datagram starts in `buffer`, and the distance from each
    /// datagram to the next: the slot, or a coalesced read's datagram size.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    base: usize,
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    stride: usize,
    /// Linux and Android: the datagrams of the last receive, each a span of `buffer` (see
    /// [`push_segments`]). A message of UDP_GRO segments is several of them.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    spans: Vec<Span>,
    /// Linux and Android: the most bytes kept of each datagram, the reader's maximum with
    /// UDP_GRO and the slot otherwise.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    cut: usize,
    /// Linux and Android: the arrival of each datagram of the last receive on the clock of
    /// [`now_us`](super::super::now_us), as the kernel stamped it; None without a stamp.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    arrivals: Vec<Option<u64>>,
    /// Linux and Android: whether UDP_GRO is on for the socket.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    gro: bool,
    /// Linux and Android: the socket descriptor UDP_GRO was set on.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    gro_fd: Option<std::os::fd::RawFd>,
    /// Linux and Android: why UDP_GRO was refused, until the caller takes it.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    gro_refused: Option<io::Error>,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    batch: Option<mmsg::Batch>,
    #[cfg(windows)]
    pub(super) coalesce: Option<uro::Coalesce>,
    /// The datagrams of the last coalesced read that the receives so far have not taken.
    #[cfg(windows)]
    pending: Segments,
    /// Windows: the reader's own maximum. A coalesced read that its size leaves with a
    /// datagram longer than this came from an adapter that lost the size (see
    /// [`BatchReceiver::receive_coalesced`]).
    #[cfg(windows)]
    max_datagram: usize,
    /// Why batching stopped, until the caller takes it.
    fallback: Option<io::Error>,
}

impl BatchReceiver {
    /// A receiver for `socket` and the sockets that replace it. With `recvmmsg` batches, a
    /// datagram longer than `max_datagram` (the reader's own maximum) is cut to it; its length
    /// still counts in full. Otherwise, coalesced or not, datagrams are read whole.
    /// `coalescing` is the setting for the stack's coalescing (UDP_GRO on Linux and Android,
    /// receive coalescing on Windows); off, Linux and Android still take `recvmmsg` batches.
    pub fn new(socket: &UdpSocket, max_datagram: usize, coalescing: bool) -> Self {
        Self::with_batches(socket, max_datagram, true, coalescing)
    }

    /// One datagram per receive, as where the kernel refuses batches.
    #[cfg(test)]
    pub fn unbatched(socket: &UdpSocket, max_datagram: usize) -> Self {
        Self::with_batches(socket, max_datagram, false, false)
    }

    fn with_batches(
        socket: &UdpSocket,
        max_datagram: usize,
        batches: bool,
        coalescing: bool,
    ) -> Self {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            let (probed, fallback) = match mmsg::probe(socket) {
                Ok(()) => (true, None),
                Err(e) => (false, Some(e)),
            };
            // A coalesced message is split only by batches, so UDP_GRO is asked for with them.
            let (coalescing, gro_fd, refused) = if probed && batches && coalescing {
                match gro::set(socket, true) {
                    Ok(()) => (true, Some(std::os::fd::AsRawFd::as_raw_fd(socket)), None),
                    Err(e) => (false, None, Some(e)),
                }
            } else {
                (false, None, None)
            };
            let slot = if coalescing {
                gro::SLOT
            } else {
                max_datagram.max(1).next_multiple_of(64)
            };
            let mut buffer = vec![0u8; slot * RECEIVE_BATCH];
            // Points into `buffer`'s allocation, which is never resized and outlives it here.
            let batch = mmsg::Batch::new(&mut buffer, slot);
            Self {
                slot,
                buffer,
                spans: Vec::with_capacity(RECEIVE_BATCH),
                arrivals: Vec::with_capacity(RECEIVE_BATCH),
                cut: if coalescing {
                    max_datagram.max(1)
                } else {
                    slot
                },
                gro: coalescing,
                gro_fd,
                gro_refused: refused,
                batch: (probed && batches).then_some(batch),
                fallback,
            }
        }
        #[cfg(windows)]
        {
            // Batches on Windows are coalesced reads, so they come only with the setting.
            let (coalesce, fallback) = if batches && coalescing {
                match uro::Coalesce::new(socket, max_datagram) {
                    Ok(coalesce) => (Some(coalesce), None),
                    Err(e) => (None, Some(e)),
                }
            } else {
                (None, None)
            };
            Self {
                slot: WHOLE_DATAGRAM,
                buffer: vec![0u8; WHOLE_DATAGRAM],
                lengths: [0; RECEIVE_BATCH],
                base: 0,
                stride: WHOLE_DATAGRAM,
                coalesce,
                pending: Segments::default(),
                max_datagram: max_datagram.max(1),
                fallback,
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
        {
            let _ = (socket, max_datagram, batches, coalescing);
            Self {
                slot: WHOLE_DATAGRAM,
                buffer: vec![0u8; WHOLE_DATAGRAM],
                lengths: [0; RECEIVE_BATCH],
                base: 0,
                stride: WHOLE_DATAGRAM,
                fallback: None,
            }
        }
    }

    /// Whether receives take batches: `recvmmsg`, or coalesced reads on Windows.
    pub fn batched(&self) -> bool {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return self.batch.is_some();
        #[cfg(windows)]
        return self.coalesce.as_ref().is_some_and(uro::Coalesce::on);
        #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
        false
    }

    /// Why receives stopped taking batches, once: the kernel refused `recvmmsg`, or the system
    /// refused coalescing on the socket or the roam's new one.
    pub fn take_fallback(&mut self) -> Option<io::Error> {
        self.fallback.take()
    }

    /// Waits up to the socket's poll timeout for a datagram and takes it with every other one
    /// already queued, up to [`RECEIVE_BATCH`]: their count, 0 on the timeout. ICMP
    /// port-unreachable reports are skipped as in `receive`.
    pub fn receive(&mut self, socket: &UdpSocket) -> io::Result<usize> {
        #[cfg(windows)]
        if (self.coalesce.is_some() || self.pending.left() > 0)
            && let Some(count) = self.receive_coalesced(socket)?
        {
            return Ok(count);
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            self.base = 0;
            self.stride = self.slot;
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        while let Some(batch) = self.batch.as_mut() {
            if self.gro {
                let fd = std::os::fd::AsRawFd::as_raw_fd(socket);
                if self.gro_fd != Some(fd) {
                    match gro::set(socket, true) {
                        Ok(()) => self.gro_fd = Some(fd),
                        Err(e) => {
                            self.gro = false;
                            self.gro_fd = None;
                            self.gro_refused = Some(e);
                        }
                    }
                }
            }
            match batch.receive(socket) {
                Ok(count) => {
                    self.spans.clear();
                    self.arrivals.clear();
                    // One reading of both clocks for the batch: each datagram's stamp is
                    // taken back from it by its age on the kernel's clock.
                    let clocks =
                        mmsg::Batch::realtime_now_ns().map(|real| (real, super::super::now_us()));
                    let mut stamped = 0;
                    for i in 0..count {
                        let segment = batch.gro(i);
                        let total = batch.length(i);
                        let arrival = clocks
                            .zip(batch.stamp_ns(i))
                            .map(|((real, local), ns)| arrival_us(ns, real, local));
                        stamped += usize::from(arrival.is_some());
                        let before = self.spans.len();
                        push_segments(
                            &mut self.spans,
                            i * self.slot,
                            self.slot,
                            self.cut,
                            total,
                            segment,
                        );
                        self.arrivals
                            .resize(self.arrivals.len() + self.spans.len() - before, arrival);
                    }
                    if stamped == 0 && count > 0 {
                        batch.forget_stamps();
                    }
                    return Ok(self.spans.len());
                }
                Err(e) if mmsg::unsupported(&e) => {
                    self.batch = None;
                    self.fallback = Some(e);
                    if self.gro {
                        // One-datagram reads cannot split a coalesced read, so UDP_GRO goes off.
                        let _ = gro::set(socket, false);
                        self.gro = false;
                        self.gro_fd = None;
                        self.cut = self.slot;
                    }
                }
                Err(e) => match failure(e) {
                    Failure::Timeout => return Ok(0),
                    Failure::Retry => {}
                    Failure::Fatal(e) => return Err(e),
                },
            }
        }
        loop {
            match self.receive_one(socket).map_err(failure) {
                Ok(length) => {
                    #[cfg(any(target_os = "linux", target_os = "android"))]
                    {
                        self.spans.clear();
                        push_segments(&mut self.spans, 0, self.slot, self.cut, length, None);
                        self.arrivals.clear();
                        self.arrivals.resize(self.spans.len(), None);
                        return Ok(self.spans.len());
                    }
                    #[cfg(not(any(target_os = "linux", target_os = "android")))]
                    {
                        self.lengths[0] = length;
                        return Ok(1);
                    }
                }
                Err(Failure::Timeout) => return Ok(0),
                Err(Failure::Retry) => {}
                Err(Failure::Fatal(e)) => return Err(e),
            }
        }
    }

    /// One datagram into the first slot: its length on the wire.
    fn receive_one(&mut self, socket: &UdpSocket) -> io::Result<usize> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return mmsg::receive_one(socket, &mut self.buffer[..self.slot]);
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        socket.recv(&mut self.buffer[..self.slot])
    }

    /// The next datagrams of a coalesced read: those the last receive left, or a new read's.
    /// None when the roam's new socket refused coalescing, which leaves the receive to the
    /// one-datagram path from now on.
    ///
    /// A read that its size (`UDP_COALESCED_INFO`) leaves with a datagram longer than the
    /// reader's maximum held several datagrams whose size the stack did not report: some
    /// network adapters' receive segment coalescing (RSC) loses it (quinn-rs/quinn#2041).
    /// Split by guesswork, a read could come apart wrongly, so it is passed on as before (the
    /// video receiver keeps its first datagram's worth), and coalescing goes off on the socket
    /// for the rest of the stream; the reads already queued are still read with their size.
    #[cfg(windows)]
    fn receive_coalesced(&mut self, socket: &UdpSocket) -> io::Result<Option<usize>> {
        if self.pending.left() == 0 {
            let Some(coalesce) = self.coalesce.as_mut() else {
                return Ok(None);
            };
            if let Err(e) = coalesce.follow(socket) {
                self.coalesce = None;
                self.fallback = Some(e);
                return Ok(None);
            }
            loop {
                match coalesce.receive(socket, &mut self.buffer).map_err(failure) {
                    Ok((total, segment)) => {
                        self.pending = Segments::new(total, segment);
                        if coalesce.on() && self.pending.segment() > self.max_datagram {
                            self.fallback =
                                Some(unsized_read(coalesce.turn_off(), total, segment, self.max_datagram));
                        }
                        break;
                    }
                    Err(Failure::Timeout) => return Ok(Some(0)),
                    Err(Failure::Retry) => {}
                    Err(Failure::Fatal(e)) => return Err(e),
                }
            }
        }
        let (base, count) = self.pending.take(&mut self.lengths);
        self.base = base;
        self.stride = self.pending.segment();
        Ok(Some(count))
    }

    /// Datagram `index` of the last receive, cut to the slot.
    pub fn datagram(&self, index: usize) -> &[u8] {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return {
            let span = self.spans[index];
            &self.buffer[span.start..span.start + span.kept]
        };
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            let start = self.base + index * self.stride;
            &self.buffer[start..start + self.lengths[index].min(self.stride)]
        }
    }

    /// When datagram `index` of the last receive arrived, in microseconds on the clock of
    /// [`now_us`](super::super::now_us), as the kernel stamped it: Linux and Android with batches.
    /// None where the receive has no stamp (Windows, a kernel without it, one-datagram
    /// reads); the caller then stamps the batch.
    pub fn arrival_us(&self, index: usize) -> Option<u64> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return self.arrivals.get(index).copied().flatten();
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            let _ = index;
            None
        }
    }

    /// The length on the wire of datagram `index` of the last receive.
    pub fn length(&self, index: usize) -> usize {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return self.spans[index].wire;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        self.lengths[index]
    }

    /// Whether receives take UDP_GRO coalesced reads: Linux and Android, with batches.
    pub fn gro(&self) -> bool {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return self.gro;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        false
    }

    /// Why UDP_GRO was refused for the socket, once: receives then take one datagram per read
    /// as before.
    pub fn take_gro_refusal(&mut self) -> Option<io::Error> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return self.gro_refused.take();
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        None
    }
}

/// The datagrams of one coalesced read still to take: `left` datagrams of `segment` bytes back
/// to back from `next`, the last one of what remains up to `end` when the read did not divide
/// evenly.
#[cfg(any(windows, target_os = "linux", target_os = "android", test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Segments {
    next: usize,
    end: usize,
    segment: usize,
    left: usize,
}

#[cfg(any(windows, target_os = "linux", target_os = "android", test))]
impl Segments {
    /// A read of `total` bytes from the start of the buffer, with the datagram size its
    /// `UDP_COALESCED_INFO` gave (None without one). Without one, or with a size that does not
    /// split the read (0, or the whole read or more), the read is one datagram: what Windows
    /// hands over when it did not coalesce.
    pub(super) fn new(total: usize, segment: Option<usize>) -> Self {
        let (segment, left) = match segment {
            Some(segment) if segment > 0 && segment < total => (segment, total.div_ceil(segment)),
            _ => (total, 1),
        };
        Self {
            next: 0,
            end: total,
            segment,
            left,
        }
    }

    pub(super) fn left(&self) -> usize {
        self.left
    }

    // Windows' coalesced reads and the tests use it; Linux reads the GRO size per datagram.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(super) fn segment(&self) -> usize {
        self.segment
    }

    /// Takes up to `lengths.len()` datagrams, writing their lengths: where the first starts,
    /// and how many.
    pub(super) fn take(&mut self, lengths: &mut [usize]) -> (usize, usize) {
        let base = self.next;
        let count = self.left.min(lengths.len());
        for length in &mut lengths[..count] {
            *length = self.segment.min(self.end - self.next);
            self.next += *length;
        }
        self.left -= count;
        (base, count)
    }
}

/// The data of the control message at `level` and `kind` among the control messages in
/// `control`: a `cmsghdr` (`WSACMSGHDR` on Windows, the same layout) of `cmsg_len`, `cmsg_level`
/// and `cmsg_type`, then the data at the next natural alignment, then padding to the header
/// alignment. The data runs to the message's `cmsg_len`. None without one, or when the messages
/// run past the buffer.
#[cfg(any(windows, target_os = "linux", target_os = "android", test))]
fn cmsg_data(control: &[u8], level: i32, kind: i32) -> Option<&[u8]> {
    const WORD: usize = std::mem::size_of::<usize>();
    // The header, and the data's alignment after it.
    const HEADER: usize = WORD + 8;
    const DATA: usize = HEADER.next_multiple_of(WORD);
    let word = |at: usize| usize::from_ne_bytes(control[at..at + WORD].try_into().unwrap());
    let int = |at: usize| i32::from_ne_bytes(control[at..at + 4].try_into().unwrap());
    let mut at = 0;
    while at + HEADER <= control.len() {
        let length = word(at);
        if length < HEADER || length > control.len() - at {
            return None;
        }
        if int(at + WORD) == level && int(at + WORD + 4) == kind && length >= DATA + 4 {
            return Some(&control[at + DATA..at + length]);
        }
        // The next header, at the header alignment.
        at += length.next_multiple_of(WORD);
    }
    None
}

/// The first int of the control message at `level` and `kind` (see [`cmsg_data`]). None without
/// one, or with one shorter than an int.
#[cfg(any(windows, target_os = "linux", target_os = "android", test))]
pub(super) fn cmsg_int(control: &[u8], level: i32, kind: i32) -> Option<usize> {
    let value = cmsg_data(control, level, kind)?.get(..4)?;
    Some(u32::from_ne_bytes(value.try_into().unwrap()) as usize)
}

/// `SOL_SOCKET` and `SO_TIMESTAMPNS` (which also names its control message, `SCM_TIMESTAMPNS`),
/// as Linux and Android define them on every architecture Nightjar builds for.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) const SOL_SOCKET: i32 = 1;
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) const SO_TIMESTAMPNS: i32 = 35;

/// The kernel's receive time of a datagram, in nanoseconds on `CLOCK_REALTIME`, from its
/// `SCM_TIMESTAMPNS` control message (a `timespec`). None without one, or before the epoch.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn cmsg_timestamp_ns(control: &[u8]) -> Option<u64> {
    const SEC: usize = std::mem::size_of::<std::os::raw::c_long>();
    let data = cmsg_data(control, SOL_SOCKET, SO_TIMESTAMPNS)?;
    // `timespec`: seconds, then nanoseconds, each the architecture's own word.
    let word = |at: usize| -> Option<i64> {
        let bytes = data.get(at..at + SEC)?;
        Some(if SEC == 8 {
            i64::from_ne_bytes(bytes.try_into().unwrap())
        } else {
            i64::from(i32::from_ne_bytes(bytes.try_into().unwrap()))
        })
    };
    let (sec, nsec) = (word(0)?, word(SEC)?);
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return None;
    }
    Some(sec as u64 * 1_000_000_000 + nsec as u64)
}

/// The arrival of a datagram on the clock of [`super::super::now_us`]: its kernel stamp
/// (`stamp_ns`, `CLOCK_REALTIME`) taken back from `local_now_us` by how long ago it was
/// against `realtime_now_ns`, both read together once for the batch. A stamp that reads later
/// than that moment (the wall clock stepped back) counts as now; one before the clock's origin
/// as 0.
#[cfg(any(target_os = "linux", target_os = "android", test))]
pub(super) fn arrival_us(stamp_ns: u64, realtime_now_ns: u64, local_now_us: u64) -> u64 {
    let age_us = realtime_now_ns.saturating_sub(stamp_ns) / 1000;
    local_now_us.saturating_sub(age_us)
}

/// The datagram size of a coalesced read: the `UDP_COALESCED_INFO` message among the `WSARecvMsg`
/// control messages in `control` (see [`cmsg_int`]).
#[cfg(any(windows, test))]
pub(super) fn coalesced_segment(control: &[u8]) -> Option<usize> {
    const IPPROTO_UDP: i32 = 17;
    const UDP_COALESCED_INFO: i32 = 3;
    cmsg_int(control, IPPROTO_UDP, UDP_COALESCED_INFO)
}

/// One datagram of a Linux or Android receive: where it starts in the buffer, its length on the
/// wire, and the bytes of it kept there.
#[cfg(any(target_os = "linux", target_os = "android", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Span {
    pub(super) start: usize,
    pub(super) wire: usize,
    pub(super) kept: usize,
}

/// Appends the datagrams of one received message to `spans`. The message starts at `slot_start`
/// in a slot of `slot` bytes, is `total` bytes on the wire, and UDP_GRO coalesced it into
/// `segment`-byte datagrams (the last may be shorter) when that is Some; otherwise it is one
/// datagram. Each keeps up to `cut` bytes, and no bytes past the slot.
#[cfg(any(target_os = "linux", target_os = "android", test))]
pub(super) fn push_segments(
    spans: &mut Vec<Span>,
    slot_start: usize,
    slot: usize,
    cut: usize,
    total: usize,
    segment: Option<usize>,
) {
    let slot_end = slot_start + slot;
    let mut segments = Segments::new(total, segment);
    let mut lengths = [0; 64];
    while segments.left() > 0 {
        let (base, count) = segments.take(&mut lengths);
        let mut start = slot_start + base;
        for &wire in &lengths[..count] {
            let kept = wire.min(cut).min(slot_end.saturating_sub(start));
            spans.push(Span { start, wire, kept });
            start += wire;
        }
    }
}

/// Why receive coalescing went off for the stream: a read of `total` bytes whose size
/// (`segment`, from `UDP_COALESCED_INFO`) left a datagram longer than `max_datagram`, and
/// whether turning it off worked (`turned_off`).
#[cfg(any(windows, test))]
pub(super) fn unsized_read(
    turned_off: io::Result<()>,
    total: usize,
    segment: Option<usize>,
    max_datagram: usize,
) -> io::Error {
    let size = match segment {
        Some(segment) => format!("a datagram size of {segment}"),
        None => "no datagram size".to_owned(),
    };
    let read = format!(
        "a {total}-byte read of datagrams of at most {max_datagram} bytes came with {size}: \
         this network adapter loses the size of coalesced reads"
    );
    match turned_off {
        Ok(()) => io::Error::other(format!("receive coalescing is off for this stream, {read}")),
        Err(e) => io::Error::new(
            e.kind(),
            format!("turning receive coalescing off failed ({e}), {read}"),
        ),
    }
}
