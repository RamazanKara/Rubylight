//! Separate packetization and paced sending, retaining the newest intra frame.
mod transport;
use crate::state::{Session, Shared};
use anyhow::{Result, bail};
use butterpollo_core::{
    config::Config,
    packet::{PacketBlock, PyrowaveFec, VideoPacketizer},
    performance::{Claim, Claims},
    pyrowave::DetailFec,
    session::Warnings,
};
use butterpollo_windows::{encoder::Encoded, timing::Signal};
use std::{
    net::{SocketAddr, UdpSocket},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

struct Frame {
    encoded: Encoded,
    peer: SocketAddr,
    claimed: Instant,
    processing_started: Instant,
    latency: Duration,
    /// The encoder's frame came back from its poll.
    polled: Instant,
    /// A new picture's claim, for the stage timings.
    claim: Option<Claim>,
}
struct Prepared {
    blocks: Vec<PacketBlock>,
    peer: SocketAddr,
    claimed: Instant,
    processing: u64,
    latency: u64,
    age: u64,
    wire_frame: u32,
    demand: u64,
    kbps: u32,
    /// For the stage timings and the timing record: a new picture's claim, the poll that
    /// returned its frame, packetization's start and end, and the RTP timestamp.
    claim: Option<Claim>,
    polled: Instant,
    packetized: Instant,
    prepared: Instant,
    timestamp: u32,
}
#[derive(Default)]
struct DroppedFrames {
    unreported: u64,
    reported: Option<Instant>,
}
impl DroppedFrames {
    fn prepared<T>(
        &mut self,
        result: Result<T>,
        bytes: usize,
        now: Instant,
        warnings: &Warnings,
    ) -> Option<T> {
        match result {
            Ok(packets) => Some(packets),
            Err(error) => {
                warnings.event("pyrowave_frame", format!("PyroWave frame dropped ({error:#}); the client may hold the previous picture. Lower bitrate or resolution to stay within Moonlight's packet limit, or use HEVC/AV1."), butterpollo_core::session::EVENT_PERIOD);
                self.unreported += 1;
                if self
                    .reported
                    .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(5))
                {
                    tracing::warn!(error = %format!("{error:#}"), bytes, dropped = self.unreported, "PyroWave frame dropped");
                    self.unreported = 0;
                    self.reported = Some(now);
                }
                None
            }
        }
    }
}
/// Wire rate to send a frame of `packets` within one frame period.
fn demand_bps(
    packets: usize,
    packet_size: usize,
    encrypted: bool,
    ipv6: bool,
    fps_millihz: u32,
) -> u64 {
    let packet_bytes = packet_size + 16 + if encrypted { 32 } else { 0 };
    let wire_bytes = packets * (packet_bytes + butterpollo_core::network_pacing::overhead(ipv6));
    (wire_bytes as u64).saturating_mul(u64::from(fps_millihz)) / 1000 * 8
}
/// Calibration and live traffic share the same one-millisecond wire batches.
fn batch_limit(bps: u64, packet: usize, ipv6: bool) -> usize {
    butterpollo_core::network_pacing::pyrowave_batch_limit(bps, packet, ipv6)
}
struct Slot {
    pending: Mutex<Option<Frame>>,
    changed: Condvar,
    ready: Mutex<Option<Prepared>>,
    ready_changed: Condvar,
    error: Mutex<Option<String>>,
    stop: AtomicBool,
    stop_signal: Signal,
}
pub struct Sender {
    slot: Arc<Slot>,
    worker: Option<thread::JoinHandle<()>>,
    transport: Option<thread::JoinHandle<()>>,
    session: Arc<Session>,
}
impl Sender {
    pub fn new(
        socket: Arc<UdpSocket>,
        session: Arc<Session>,
        config: Config,
        host: Shared,
        start: Instant,
        track_presents: bool,
    ) -> Result<Self> {
        let mut sender = Self::packetizer(
            session.clone(),
            config.clone(),
            host.clone(),
            start,
            track_presents,
        )?;
        let shared = sender.slot.clone();
        sender.transport = Some(thread::Builder::new().name("pyrowave-send".into()).spawn(
            move || {
                let result = transport::run(&shared, socket, session, config, host, start);
                if let Err(error) = result {
                    *shared.error.lock().unwrap() = Some(format!("{error:#}"));
                }
                shared.stop.store(true, Ordering::Release);
                let _ = shared.stop_signal.set();
                shared.changed.notify_all();
            },
        )?);
        Ok(sender)
    }

    fn packetizer(
        session: Arc<Session>,
        config: Config,
        host: Shared,
        start: Instant,
        track_presents: bool,
    ) -> Result<Self> {
        let slot = Arc::new(Slot {
            pending: Mutex::new(None),
            changed: Condvar::new(),
            ready: Mutex::new(None),
            ready_changed: Condvar::new(),
            error: Mutex::new(None),
            stop: AtomicBool::new(false),
            stop_signal: Signal::new()?,
        });
        let shared = slot.clone();
        let current = session.clone();
        let worker = thread::Builder::new().name("pyrowave-packetize".into()).spawn(move || {
            let _priority = butterpollo_windows::capture::Priority::new();
            let mut packetizer = VideoPacketizer {
                sequence: 0, iv_counter: 0, frame: 1,
                packet_size: current.config.packet_size, fec_percent: 0,
                min_fec: current.config.min_fec,
                key: (current.config.encryption & 2 != 0).then_some(current.launch.key),
            };
            let mut controller = DetailFec::new(current.config.fps_millihz());
            let mut stamper = track_presents.then(butterpollo_windows::present_timing::Stamper::default);
            let mut last_stamp = start;
            let mut fec_reported: Option<Instant> = None;
            let requested_fec = config.integer("pyrowave_critical_fec_percentage", 20);
            if requested_fec != requested_fec.clamp(0, 255) {
                current.launch.warnings.set("pyrowave_fec_config", format!("PyroWave critical FEC {requested_fec}% is outside 0-255; using {}%. Correct the critical FEC setting.", requested_fec.clamp(0, 255)));
            }
            let mut dropped = DroppedFrames::default();
            'frames: while !shared.stop.load(Ordering::Acquire) && !current.stopping() && !host.stop.load(Ordering::Acquire) {
                let frame = {
                    let mut pending = shared.pending.lock().unwrap();
                    if pending.is_none() {
                        pending = shared.changed.wait_timeout(pending, Duration::from_millis(20)).unwrap().0;
                    }
                    pending.take()
                };
                let Some(Frame { encoded: frame, peer, claimed, processing_started, latency, polled, claim }) = frame else { continue };
                let captured = frame.presentation.unwrap_or(claimed);
                let stamp = stamper.as_mut().map_or(captured, |stamper| stamper.stamp(captured, &current.output.read().unwrap()))
                    .max(last_stamp + Duration::from_nanos(11_112));
                last_stamp = stamp;
                let timestamp = (stamp.saturating_duration_since(start).as_secs_f64() * 90000.) as u64 as u32;
                let critical_percentage = current.fec_percent().min(requested_fec.clamp(0, 255) as usize);
                let bitrate = current.bitrate.load(Ordering::Relaxed);
                let kbps = if current.config.configured_bitrate_kbps > 0 {
                    (u64::from(current.config.configured_bitrate_kbps) * u64::from(bitrate)
                        / u64::from(current.config.bitrate_kbps.max(1))).min(2_000_000) as u32
                } else { bitrate };
                let now = Instant::now();
                let Some((detail_percentage, wire_budget)) = (if current.config.pyrowave_records && critical_percentage > 0 {
                    dropped.prepared(controller.observe(&frame.bytes, now, kbps), frame.bytes.len(), now, &current.launch.warnings)
                } else { Some((0, 0)) }) else { continue };
                let wire_frame = packetizer.frame;
                // Same boundary as the conventional packetizer. Neither the
                // prepared frame's queue nor packet pacing can change it.
                let packetized = Instant::now();
                let processing = micros(packetized.saturating_duration_since(processing_started));
                let prepared = packetizer.pyrowave_blocks(&frame.bytes, timestamp, processing, PyrowaveFec {
                    records: current.config.pyrowave_records, critical_percentage, detail_percentage,
                    wire_budget, ipv6: peer.ip().to_canonical().is_ipv6(),
                });
                let Some((packets, limited, blocks)) = dropped.prepared(prepared, frame.bytes.len(), now, &current.launch.warnings) else { continue };
                if limited && fec_reported.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
                    fec_reported = Some(Instant::now());
                    current.launch.warnings.event("pyrowave_fec_frame", "PyroWave error correction was reduced or omitted because the frame and its parity cannot fit the supported wire blocks. Recovery protection is reduced; lower bitrate/resolution or use HEVC/AV1 on a lossy link.", butterpollo_core::session::EVENT_PERIOD);
                }
                let mut ready = Vec::new();
                for block in blocks {
                    let Some(block) = dropped.prepared(block, frame.bytes.len(), Instant::now(), &current.launch.warnings) else { continue 'frames };
                    ready.push(block);
                }
                let demand = demand_bps(packets, current.config.packet_size, current.config.encryption & 2 != 0, peer.ip().to_canonical().is_ipv6(), current.config.fps_millihz());
                if shared.ready.lock().unwrap().replace(Prepared { blocks: ready, peer, claimed, processing, latency: micros(latency),
                    age: micros(claimed.saturating_duration_since(captured)), wire_frame, demand, kbps,
                    claim, polled, packetized, prepared: Instant::now(), timestamp }).is_some() {
                    current.stats.frames_replaced.fetch_add(1, Ordering::Relaxed);
                }
                shared.ready_changed.notify_one();
            }
            shared.stop.store(true, Ordering::Release);
            let _ = shared.stop_signal.set();
            shared.changed.notify_all();
            shared.ready_changed.notify_all();
        })?;
        Ok(Self {
            slot,
            worker: Some(worker),
            transport: None,
            session,
        })
    }

    pub fn submit(
        &self,
        frames: Vec<Encoded>,
        peer: SocketAddr,
        latency: Duration,
        processing_started: Instant,
        claims: &mut Claims,
    ) -> Result<()> {
        if let Some(error) = self.slot.error.lock().unwrap().as_ref() {
            bail!("video sender stopped: {error}");
        }
        if frames.is_empty() {
            return Ok(());
        }
        let polled = Instant::now();
        let mut pending = self.slot.pending.lock().unwrap();
        for frame in frames {
            let latency = frame.latency.unwrap_or(latency);
            let claimed = polled.checked_sub(latency).unwrap_or(polled);
            let claim = claims.take(frame.presentation);
            if pending
                .replace(Frame {
                    encoded: frame,
                    peer,
                    claimed,
                    latency,
                    processing_started,
                    polled,
                    claim,
                })
                .is_some()
            {
                self.session
                    .stats
                    .frames_replaced
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        self.slot.changed.notify_one();
        Ok(())
    }
}
impl Drop for Sender {
    fn drop(&mut self) {
        self.slot.stop.store(true, Ordering::Release);
        let _ = self.slot.stop_signal.set();
        self.slot.changed.notify_all();
        self.slot.ready_changed.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.transport.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_windows::{net::Batch, timing::Timer};

    #[test]
    fn stopping_interrupts_a_paced_wait_without_sending_another_packet() -> Result<()> {
        let fixture = crate::state::test_support::Fixture::new();
        let launch = fixture.launch(fixture.client(0), butterpollo_core::session::Role::Stream);
        let session = Session::new(
            launch,
            butterpollo_core::rtsp::Negotiated {
                codec: 3,
                bitrate_kbps: 80_000,
                ..Default::default()
            },
        );
        let mut config = fixture.host.config.read().unwrap().clone();
        config
            .values
            .insert("pacing_max_bitrate_kbps".into(), "1".into());
        let client = UdpSocket::bind("127.0.0.1:0")?;
        client.set_read_timeout(Some(Duration::from_secs(2)))?;
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0")?);
        let sender = Sender::new(
            socket,
            session,
            config,
            fixture.host.clone(),
            Instant::now(),
            false,
        )?;
        sender.submit(
            vec![Encoded {
                bytes: vec![42; 3000],
                idr: true,
                after_invalidation: false,
                frame_cap_bytes: None,
                latency: None,
                presentation: None,
            }],
            client.local_addr()?,
            Duration::ZERO,
            Instant::now(),
            &mut Claims::default(),
        )?;
        let mut packet = [0; 2048];
        client.recv_from(&mut packet)?;
        thread::sleep(Duration::from_millis(30));
        let (done, finished) = std::sync::mpsc::channel();
        thread::spawn(move || {
            drop(sender);
            let _ = done.send(());
        });
        assert!(
            finished.recv_timeout(Duration::from_millis(250)).is_ok(),
            "sender teardown remained blocked by packet pacing"
        );
        client.set_read_timeout(Some(Duration::from_millis(100)))?;
        let error = client
            .recv_from(&mut packet)
            .expect_err("a packet was sent after stopping");
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
        Ok(())
    }

    #[test]
    fn a_pyrowave_picture_yields_host_timing_stages_and_a_record_row() {
        let fixture = crate::state::test_support::Fixture::new();
        let launch = fixture.launch(fixture.client(0), butterpollo_core::session::Role::Stream);
        let session = Session::new(
            launch,
            butterpollo_core::rtsp::Negotiated {
                codec: 3,
                bitrate_kbps: 80_000,
                ..Default::default()
            },
        );
        let asked = butterpollo_core::host_timing::HostTimingRequest { enabled: true }.encode();
        assert_eq!(
            session.host_timings.lock().unwrap().on_request(&asked),
            Some(true)
        );
        let (sink, rows) = butterpollo_core::timing_record::channel(8);
        *session.timing_record.write().unwrap() = Some(Arc::new(sink));
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap());
        // The media clock started 500 ms ago; the picture was presented 6 ms ago.
        let now = Instant::now();
        let ago = |ms: u64| now - Duration::from_millis(ms);
        let sender = Sender::new(
            socket,
            session.clone(),
            fixture.host.config.read().unwrap().clone(),
            fixture.host.clone(),
            ago(500),
            false,
        )
        .unwrap();
        let mut claims = Claims::default();
        claims.push(Claim {
            key: ago(6),
            presented: ago(6),
            acquired: ago(5),
            published: ago(4),
            claimed: ago(3),
            submitted: ago(2),
        });
        let picture = Encoded {
            bytes: vec![42; 3000],
            idr: true,
            after_invalidation: false,
            frame_cap_bytes: None,
            latency: Some(Duration::from_millis(3)),
            presentation: Some(ago(6)),
        };
        sender
            .submit(
                vec![picture],
                client.local_addr().unwrap(),
                Duration::ZERO,
                now,
                &mut claims,
            )
            .unwrap();
        let mut report = None;
        for _ in 0..400 {
            report = session.host_timings.lock().unwrap().take_report();
            if report.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let report = report.expect("no 0x5536 record for the PyroWave picture");
        let records =
            butterpollo_core::host_timing::HostTiming::decode_report_with(&report).unwrap();
        assert_eq!(records.len(), 1);
        let (record, extra) = records[0];
        assert_eq!(record.frame, 1);
        assert_eq!(record.queue_us, 4_000, "capture, hold, claim and convert");
        let extra = extra.unwrap();
        assert_eq!(
            (extra.presented_at_us, extra.capture_us, extra.convert_us),
            (494_000, 1_000, 1_000)
        );
        assert!(record.host_us >= 6_000 + record.encode_us);
        // The stage samples exist too, and the record has its row.
        let performance = &session.info()["performance"];
        assert_eq!(performance["stages"]["frames"], 1);
        let row = rows.try_recv().unwrap();
        assert_eq!(row.wire_frame, 1);
        assert!(row.rtp_us.abs_diff(494_000) <= 12, "{}", row.rtp_us);
        assert_eq!(row.points[0], 494_000);
        assert_eq!(row.points[4], 498_000);
        // The report sums nine stages, each cut to whole microseconds.
        assert!(
            (row.points[9] - row.points[0]).abs_diff(i64::from(record.host_us)) <= 9,
            "the row and the report end at the same last packet"
        );
    }

    #[test]
    fn prepared_intra_frames_freeze_timing_and_keep_only_the_newest_waiter() {
        let wait = Duration::from_secs(4);
        for encrypted in [false, true] {
            let fixture = crate::state::test_support::Fixture::new();
            let launch = fixture.launch(fixture.client(0), butterpollo_core::session::Role::Stream);
            let session = Session::new(
                launch,
                butterpollo_core::rtsp::Negotiated {
                    encryption: if encrypted { 3 } else { 1 },
                    bitrate_kbps: 80_000,
                    configured_bitrate_kbps: 100_000,
                    ..Default::default()
                },
            );
            session.bitrate.store(40_000, Ordering::Relaxed);
            let config = fixture.host.config.read().unwrap().clone();
            let sender = Sender::packetizer(
                session.clone(),
                config,
                fixture.host.clone(),
                Instant::now(),
                false,
            )
            .unwrap();
            let snapshot = |wire_frame| {
                let ready = sender.slot.ready.lock().unwrap();
                let (ready, timeout) = sender
                    .slot
                    .ready_changed
                    .wait_timeout_while(ready, wait, |r| {
                        r.as_ref().is_none_or(|f| f.wire_frame != wire_frame)
                    })
                    .unwrap();
                let result = ready.as_ref().map(|f| {
                    (
                        f.processing,
                        f.latency,
                        f.kbps,
                        f.blocks[0][0].clone(),
                        f.blocks.iter().map(Vec::len).sum::<usize>(),
                    )
                });
                drop(ready);
                assert!(
                    !timeout.timed_out(),
                    "packetizer did not prepare frame {wire_frame}"
                );
                result.unwrap()
            };
            let frame = || Encoded {
                bytes: vec![42; 3000],
                idr: true,
                after_invalidation: false,
                frame_cap_bytes: None,
                latency: Some(Duration::from_millis(12)),
                presentation: None,
            };
            let tick = Instant::now() - Duration::from_millis(2);
            sender
                .submit(
                    vec![frame()],
                    "127.0.0.1:12345".parse().unwrap(),
                    Duration::ZERO,
                    tick,
                    &mut Claims::default(),
                )
                .unwrap();
            let (processing, latency, kbps, packet, packets) = snapshot(1);
            assert_eq!(latency, 12_000);
            assert_eq!(kbps, 50_000);
            let plaintext = if encrypted {
                butterpollo_core::crypto::gcm_open(
                    &session.launch.key,
                    &packet[..12],
                    &packet[16..32],
                    &packet[32..],
                )
                .unwrap()
            } else {
                packet.clone()
            };
            assert_eq!(
                u16::from_le_bytes(plaintext[33..35].try_into().unwrap()),
                (processing / 100).min(65535) as u16
            );
            thread::sleep(Duration::from_millis(30));
            assert_eq!(snapshot(1).0, processing);
            assert_eq!(snapshot(1).3, packet);

            // A slow sender consumes the latest independent intra frame;
            // replacing prepared packets must never reuse their encryption IVs.
            sender
                .submit(
                    vec![frame()],
                    "127.0.0.1:12345".parse().unwrap(),
                    Duration::ZERO,
                    Instant::now(),
                    &mut Claims::default(),
                )
                .unwrap();
            let (_, _, _, next, _) = snapshot(2);
            assert_eq!(session.stats.frames_replaced.load(Ordering::Relaxed), 1);
            assert_eq!(session.stats.frames.load(Ordering::Relaxed), 0);
            if encrypted {
                assert_eq!(
                    u64::from_le_bytes(next[..8].try_into().unwrap()),
                    packets as u64
                );
            }
            let (done, finished) = std::sync::mpsc::channel();
            thread::spawn(move || {
                drop(sender);
                done.send(()).unwrap();
            });
            finished.recv_timeout(wait).unwrap();
        }
    }

    #[test]
    fn failed_block_abandons_the_frame_and_the_next_frame_keeps_its_counters() {
        for fail_at in [0, 1] {
            let payload = vec![7; 400_000];
            let mut packetizer = VideoPacketizer {
                sequence: u32::MAX - 10,
                iv_counter: 100,
                frame: u32::MAX,
                packet_size: 1392,
                fec_percent: 0,
                min_fec: 2,
                key: Some([42; 16]),
            };
            let fec = || PyrowaveFec {
                records: false,
                critical_percentage: 20,
                detail_percentage: 0,
                wire_budget: 0,
                ipv6: false,
            };
            let mut dropped = DroppedFrames::default();
            let warnings = Warnings::default();
            let now = Instant::now();
            let mut sent = 0;
            {
                let (_, _, mut blocks) = packetizer
                    .pyrowave_blocks(&payload, 9000, 543, fec())
                    .unwrap();
                let mut prepared = 0;
                let failing = std::iter::from_fn(|| {
                    let result = if prepared == fail_at {
                        Some(Err(anyhow::anyhow!("injected block preparation failure")))
                    } else {
                        blocks.next()
                    };
                    prepared += 1;
                    result
                });
                for block in failing {
                    let Some(packets) = dropped.prepared(block, payload.len(), now, &warnings)
                    else {
                        break;
                    };
                    sent += packets.len();
                }
                assert_eq!(prepared, fail_at + 1);
            }
            assert_eq!(warnings.snapshot()[0].code, "pyrowave_frame");
            let sequence = (u32::MAX - 10).wrapping_add(sent as u32);
            let frame = if fail_at == 0 { u32::MAX } else { 0 };
            assert_eq!(packetizer.sequence, sequence);
            assert_eq!(packetizer.iv_counter, 100 + sent as u64);
            assert_eq!(packetizer.frame, frame);
            let packets = dropped
                .prepared(
                    packetizer.encode_pyrowave(&payload, 9750, 543, fec()),
                    payload.len(),
                    now,
                    &warnings,
                )
                .unwrap();
            assert_eq!(&packets[0][..8], &(100 + sent as u64).to_le_bytes());
            assert_eq!(&packets[0][12..16], &frame.to_le_bytes());
            assert_eq!(
                packetizer.sequence,
                sequence.wrapping_add(packets.len() as u32)
            );
            assert_eq!(packetizer.iv_counter, 100 + (sent + packets.len()) as u64);
            assert_eq!(packetizer.frame, frame.wrapping_add(1));
        }
    }

    #[test]
    fn malformed_frame_after_a_late_interval_is_dropped_by_detail_planning() {
        // Detail planning parses records only after a late frame. That parse
        // used to end the session through `?` before block preparation ran.
        let mut controller = DetailFec::new(60_000);
        let mut dropped = DroppedFrames::default();
        let warnings = Warnings::default();
        let now = Instant::now();
        let malformed = [0; 64];
        assert_eq!(
            dropped.prepared(
                controller.observe(&malformed, now, 100_000),
                64,
                now,
                &warnings
            ),
            Some((0, 0))
        );
        assert!(warnings.snapshot().is_empty());
        let late = now + Duration::from_millis(50);
        assert!(
            dropped
                .prepared(
                    controller.observe(&malformed, late, 100_000),
                    64,
                    late,
                    &warnings
                )
                .is_none()
        );
        assert_eq!(dropped.reported, Some(late));
        assert_eq!(warnings.snapshot()[0].code, "pyrowave_frame");
    }

    #[test]
    fn frame_drop_warnings_report_the_first_failure_and_count_repeats() {
        let mut dropped = DroppedFrames::default();
        let warnings = Warnings::default();
        let now = Instant::now();
        for (millis, unreported) in [(0, 0), (1, 1), (4999, 2), (5000, 0)] {
            warnings.clear("pyrowave_frame");
            assert!(
                dropped
                    .prepared::<()>(
                        Err(anyhow::anyhow!("bad frame")),
                        123,
                        now + Duration::from_millis(millis),
                        &warnings,
                    )
                    .is_none()
            );
            assert_eq!(dropped.unreported, unreported);
            assert_eq!(warnings.snapshot()[0].code, "pyrowave_frame");
            if millis == 0 || millis == 5000 {
                assert_eq!(dropped.reported, Some(now + Duration::from_millis(millis)));
            }
        }
        assert_eq!(
            dropped.prepared(Ok(42), 123, now + Duration::from_secs(6), &warnings),
            Some(42)
        );
        assert_eq!(dropped.reported, Some(now + Duration::from_secs(5)));
    }

    /// An encrypted 1080p60 frame filling the encoder's 400 Mbps budget, as
    /// the sender's blocks, and the rate the sender paces it at.
    fn budget_sized_1080p_frame() -> (Vec<Vec<Vec<u8>>>, u64, Duration) {
        use butterpollo_core::pyrowave;
        let (packet_size, kbps, fps_millihz) = (1360, 400_000, 60_000);
        let period = Duration::from_secs_f64(1000. / f64::from(fps_millihz));
        let budget = pyrowave::budget(kbps, period, packet_size, true, 20);
        let size = |i: usize| 8 + (i * 73 % 504) * 4;
        let (mut count, mut total) = (0, 8);
        while total + size(count) <= budget {
            total += size(count);
            count += 1;
        }
        let mut raw = Vec::with_capacity(total);
        raw.extend_from_slice(&(0x80000000u32 | 1919 | (1079 << 14)).to_le_bytes());
        raw.extend_from_slice(&(count as u32).to_le_bytes());
        for i in 0..count {
            raw.extend_from_slice(&((size(i) as u32 / 4) << 16).to_le_bytes());
            raw.extend_from_slice(&((((i * 37) % count) as u32) << 8).to_le_bytes());
            raw.extend((0..size(i) - 8).map(|j| (i * 31 + j * 17) as u8));
        }
        let framed = pyrowave::record_frame(
            &raw,
            pyrowave::aligned_payload(packet_size),
            pyrowave::max_frame_bytes(packet_size, true),
        )
        .unwrap();
        let mut packetizer = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size,
            fec_percent: 0,
            min_fec: 2,
            key: Some([42; 16]),
        };
        let fec = PyrowaveFec {
            records: true,
            critical_percentage: 20,
            detail_percentage: 0,
            wire_budget: 0,
            ipv6: false,
        };
        let (packets, _, blocks) = packetizer.pyrowave_blocks(&framed, 0, 0, fec).unwrap();
        let blocks = blocks.collect::<Result<Vec<_>>>().unwrap();
        let demand = demand_bps(packets, packet_size, true, false, fps_millihz);
        let bps = butterpollo_core::network_pacing::pyrowave_rate_bps(
            0,
            kbps,
            0,
            demand,
            Default::default(),
        );
        (blocks, bps, period)
    }

    #[test]
    fn paced_budget_sized_1080p_frame_leaves_within_85_percent_of_the_period() {
        let (blocks, bps, period) = budget_sized_1080p_frame();
        // Loopback costs on the RX 7900 XT machine: a 64 KB segmented send
        // takes about 0.2 ms and the precise timer wakes up to 0.1 ms late.
        let (send, wake) = (Duration::from_micros(250), Duration::from_micros(100));
        let start = Instant::now();
        let mut pacer = butterpollo_core::network_pacing::Pacer::new(start);
        let mut now = start;
        let mut batches = 0;
        for block in &blocks {
            let mut remaining = block.as_slice();
            while !remaining.is_empty() {
                if pacer.due() > now {
                    now = pacer.due() + wake;
                }
                let count = Batch::count(remaining, batch_limit(bps, remaining[0].len(), false));
                let bytes = remaining[..count].iter().map(Vec::len).sum();
                now += send;
                pacer.sent(now, bytes, count, false, bps);
                remaining = &remaining[count..];
                batches += 1;
            }
        }
        let spent = now - start;
        assert!(
            spent <= period.mul_f64(0.85),
            "{batches} batches at {bps} bps took {spent:?} of a {period:?} frame"
        );
    }

    /// Timing-dependent: run at normal priority on an otherwise idle machine
    /// with `cargo test -p butterpollo pyrowave_loopback -- --ignored --nocapture`.
    #[test]
    #[ignore = "measures real loopback sends; needs an idle machine"]
    fn pyrowave_loopback_sends_a_budget_sized_1080p_frame_within_85_percent_of_the_period()
    -> Result<()> {
        let (blocks, bps, period) = budget_sized_1080p_frame();
        let receiver = UdpSocket::bind("127.0.0.1:0")?;
        socket2::SockRef::from(&receiver).set_recv_buffer_size(64 << 20)?;
        receiver.set_read_timeout(Some(Duration::from_millis(50)))?;
        let peer = receiver.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let drain = {
            let stop = stop.clone();
            thread::spawn(move || {
                let mut buffer = [0; 2048];
                while !stop.load(Ordering::Relaxed) {
                    let _ = receiver.recv(&mut buffer);
                }
            })
        };
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        socket.set_nonblocking(true)?;
        butterpollo_windows::net::configure_udp(&socket)?;
        let timer = Timer::new()?;
        let mut batch = Batch::default();
        let mut pacer = butterpollo_core::network_pacing::Pacer::new(Instant::now());
        let mut spans = Vec::new();
        for _ in 0..180 {
            let begin = Instant::now();
            for block in &blocks {
                let mut remaining = block.as_slice();
                while !remaining.is_empty() {
                    if pacer.due() > Instant::now() {
                        timer.until_paced(pacer.due());
                    }
                    let count =
                        Batch::count(remaining, batch_limit(bps, remaining[0].len(), false));
                    let bytes = batch.send(&socket, &remaining[..count], peer)?;
                    pacer.sent(Instant::now(), bytes, count, false, bps);
                    remaining = &remaining[count..];
                }
            }
            spans.push(begin.elapsed());
            timer.until_precise(begin + period);
        }
        stop.store(true, Ordering::Relaxed);
        drain.join().unwrap();
        spans.sort();
        let median = spans[spans.len() / 2];
        println!(
            "{bps} bps, frame send median {median:?}, p95 {:?}, max {:?}, period {period:?}, dropped {}",
            spans[spans.len() * 95 / 100],
            spans[spans.len() - 1],
            batch.dropped
        );
        assert!(median <= period.mul_f64(0.85));
        Ok(())
    }
}
