//! Differential tests: identical packet sequences go through Nightjar's patched
//! moonlight-common-c (built by `build.rs`) and through `nightjar-core`, and the outputs must
//! match byte for byte. Each side writes the same event log: frames with their buffers,
//! control-stream signals, decode queue flushes and audio samples.
//!
//! The test hosts follow the Rubylight host's packetizers (`rust/core/src/packet.rs`).

pub mod host;
pub mod net;

#[cfg(difftest_c)]
pub mod c;
#[cfg(difftest_c)]
pub mod rust;

/// The C core keeps global state, so the harnesses run one at a time.
#[cfg(difftest_c)]
pub static C_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One input to both implementations.
#[derive(Clone, Debug)]
pub enum Step {
    /// A datagram and its receive time.
    Packet(Vec<u8>, u64),
    /// The decoder completes every queued frame: `true` decoded, `false` needs a key frame.
    Complete(bool),
    /// The host's HDR mode changes.
    Hdr(bool),
    /// The audio socket timed out once.
    Timeout,
}

/// Feeds `steps` to the C and Rust video receivers and asserts identical logs after every
/// step, identical statistics at the end, and returns the number of frames delivered.
#[cfg(difftest_c)]
pub fn run_video(
    config: &nightjar_core::video::VideoConfig,
    queue_bound: usize,
    steps: &[Step],
) -> usize {
    let _guard = C_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut c = c::CVideo::new(config, queue_bound);
    let mut r = rust::RustVideo::new(config.clone(), queue_bound);
    let (mut c_all, mut r_all) = (Vec::new(), Vec::new());
    for (index, step) in steps.iter().enumerate() {
        match step {
            Step::Packet(data, now) => {
                c.receive(data, *now);
                r.receive(data, *now);
            }
            Step::Complete(ok) => {
                c.complete_all(*ok);
                r.complete_all(*ok);
            }
            Step::Hdr(active) => {
                c.set_hdr(*active);
                r.set_hdr(*active);
            }
            Step::Timeout => {}
        }
        let (c_log, r_log) = (c::take_log(), r.take_log());
        if c_log != r_log {
            panic!(
                "video logs differ at step {index} of {}\nC:    {:#?}\nRust: {:#?}",
                steps.len(),
                describe_log(&c_log),
                describe_log(&r_log)
            );
        }
        c_all.extend(c_log);
        r_all.extend(r_log);
        assert_eq!(
            c.current_frame(),
            r.current_frame(),
            "current frame at step {index}"
        );
    }
    assert_eq!(c.stats(), r.stats(), "RTP video stats");
    let events = describe_log(&c_all);
    let count = |prefix: &str| events.iter().filter(|e| e.starts_with(prefix)).count();
    eprintln!(
        "{} steps: {} frames, {} IDR requests, {} invalidations, {} FEC reports, {} flushes, {:?} packets",
        steps.len(),
        count("frame "),
        count("request IDR"),
        count("invalidate"),
        count("fec status"),
        count("flush"),
        c.stats(),
    );
    count("frame ")
}

/// As [`run_video`] for audio; returns the number of samples played (including concealed).
#[cfg(difftest_c)]
pub fn run_audio(config: &nightjar_core::audio::AudioConfig, steps: &[Step]) -> usize {
    let _guard = C_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut c = c::CAudio::new(config);
    let mut r = rust::RustAudio::new(config.clone());
    let (mut played, mut lost) = (0, 0);
    for (index, step) in steps.iter().enumerate() {
        match step {
            Step::Packet(data, now) => {
                c.receive(data, *now);
                r.receive(data, *now);
            }
            Step::Timeout => {
                c.timeout();
                r.timeout();
            }
            _ => {}
        }
        let (c_log, r_log) = (c::take_log(), r.take_log());
        if c_log != r_log {
            panic!(
                "audio logs differ at step {index} of {}\nC:    {:#?}\nRust: {:#?}",
                steps.len(),
                describe_log(&c_log),
                describe_log(&r_log)
            );
        }
        played += describe_log(&c_log).len();
        lost += describe_log(&c_log)
            .iter()
            .filter(|e| e.starts_with("audio 0 "))
            .count();
        assert_eq!(c.stats(), r.stats(), "RTP audio stats at step {index}");
    }
    eprintln!(
        "{} steps: {played} played, {lost} concealed, stats {:?}",
        steps.len(),
        c.stats()
    );
    played
}

/// Readable form of an event log, for failure messages.
pub fn describe_log(mut log: &[u8]) -> Vec<String> {
    fn take<'a>(log: &mut &'a [u8], n: usize) -> &'a [u8] {
        let n = n.min(log.len());
        let (head, tail) = log.split_at(n);
        *log = tail;
        head
    }
    fn u32le(log: &mut &[u8]) -> u32 {
        let b = take(log, 4);
        let mut a = [0; 4];
        a[..b.len()].copy_from_slice(b);
        u32::from_le_bytes(a)
    }
    fn u64le(log: &mut &[u8]) -> u64 {
        let b = take(log, 8);
        let mut a = [0; 8];
        a[..b.len()].copy_from_slice(b);
        u64::from_le_bytes(a)
    }
    let mut out = Vec::new();
    while !log.is_empty() {
        let tag = take(&mut log, 1)[0];
        out.push(match tag {
            b'F' => {
                let number = u32le(&mut log);
                let kind = take(&mut log, 1)[0];
                let latency = u16::from_le_bytes(take(&mut log, 2).try_into().unwrap_or([0; 2]));
                let receive = u64le(&mut log);
                let enqueue = u64le(&mut log);
                let pts = u64le(&mut log);
                let rtp = u32le(&mut log);
                let flags = take(&mut log, 2).to_vec();
                let count = u32le(&mut log);
                let mut buffers = Vec::new();
                let mut sum = 0u32;
                for _ in 0..count {
                    let kind = take(&mut log, 1)[0];
                    let len = u32le(&mut log);
                    let bytes = take(&mut log, len as usize);
                    sum = bytes.iter().fold(sum, |s, &b| s.wrapping_mul(31).wrapping_add(b.into()));
                    buffers.push(format!("{kind}:{len}"));
                }
                format!(
                    "frame {number} type {kind} lat {latency} recv {receive} enq {enqueue} pts {pts} \
                     rtp {rtp} hdr/cs {flags:?} buffers [{}] sum {sum:08x}",
                    buffers.join(" ")
                )
            }
            b'I' => "request IDR".into(),
            b'R' => format!("invalidate {}..={}", u32le(&mut log), u32le(&mut log)),
            b'S' => format!("fec status {:02x?}", take(&mut log, 21)),
            b'C' => format!("received {} ltr {}", u32le(&mut log), take(&mut log, 1)[0]),
            b'W' => format!("saw {}", u32le(&mut log)),
            b'X' => "flush".into(),
            b'A' => {
                let len = u32le(&mut log);
                format!("audio {len} {:02x?}", take(&mut log, len as usize))
            }
            other => format!("unknown tag {other}"),
        });
    }
    out
}
