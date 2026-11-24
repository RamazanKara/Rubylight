//! Client-side decoder, renderer, and listener recorders plus shared loopback test helpers.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nightjar_core::control::{ptype, ControlEvent};
use nightjar_core::roaming::RoamEvent;
use nightjar_core::sdp::{StreamConfig, SCM_H264};
use nightjar_core::session::{
    AudioRenderer, AudioSetup, DecodeStatus, HostInfo, Interrupt, Listener, Location, Session,
    SessionConfig, Stage, VideoDecoder, VideoSetup,
};
use nightjar_core::video::{Frame, FrameType};
use rubylight_wire::mic as host_mic;

use crate::host::FakeHost;
use crate::media::{KEY, PACKET_SIZE};

#[derive(Default)]
pub(crate) struct Record {
    pub(crate) setup: Option<VideoSetup>,
    pub(crate) frames: Vec<(u32, FrameType)>,
    pub(crate) audio_setup: Option<AudioSetup>,
    pub(crate) audio: Vec<Option<Vec<u8>>>,
    pub(crate) stages: Vec<Stage>,
    pub(crate) started: bool,
    pub(crate) terminated: Option<i32>,
    pub(crate) events: Vec<ControlEvent>,
    pub(crate) lifecycle: Vec<&'static str>,
    /// What the listener and the renderers saw, in order, up to the first frame.
    pub(crate) startup: Vec<String>,
    /// The DESCRIBE's packed modes as the listener got them, and whether the decoder was set up.
    pub(crate) packed_modes: Option<(u8, bool)>,
    pub(crate) logs: Vec<String>,
}

pub(crate) struct Recorder {
    pub(crate) record: Arc<Mutex<Record>>,
    pub(crate) need_idr_at: Option<u32>,
}

impl VideoDecoder for Recorder {
    fn setup(&mut self, setup: &VideoSetup) -> Result<(), i32> {
        self.record.lock().unwrap().setup = Some(*setup);
        Ok(())
    }

    fn start(&mut self) {
        self.record
            .lock()
            .unwrap()
            .startup
            .push("video start".into());
    }

    fn submit(&mut self, frame: &Frame) -> DecodeStatus {
        self.record
            .lock()
            .unwrap()
            .frames
            .push((frame.number, frame.frame_type));
        if Some(frame.number) == self.need_idr_at {
            DecodeStatus::NeedIdr
        } else {
            DecodeStatus::Ok
        }
    }

    fn stop(&mut self) {
        self.record.lock().unwrap().lifecycle.push("video stop");
    }

    fn cleanup(&mut self) {
        self.record.lock().unwrap().lifecycle.push("video cleanup");
    }
}

impl AudioRenderer for Recorder {
    fn init(&mut self, setup: &AudioSetup) -> Result<(), i32> {
        self.record.lock().unwrap().audio_setup = Some(*setup);
        Ok(())
    }

    fn play(&mut self, packet: Option<&[u8]>) {
        self.record
            .lock()
            .unwrap()
            .audio
            .push(packet.map(<[u8]>::to_vec));
    }

    fn cleanup(&mut self) {
        self.record.lock().unwrap().lifecycle.push("audio cleanup");
    }
}

impl Listener for Recorder {
    fn stage_complete(&self, stage: Stage) {
        self.record.lock().unwrap().stages.push(stage);
    }

    fn host_control_messages(&self, messages: &[u16]) {
        self.record
            .lock()
            .unwrap()
            .startup
            .push(format!("control messages {messages:x?}"));
    }

    fn host_packed444_modes(&self, modes: u8) {
        let mut r = self.record.lock().unwrap();
        r.packed_modes = Some((modes, r.setup.is_some()));
    }

    fn log(&self, message: &str) {
        self.record.lock().unwrap().logs.push(message.into());
    }

    fn connection_started(&self) {
        self.record.lock().unwrap().started = true;
    }

    fn connection_terminated(&self, error: i32) {
        self.record.lock().unwrap().terminated = Some(error);
    }

    fn control_event(&self, event: ControlEvent) {
        self.record.lock().unwrap().events.push(event);
    }
}

pub(crate) fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Waits for a microphone packet from `ip` and returns the Opus packet inside.
pub(crate) fn await_mic_from(socket: &UdpSocket, ip: IpAddr) -> Vec<u8> {
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut buffer = [0u8; 2048];
    loop {
        let (n, from) = socket.recv_from(&mut buffer).expect("a microphone packet");
        if from.ip() == ip {
            let packet = host_mic::parse(&buffer[..n]).expect("a microphone packet");
            return host_mic::open(&KEY, 0, &packet).unwrap();
        }
    }
}

/// A 720p60 stream with the microphone, in 1024-byte packets as on loopback.
pub(crate) fn roaming_config() -> SessionConfig {
    SessionConfig {
        stream: StreamConfig {
            width: 1280,
            height: 720,
            fps: 60,
            packet_size: PACKET_SIZE as i32,
            remote_input_aes_key: KEY,
            microphone: true,
            ..Default::default()
        },
        location: Location::Auto,
        ..Default::default()
    }
}

pub(crate) fn start_session(
    host: &FakeHost,
    config: SessionConfig,
    record: &Arc<Mutex<Record>>,
) -> Session {
    let recorder = || {
        Box::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        })
    };
    Session::start(
        &HostInfo {
            address: "127.0.0.1".into(),
            app_version: "7.1.431.-1".into(),
            rtsp_session_url: Some(format!("rtsp://127.0.0.1:{}", host.rtsp_port)),
            server_codec_mode_support: SCM_H264,
        },
        config,
        recorder(),
        recorder(),
        Arc::new(Recorder {
            record: record.clone(),
            need_idr_at: None,
        }),
        &Interrupt::default(),
    )
    .unwrap()
}

pub(crate) fn roam_events(record: &Mutex<Record>) -> Vec<RoamEvent> {
    record
        .lock()
        .unwrap()
        .events
        .iter()
        .filter_map(|e| match e {
            ControlEvent::Roam(event) => Some(*event),
            _ => None,
        })
        .collect()
}

pub(crate) fn count(messages: &Mutex<Vec<u16>>, kind: u16) -> usize {
    messages
        .lock()
        .unwrap()
        .iter()
        .filter(|&&t| t == kind)
        .count()
}

/// Two local addresses to roam between: 127.0.0.1 and 127.0.0.2.
pub(crate) fn loopback_addresses() -> Option<(IpAddr, IpAddr)> {
    let (old, new) = (
        IpAddr::from(Ipv4Addr::LOCALHOST),
        IpAddr::from(Ipv4Addr::new(127, 0, 0, 2)),
    );
    if UdpSocket::bind((new, 0)).is_err() {
        // Windows answers on all of 127.0.0.0/8; elsewhere it may take an alias.
        if cfg!(windows) {
            panic!("{new} cannot be bound");
        }
        eprintln!("skipped: {new} cannot be bound here");
        return None;
    }
    Some((old, new))
}

/// Silences the host's control stream until the client holds input back, past
/// `INPUT_CONTACT_LOST_AFTER` (which needs a network grace period), when a silence starts
/// probing with roaming on; then until the client hears the host again, which by then has
/// read whatever was sent to it meanwhile.
pub(crate) fn silence_host(host: &FakeHost, session: &Session) {
    host.silent.store(true, Ordering::SeqCst);
    until("the client to notice the silence", || {
        session.stats().input_contact_lost
    });
    host.silent.store(false, Ordering::SeqCst);
    until("the client to hear the host again", || {
        !session.stats().input_contact_lost
    });
}

/// Input after the roaming steps reaches the host: the control stream is up.
pub(crate) fn input_arrives(session: &Session, host: &FakeHost) {
    let inputs = count(&host.control_messages, ptype::INPUT_DATA);
    session
        .input(|q| q.send_keyboard(0x41, true, 0, 0))
        .unwrap();
    until("input", || {
        count(&host.control_messages, ptype::INPUT_DATA) > inputs
    });
}
