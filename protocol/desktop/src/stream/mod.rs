//! A stream: launching or resuming the app over HTTPS, starting the session on a worker
//! thread, then on the UI thread showing pictures, playing nothing (audio has its own
//! thread), sending keyboard, mouse and controller input, and drawing the overlay.
//!
//! Input handling follows moonlight-qt's SdlInputHandler (GPL-3.0, Cameron Gutman and
//! contributors): positional keys, the Ctrl+Alt+Shift combos, raising every key when focus
//! is lost, relative capture with absolute fallback.

pub mod pipeline;
pub mod present;

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nightjar_core::control::{ClientMessage, ControlEvent};
use nightjar_core::http::{App, ClientIdentity, HostClient, LaunchRequest, ServerInfo};
use nightjar_core::input::packets::{modifier, mouse_button};
use nightjar_core::sdp::*;
use nightjar_core::session::{self, HostInfo, Interrupt, Location, Session, SessionConfig, Stage};
use rubylight_protocol::control::{HostTimingRequest, CLOCK_SYNC_MESSAGE_TYPE, HOST_TIMING_MESSAGE_TYPE};
use rubylight_protocol::present_clock::ClockOffset;
use sdl3_sys::everything::*;

use crate::config::{self, AudioChannels, CodecPreference, HostRecord, MouseMode, Settings};
use crate::gamepad::{Action, Gamepads};
use crate::hosts::Waker;
use crate::keymap;
use crate::media::Hardware;
use pipeline::{lock, Notice, Shared};
use present::{letterbox, PacingMode, Pacer, VideoTexture};

/// What the display offers, for "native" resolution and "display" refresh.
#[derive(Clone, Copy, Debug)]
pub struct Display {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f32,
}

pub struct LaunchPlan {
    pub host_id: u64,
    pub record: HostRecord,
    pub info: ServerInfo,
    pub app: App,
    pub resume: bool,
    pub settings: Settings,
    pub identity: Arc<ClientIdentity>,
    pub display: Display,
    pub hardware: Hardware,
    pub pacing: PacingMode,
}

/// The stream's negotiated shape.
#[derive(Clone, Debug)]
pub struct Facts {
    pub width: i32,
    pub height: i32,
    pub fps: i32,
    pub bitrate_kbps: i32,
    pub format: u32,
    pub host_timing: bool,
    pub clock_sync: bool,
    pub app_name: String,
    pub host_name: String,
}

pub fn codec_name(format: u32) -> &'static str {
    if format & VIDEO_FORMAT_MASK_AV1 != 0 {
        if format & VIDEO_FORMAT_MASK_10BIT != 0 {
            "AV1 10-bit"
        } else {
            "AV1"
        }
    } else if format & VIDEO_FORMAT_MASK_H265 != 0 {
        if format & VIDEO_FORMAT_MASK_10BIT != 0 {
            "HEVC Main10"
        } else {
            "HEVC"
        }
    } else {
        "H.264"
    }
}

fn random_bytes<const N: usize>() -> [u8; N] {
    use std::hash::{BuildHasher, Hasher};
    let mut out = [0u8; N];
    for chunk in out.chunks_mut(8) {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(session::now_us());
        let bytes = h.finish().to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    out
}

/// The stream's resolution, rate and bitrate from the settings and display.
pub fn stream_mode(settings: &Settings, display: Display) -> (u32, u32, u32, u32) {
    let (mut w, mut h) = if settings.width == 0 || settings.height == 0 {
        (display.width, display.height)
    } else {
        (settings.width, settings.height)
    };
    w = (w.max(320) + 1) & !1;
    h = (h.max(240) + 1) & !1;
    let fps = if settings.fps == 0 {
        (display.refresh_hz.round() as u32).clamp(30, 240)
    } else {
        settings.fps
    };
    let bitrate = if settings.bitrate_kbps == 0 {
        config::default_bitrate_kbps(w, h, fps)
    } else {
        settings.bitrate_kbps
    };
    (w, h, fps, bitrate)
}

/// The formats offered for a codec preference, limited to what the host encodes.
pub fn video_formats(preference: CodecPreference, hdr: bool, host_scm: u32) -> u32 {
    let mut formats = match preference {
        CodecPreference::H264 => VIDEO_FORMAT_H264,
        CodecPreference::Hevc => VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265,
        CodecPreference::Av1 => VIDEO_FORMAT_H264 | VIDEO_FORMAT_AV1_MAIN8,
        CodecPreference::Auto => VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265,
    };
    if hdr {
        if formats & VIDEO_FORMAT_H265 != 0 {
            formats |= VIDEO_FORMAT_H265_MAIN10;
        }
        if formats & VIDEO_FORMAT_AV1_MAIN8 != 0 {
            formats |= VIDEO_FORMAT_AV1_MAIN10;
        }
    }
    // Hosts that report codec support get only what they encode; H.264 always stays.
    if host_scm != 0 {
        if host_scm & SCM_HEVC == 0 {
            formats &= !VIDEO_FORMAT_H265;
        }
        if host_scm & SCM_HEVC_MAIN10 == 0 {
            formats &= !VIDEO_FORMAT_H265_MAIN10;
        }
        if host_scm & SCM_AV1_MAIN8 == 0 {
            formats &= !VIDEO_FORMAT_AV1_MAIN8;
        }
        if host_scm & SCM_AV1_MAIN10 == 0 {
            formats &= !VIDEO_FORMAT_AV1_MAIN10;
        }
    }
    formats | VIDEO_FORMAT_H264
}

fn stage_text(stage: Stage) -> &'static str {
    match stage {
        Stage::PlatformInit | Stage::NameResolution => "Finding the PC",
        Stage::AudioStreamInit | Stage::RtspHandshake => "Negotiating the stream",
        Stage::ControlStreamInit | Stage::ControlStreamStart => "Opening the control channel",
        Stage::VideoStreamInit | Stage::VideoStreamStart => "Starting video",
        Stage::AudioStreamStart => "Starting audio",
        Stage::InputStreamInit | Stage::InputStreamStart => "Connecting input",
    }
}

pub fn termination_text(code: i32) -> Option<String> {
    use nightjar_core::session::error;
    Some(match code {
        error::GRACEFUL => return None,
        error::NO_VIDEO_TRAFFIC => {
            "No video arrived from the PC. A firewall may be blocking UDP ports 47998-48000.".into()
        }
        error::NO_VIDEO_FRAME => "Video arrived but no complete picture could be assembled.".into(),
        error::UNEXPECTED_EARLY => "The PC ended the stream before it started.".into(),
        error::PROTECTED_CONTENT => "The PC stopped streaming protected content.".into(),
        error::FRAME_CONVERSION => "The PC could not convert the picture for streaming.".into(),
        error::DISCONNECTED => "The connection to the PC was lost.".into(),
        other => format!("The stream ended unexpectedly (error {other})."),
    })
}

/// A launch and session start running on a worker thread.
pub struct Connecting {
    pub shared: Arc<Shared>,
    pub interrupt: Interrupt,
    pub stage: String,
    pub app_name: String,
    pub host_name: String,
    pub host_id: u64,
    pub pacing: PacingMode,
    pub settings: Settings,
    result: Receiver<Result<(Session, Facts), String>>,
}

impl Connecting {
    pub fn start(plan: LaunchPlan, waker: Waker) -> Self {
        // Host-time pacing schedules every picture; the other modes keep only the newest.
        let limit = if plan.pacing == PacingMode::HostTimes { 8 } else { 2 };
        let shared = Shared::new(waker.clone(), limit);
        let interrupt = Interrupt::default();
        let (sender, result) = channel();
        let app_name = plan.app.name.clone();
        let host_name = plan.record.name.clone();
        let (host_id, pacing, settings) = (plan.host_id, plan.pacing, plan.settings.clone());
        let worker_shared = shared.clone();
        let worker_interrupt = interrupt.clone();
        let _ = std::thread::Builder::new()
            .name("connect".into())
            .spawn(move || {
                let result = connect(plan, worker_shared, &worker_interrupt);
                let _ = sender.send(result);
                waker();
            });
        Self {
            shared,
            interrupt,
            stage: "Starting the app on the PC".into(),
            app_name,
            host_name,
            host_id,
            pacing,
            settings,
            result,
        }
    }

    /// Ok(Some) once streaming, Err with a message on failure.
    pub fn poll(&mut self) -> Result<Option<(Session, Facts)>, String> {
        let notices = std::mem::take(&mut *lock(&self.shared.notices));
        let mut keep = Vec::new();
        for notice in notices {
            match notice {
                Notice::Stage(stage) => self.stage = stage_text(stage).into(),
                // The rest is for the stream.
                other => keep.push(other),
            }
        }
        if !keep.is_empty() {
            let mut queue = lock(&self.shared.notices);
            keep.append(&mut queue);
            *queue = keep;
        }
        match self.result.try_recv() {
            Ok(Ok(session)) => Ok(Some(session)),
            Ok(Err(e)) => Err(e),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("The connection attempt ended.".into()),
        }
    }

    pub fn cancel(&self) {
        self.interrupt.interrupt();
    }
}

fn connect(plan: LaunchPlan, shared: Arc<Shared>, interrupt: &Interrupt) -> Result<(Session, Facts), String> {
    let settings = &plan.settings;
    let (width, height, fps, bitrate) = stream_mode(settings, plan.display);
    let audio = match settings.audio {
        AudioChannels::Stereo => AudioConfiguration::STEREO,
        AudioChannels::Surround51 => AudioConfiguration::SURROUND_51,
        AudioChannels::Surround71 => AudioConfiguration::SURROUND_71,
    };
    let ri_key: [u8; 16] = random_bytes();
    let key_id_bytes: [u8; 4] = random_bytes();
    let ri_key_id = i32::from_be_bytes(key_id_bytes) & 0x7FFF_FFFF;
    let refresh_x100 = (plan.display.refresh_hz * 100.0).round() as u32;
    let request = LaunchRequest {
        app_id: plan.app.id,
        app_uuid: plan.app.uuid.clone(),
        role: plan.app.role(false),
        width,
        height,
        fps,
        client_refresh_rate_x100: refresh_x100,
        vrr: plan.pacing == PacingMode::HostTimes,
        hdr: settings.hdr,
        audio,
        ri_key,
        ri_key_id,
        play_local_audio: settings.play_audio_on_host,
        gamepad_mask: 0xFFFF,
        ..Default::default()
    };
    let port = if plan.record.http_port == 0 {
        nightjar_core::http::DEFAULT_HTTP_PORT
    } else {
        plan.record.http_port
    };
    let pem = plan
        .record
        .server_certificate
        .as_deref()
        .ok_or("This PC is not paired.")?;
    let client = HostClient::new(&plan.record.address, port, plan.identity.clone())
        .and_then(|c| c.with_server_certificate_pem(pem))
        .map_err(|e| e.to_string())?
        .with_https_port(plan.info.https_port);
    let launched = if plan.resume {
        client.resume(&request, &plan.info)
    } else {
        client.launch(&request, &plan.info)
    };
    let launched = match launched {
        Ok(Some(launched)) => launched,
        Ok(None) => return Err("The PC declined to start the app.".into()),
        Err(nightjar_core::http::Error::Status { code, message }) => {
            return Err(format!("The PC could not start the app ({code}: {message})."))
        }
        Err(e) => return Err(format!("Starting the app failed: {e}")),
    };
    if interrupt.is_interrupted() {
        return Err("Cancelled.".into());
    }
    let scm = plan
        .info
        .capabilities
        .trusted_codec_mode_support(plan.info.server_codec_mode_support);
    let mut iv = [0u8; 16];
    iv[..4].copy_from_slice(&ri_key_id.to_be_bytes());
    let stream = StreamConfig {
        width: width as i32,
        height: height as i32,
        fps: fps as i32,
        bitrate_kbps: bitrate as i32,
        packet_size: 1392,
        audio,
        supported_video_formats: video_formats(settings.codec, settings.hdr, scm),
        client_refresh_rate_x100: refresh_x100 as i32,
        color_space: if settings.hdr { COLORSPACE_REC_2020 } else { COLORSPACE_REC_709 },
        color_range: COLOR_RANGE_LIMITED,
        encryption_flags: ENCFLG_ALL,
        remote_input_aes_key: ri_key,
        remote_input_aes_iv: iv,
        video_capabilities: 0,
        audio_capabilities: CAPABILITY_DIRECT_SUBMIT,
        ..Default::default()
    };
    let host = HostInfo {
        address: plan.record.address.clone(),
        app_version: plan.info.app_version.clone().unwrap_or_else(|| "7.1.431.-1".into()),
        rtsp_session_url: launched.rtsp_session_url,
        server_codec_mode_support: if scm != 0 { scm } else { SCM_H264 },
    };
    let session = Session::start(
        &host,
        SessionConfig {
            stream,
            location: Location::Auto,
            network_grace: Duration::from_secs(3),
            input_only: false,
        },
        Box::new(pipeline::Decoder::new(shared.clone(), plan.hardware)),
        Box::new(pipeline::Audio::new(shared.clone())),
        Arc::new(pipeline::Events { shared }),
        interrupt,
    )
    .map_err(|e| format!("Could not start the stream: {e}"))?;
    let outcome = session.outcome();
    let facts = Facts {
        width: outcome.config.width,
        height: outcome.config.height,
        fps: outcome.config.fps,
        bitrate_kbps: outcome.config.bitrate_kbps,
        format: outcome.negotiated_video_format,
        host_timing: outcome.host_control_messages.contains(&HOST_TIMING_MESSAGE_TYPE),
        clock_sync: outcome.host_control_messages.contains(&CLOCK_SYNC_MESSAGE_TYPE),
        app_name: plan.app.name.clone(),
        host_name: plan.record.name.clone(),
    };
    Ok((session, facts))
}

/// Why a stream ended.
pub enum Exit {
    /// The user left (combo, controller combo, overlay button).
    User,
    /// The window was closed: leave Nightjar too.
    CloseWindow,
    /// The host ended it or the connection failed; Some carries the message.
    Ended(Option<String>),
}

/// One-second statistics for the overlay.
#[derive(Default)]
struct Window {
    started: Option<Instant>,
    shown: u32,
    dropped_pacing: u32,
    late: u32,
    render_us: u64,
    hold_us: i64,
    latency_us: i64,
    latency_n: u32,
    host_latency: u64,
    host_latency_n: u32,
    network_us: u64,
    network_n: u32,
    timing_host_us: u64,
    timing_encode_us: u64,
    timing_n: u32,
}

#[derive(Default, Clone)]
pub struct Overlay {
    pub lines: Vec<(String, String)>,
}

/// Unwraps 90 kHz RTP timestamps into host microseconds.
#[derive(Default)]
struct RtpClock {
    last: Option<(u32, i64)>,
}

impl RtpClock {
    fn host_us(&mut self, rtp: u32) -> i64 {
        let ticks = match self.last {
            Some((last, ticks)) => ticks + i64::from(rtp.wrapping_sub(last) as i32),
            None => i64::from(rtp),
        };
        self.last = Some((rtp, ticks));
        ticks * 100 / 9
    }
}

pub struct Streaming {
    pub session: Session,
    pub shared: Arc<Shared>,
    pub facts: Facts,
    pub pacer: Pacer,
    pub texture: VideoTexture,
    pub overlay_visible: bool,
    pub overlay: Overlay,
    pub exit: Option<Exit>,
    pub toast: Option<(String, Instant)>,
    pub host_id: u64,
    settings: Settings,
    keys_down: HashSet<u16>,
    captured: bool,
    absolute: bool,
    relative_remainder: (f32, f32),
    offset: ClockOffset,
    rtp_clock: RtpClock,
    last_probe: Instant,
    window: Window,
    last_counters: (u64, u64, u64, u64, u64, u64),
    last_video: (u64, u64),
    started: Instant,
    first_frame: bool,
    /// Pictures shown at least once since the last overlay redraw.
    pub needs_redraw: bool,
    timings: VecDeque<(u32, u64)>,
}

/// What the main loop should do next.
pub enum Wait {
    /// Present now.
    Now,
    /// Wait for events up to this long.
    For(Duration),
}

impl Streaming {
    pub fn new(connecting: Connecting, session: Session, facts: Facts, display: Display) -> Self {
        let min_interval = if display.refresh_hz > 0.0 {
            (1_000_000.0 / display.refresh_hz) as i64
        } else {
            0
        };
        if facts.host_timing {
            let _ = session.send(ClientMessage::HostTimingRequest(HostTimingRequest { enabled: true }));
        }
        let settings = connecting.settings.clone();
        Self {
            session,
            shared: connecting.shared,
            facts,
            pacer: Pacer::new(connecting.pacing, min_interval),
            texture: VideoTexture::default(),
            overlay_visible: settings.overlay,
            overlay: Overlay::default(),
            exit: None,
            toast: None,
            host_id: connecting.host_id,
            absolute: settings.mouse_mode == MouseMode::Absolute,
            settings,
            keys_down: HashSet::new(),
            captured: false,
            relative_remainder: (0.0, 0.0),
            offset: ClockOffset::default(),
            rtp_clock: RtpClock::default(),
            last_probe: Instant::now(),
            window: Window::default(),
            last_counters: (0, 0, 0, 0, 0, 0),
            last_video: (0, 0),
            started: Instant::now(),
            first_frame: false,
            needs_redraw: true,
            timings: VecDeque::new(),
        }
    }

    pub fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    /// Mouse capture: relative motion with a hidden, confined pointer.
    pub fn set_capture(&mut self, window: *mut SDL_Window, on: bool) {
        self.captured = on;
        // SAFETY: a live window.
        unsafe {
            if self.absolute {
                SDL_SetWindowRelativeMouseMode(window, false);
                if on {
                    SDL_HideCursor();
                } else {
                    SDL_ShowCursor();
                }
            } else {
                SDL_SetWindowRelativeMouseMode(window, on);
            }
            let grab = on && self.settings.capture_system_keys;
            SDL_SetWindowKeyboardGrab(window, grab);
        }
    }

    pub fn raise_all_keys(&mut self) {
        let keys: Vec<u16> = self.keys_down.drain().collect();
        self.session.input(|q| {
            for key in keys {
                let _ = q.send_keyboard(0x8000 | key, false, 0, 0);
            }
        });
    }

    /// Handles queued notices from the session threads. Control events that need the
    /// controllers are applied to `pads`.
    pub fn drain_notices(&mut self, pads: &Gamepads) {
        let notices = std::mem::take(&mut *lock(&self.shared.notices));
        for notice in notices {
            match notice {
                Notice::Terminated(code) => {
                    self.exit.get_or_insert(Exit::Ended(termination_text(code)));
                }
                Notice::DecoderFallback(text) => self.show_toast(text),
                Notice::Control(event) => self.control_event(event, pads),
                Notice::Stage(_) | Notice::Started => {}
            }
        }
    }

    fn control_event(&mut self, event: ControlEvent, pads: &Gamepads) {
        match event {
            ControlEvent::Rumble {
                controller,
                low_frequency,
                high_frequency,
            } => pads.rumble(controller, low_frequency, high_frequency),
            ControlEvent::RumbleTriggers { controller, left, right } => {
                pads.rumble_triggers(controller, left, right)
            }
            ControlEvent::SetControllerLed { controller, r, g, b } => pads.set_led(controller, r, g, b),
            ControlEvent::SetMotionEventState {
                controller,
                motion_type,
                report_rate_hz,
            } => pads.set_motion(controller, motion_type, report_rate_hz),
            ControlEvent::HostTiming(records) => {
                for r in records {
                    self.window.timing_host_us += u64::from(r.host_us);
                    self.window.timing_encode_us += u64::from(r.encode_us);
                    self.window.timing_n += 1;
                    self.timings.push_back((r.frame, u64::from(r.host_us)));
                    if self.timings.len() > 64 {
                        self.timings.pop_front();
                    }
                }
            }
            ControlEvent::ClockSync { reply, received } => {
                let receive_us = session::now_us().saturating_sub(received.elapsed().as_micros() as u64);
                self.offset.add(&reply, receive_us);
            }
            ControlEvent::HdrMode { enabled: true, .. } => {
                self.show_toast("The PC switched to HDR (shown tone-mapped)");
            }
            _ => {}
        }
    }

    /// Moves decoded pictures into the pacer; sends a clock sync probe when due.
    pub fn pump(&mut self) {
        let frames: Vec<_> = lock(&self.shared.frames).drain(..).collect();
        for decoded in frames {
            self.pacer.accept(decoded);
        }
        if self.facts.clock_sync && self.last_probe.elapsed() >= Duration::from_millis(250) {
            self.last_probe = Instant::now();
            let _ = self.session.send_clock_sync_probe(session::now_us());
        }
    }

    /// How long the loop may sleep before the next picture is due.
    pub fn wait(&self) -> Wait {
        match self.pacer.next_due() {
            Some(due) => {
                let now = session::now_us();
                if due <= now {
                    Wait::Now
                } else {
                    Wait::For(Duration::from_micros(due - now))
                }
            }
            None => Wait::For(Duration::from_millis(if self.overlay_visible { 250 } else { 500 })),
        }
    }

    /// Shows the due picture, if any. Returns whether something was drawn.
    pub fn present(
        &mut self,
        renderer: *mut SDL_Renderer,
        mut draw_overlay: impl FnMut(&mut Self, *mut SDL_Renderer),
    ) -> bool {
        let now = session::now_us();
        let (picture, dropped) = self.pacer.take_due(now);
        self.window.dropped_pacing += dropped;
        self.shared.counters.replaced.fetch_add(u64::from(dropped), Ordering::Relaxed);
        let has_picture = picture.is_some();
        if !has_picture && !self.needs_redraw {
            return false;
        }
        let begin = session::now_us();
        if let Some(p) = &picture {
            if !self.texture.upload(renderer, &p.decoded.frame) {
                self.show_toast(format!("Cannot show {} pictures", p.decoded.frame.format_name()));
            }
            if !self.first_frame {
                self.first_frame = true;
            }
        }
        // SAFETY: drawing on a live renderer.
        unsafe {
            SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255);
            SDL_RenderClear(renderer);
            if !self.texture.texture.is_null() {
                let (mut w, mut h) = (0, 0);
                SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h);
                let dst = letterbox(self.texture.size(), (w, h));
                SDL_RenderTexture(renderer, self.texture.texture, std::ptr::null(), &dst);
            }
        }
        draw_overlay(self, renderer);
        // SAFETY: a live renderer.
        unsafe { SDL_RenderPresent(renderer) };
        let presented = session::now_us();
        self.needs_redraw = false;
        if let Some(p) = picture {
            let w = &mut self.window;
            w.shown += 1;
            w.render_us += presented - begin;
            w.hold_us += p.hold_us;
            w.late += u32::from(p.late);
            if p.decoded.host_latency != 0 {
                w.host_latency += u64::from(p.decoded.host_latency);
                w.host_latency_n += 1;
            }
            w.network_us += p.decoded.enqueue_us.saturating_sub(p.decoded.receive_us);
            w.network_n += 1;
            let host_us = self.rtp_clock.host_us(p.decoded.rtp);
            if let Some(offset) = self.offset.offset_at(presented as i64) {
                let latency = presented as i64 - (host_us + offset);
                if (0..2_000_000).contains(&latency) {
                    w.latency_us += latency;
                    w.latency_n += 1;
                }
            }
        }
        true
    }

    /// Recomputes the overlay text once a second. Returns whether it changed.
    pub fn update_overlay(&mut self) -> bool {
        let started = *self.window.started.get_or_insert_with(Instant::now);
        let elapsed = started.elapsed();
        if elapsed < Duration::from_secs(1) {
            return false;
        }
        let secs = elapsed.as_secs_f64();
        let stats = self.session.stats();
        let c = &self.shared.counters;
        let now = (
            stats.frames_submitted,
            c.decoded.load(Ordering::Relaxed),
            c.decode_us_total.load(Ordering::Relaxed),
            c.decode_errors.load(Ordering::Relaxed),
            c.audio_dropped.load(Ordering::Relaxed),
            c.audio_underruns.load(Ordering::Relaxed),
        );
        let last = self.last_counters;
        self.last_counters = now;
        let received = now.0 - last.0;
        let decoded = now.1 - last.1;
        let decode_ms = if decoded > 0 {
            (now.2 - last.2) as f64 / decoded as f64 / 1000.0
        } else {
            0.0
        };
        let video = (stats.video.frames_completed, stats.video.frame_drops);
        let (completed, drops) = (video.0 - self.last_video.0, video.1 - self.last_video.1);
        self.last_video = video;
        let loss = if completed + drops > 0 {
            drops as f64 * 100.0 / (completed + drops) as f64
        } else {
            0.0
        };
        let w = std::mem::take(&mut self.window);
        let avg = |sum: f64, n: u32| if n > 0 { sum / f64::from(n) } else { 0.0 };
        let mut lines = Vec::new();
        let f = &self.facts;
        lines.push((
            "Stream".into(),
            format!("{}x{} {} fps, {}", f.width, f.height, f.fps, codec_name(f.format)),
        ));
        lines.push(("Decoder".into(), lock(&self.shared.decoder_name).clone()));
        lines.push((
            "Frames".into(),
            format!(
                "{:.0} in, {:.0} decoded, {:.0} shown",
                received as f64 / secs,
                decoded as f64 / secs,
                f64::from(w.shown) / secs
            ),
        ));
        lines.push((
            "Network".into(),
            match stats.rtt {
                Some((rtt, var)) => format!(
                    "{:.1} ms RTT (±{:.1}), {:.1}% frames lost",
                    rtt.as_secs_f64() * 1000.0,
                    var.as_secs_f64() * 1000.0,
                    loss
                ),
                None => format!("{loss:.1}% frames lost"),
            },
        ));
        if w.network_n > 0 && w.network_us / u64::from(w.network_n) >= 50 {
            lines.push((
                "Receive".into(),
                format!("{:.1} ms first packet to frame", avg(w.network_us as f64, w.network_n) / 1000.0),
            ));
        }
        lines.push((
            "Decode".into(),
            format!("{decode_ms:.2} ms{}", if now.3 > last.3 { ", errors" } else { "" }),
        ));
        lines.push((
            "Render".into(),
            format!("{:.2} ms", avg(w.render_us as f64, w.shown) / 1000.0),
        ));
        let mut pacing = self.pacer.mode.label().to_owned();
        if self.pacer.mode == PacingMode::HostTimes {
            if let Some(delay) = self.pacer.clock.delay_us() {
                pacing.push_str(&format!(
                    ", hold {:.1} ms, {} late",
                    avg(w.hold_us as f64, w.shown) / 1000.0,
                    w.late
                ));
                let _ = delay;
            }
        }
        if w.dropped_pacing > 0 {
            pacing.push_str(&format!(", {} replaced", w.dropped_pacing));
        }
        lines.push(("Pacing".into(), pacing));
        if w.timing_n > 0 {
            lines.push((
                "Host".into(),
                format!(
                    "{:.1} ms present to send, {:.1} ms encode",
                    avg(w.timing_host_us as f64, w.timing_n) / 1000.0,
                    avg(w.timing_encode_us as f64, w.timing_n) / 1000.0
                ),
            ));
        } else if w.host_latency_n > 0 {
            lines.push((
                "Host".into(),
                format!("{:.1} ms processing", avg(w.host_latency as f64, w.host_latency_n) / 10.0),
            ));
        }
        if w.latency_n > 0 {
            lines.push((
                "Latency".into(),
                format!(
                    "{:.1} ms PC frame to display",
                    avg(w.latency_us as f64, w.latency_n) / 1000.0
                ),
            ));
        }
        let channels = c.audio_channels.load(Ordering::Relaxed);
        let mut audio = match channels {
            0 => "no output device".to_owned(),
            2 => "stereo".to_owned(),
            6 => "5.1".to_owned(),
            8 => "7.1".to_owned(),
            n => format!("{n} channels"),
        };
        if channels > 0 {
            audio.push_str(&format!(
                ", {:.0} ms buffered",
                c.audio_queued_us.load(Ordering::Relaxed) as f64 / 1000.0
            ));
        }
        if now.4 > last.4 {
            audio.push_str(&format!(", {} packets dropped to stay in sync", now.4 - last.4));
        }
        if now.5 > last.5 {
            audio.push_str(&format!(", {} underruns", now.5 - last.5));
        }
        lines.push(("Audio".into(), audio));
        self.overlay = Overlay { lines };
        self.window.started = Some(Instant::now());
        true
    }

    /// Whether the stream has shown its first picture.
    pub fn has_picture(&self) -> bool {
        self.first_frame
    }

    pub fn since_start(&self) -> Duration {
        self.started.elapsed()
    }

    /// Handles one SDL event while streaming.
    pub fn handle(
        &mut self,
        event: &SDL_Event,
        window: *mut SDL_Window,
        renderer: *mut SDL_Renderer,
        pads: &mut Gamepads,
    ) {
        // SAFETY: union members read by event type.
        unsafe {
            let kind = SDL_EventType(event.r#type);
            match kind {
                SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED => {
                    self.exit = Some(Exit::CloseWindow);
                }
                SDL_EVENT_KEY_DOWN | SDL_EVENT_KEY_UP => self.key(&event.key, window),
                SDL_EVENT_WINDOW_FOCUS_LOST => {
                    self.raise_all_keys();
                    if self.captured {
                        self.set_capture(window, false);
                        self.captured = true; // recapture on focus
                    }
                }
                SDL_EVENT_WINDOW_FOCUS_GAINED => {
                    if self.captured {
                        self.set_capture(window, true);
                    }
                }
                SDL_EVENT_MOUSE_MOTION => {
                    let m = event.motion;
                    if !self.captured {
                        return;
                    }
                    if self.absolute {
                        self.absolute_position(m.x, m.y, window, renderer);
                    } else {
                        let (rx, ry) = self.relative_remainder;
                        let (dx, dy) = (m.xrel + rx, m.yrel + ry);
                        let (ix, iy) = (dx.trunc(), dy.trunc());
                        self.relative_remainder = (dx - ix, dy - iy);
                        if ix != 0.0 || iy != 0.0 {
                            self.session.input(|q| {
                                let _ = q.send_mouse_move(ix as i16, iy as i16);
                            });
                        }
                    }
                }
                SDL_EVENT_MOUSE_BUTTON_DOWN | SDL_EVENT_MOUSE_BUTTON_UP => {
                    let b = event.button;
                    if !self.captured {
                        // The first click only takes the pointer.
                        if b.down {
                            self.set_capture(window, true);
                            if self.absolute {
                                self.absolute_position(b.x, b.y, window, renderer);
                            }
                        }
                        return;
                    }
                    let button = match i32::from(b.button) {
                        SDL_BUTTON_LEFT => mouse_button::LEFT,
                        SDL_BUTTON_MIDDLE => mouse_button::MIDDLE,
                        SDL_BUTTON_RIGHT => mouse_button::RIGHT,
                        SDL_BUTTON_X1 => mouse_button::X1,
                        SDL_BUTTON_X2 => mouse_button::X2,
                        _ => return,
                    };
                    self.session.input(|q| {
                        let _ = q.send_mouse_button(button, b.down);
                    });
                }
                SDL_EVENT_MOUSE_WHEEL => {
                    if !self.captured {
                        return;
                    }
                    let w = event.wheel;
                    let flip = if w.direction == SDL_MOUSEWHEEL_FLIPPED { -1.0 } else { 1.0 };
                    let (x, y) = (w.x * flip, w.y * flip);
                    self.session.input(|q| {
                        if y != 0.0 {
                            let _ = q.send_high_res_scroll((y * 120.0).clamp(-32000.0, 32000.0) as i16);
                        }
                        if x != 0.0 {
                            let _ = q.send_high_res_hscroll((x * 120.0).clamp(-32000.0, 32000.0) as i16);
                        }
                    });
                }
                SDL_EVENT_GAMEPAD_ADDED => {
                    pads.added(event.gdevice.which);
                    self.session.input(|q| pads.announce_new(q));
                }
                SDL_EVENT_GAMEPAD_REMOVED => {
                    if let Some(index) = pads.removed(event.gdevice.which) {
                        self.session.input(|q| pads.send_removed(index, q));
                    }
                }
                _ => {
                    let focused = SDL_GetWindowFlags(window) & SDL_WINDOW_INPUT_FOCUS != 0;
                    if !self.settings.controllers || (!focused && !self.settings.background_controllers) {
                        return;
                    }
                    let mut action = None;
                    self.session.input(|q| action = pads.handle(event, q));
                    if action == Some(Action::Quit) {
                        self.exit = Some(Exit::User);
                    }
                }
            }
        }
    }

    fn absolute_position(&self, x: f32, y: f32, window: *mut SDL_Window, renderer: *mut SDL_Renderer) {
        // SAFETY: live window and renderer.
        let (density, (w, h)) = unsafe {
            let mut size = (0, 0);
            SDL_GetCurrentRenderOutputSize(renderer, &mut size.0, &mut size.1);
            (SDL_GetWindowPixelDensity(window).max(0.01), size)
        };
        let rect = letterbox(self.texture.size(), (w, h));
        let (px, py) = (x * density - rect.x, y * density - rect.y);
        let (rw, rh) = (rect.w.max(1.0), rect.h.max(1.0));
        let cx = px.clamp(0.0, rw - 1.0);
        let cy = py.clamp(0.0, rh - 1.0);
        let (sw, sh) = (rw.min(32767.0), rh.min(32767.0));
        self.session.input(|q| {
            let _ = q.send_mouse_position(
                (cx * sw / rw) as i16,
                (cy * sh / rh) as i16,
                sw as i16,
                sh as i16,
            );
        });
    }

    fn key(&mut self, k: &SDL_KeyboardEvent, window: *mut SDL_Window) {
        if k.repeat {
            return;
        }
        let m = k.r#mod.0;
        let has = |flag: SDL_Keymod| m & flag.0 != 0;
        if k.down && has(SDL_KMOD_CTRL) && has(SDL_KMOD_ALT) && has(SDL_KMOD_SHIFT) {
            match k.scancode {
                SDL_SCANCODE_Q => {
                    self.exit = Some(Exit::User);
                    return;
                }
                SDL_SCANCODE_Z => {
                    let on = !self.captured;
                    self.raise_all_keys();
                    self.set_capture(window, on);
                    self.show_toast(if on {
                        "Input captured. Ctrl+Alt+Shift+Z releases it."
                    } else {
                        "Input released. Click the stream to capture it."
                    });
                    return;
                }
                SDL_SCANCODE_S => {
                    self.overlay_visible = !self.overlay_visible;
                    self.needs_redraw = true;
                    return;
                }
                SDL_SCANCODE_M => {
                    self.absolute = !self.absolute;
                    let on = self.captured;
                    self.set_capture(window, false);
                    self.set_capture(window, on);
                    self.show_toast(if self.absolute {
                        "Mouse: desktop pointer"
                    } else {
                        "Mouse: captured for games"
                    });
                    return;
                }
                SDL_SCANCODE_X => {
                    // SAFETY: a live window.
                    unsafe {
                        let fullscreen = SDL_GetWindowFlags(window) & SDL_WINDOW_FULLSCREEN != 0;
                        SDL_SetWindowFullscreen(window, !fullscreen);
                    }
                    self.raise_all_keys();
                    return;
                }
                SDL_SCANCODE_D => {
                    self.raise_all_keys();
                    // SAFETY: a live window.
                    unsafe { SDL_MinimizeWindow(window) };
                    return;
                }
                _ => {}
            }
        }
        if !self.captured {
            return;
        }
        let system_keys = self.settings.capture_system_keys;
        let Some((code, flags)) = keymap::virtual_key(k.scancode, system_keys) else {
            return;
        };
        let mut modifiers = 0u8;
        if has(SDL_KMOD_CTRL) {
            modifiers |= modifier::CTRL;
        }
        if has(SDL_KMOD_ALT) {
            modifiers |= modifier::ALT;
        }
        if has(SDL_KMOD_SHIFT) {
            modifiers |= modifier::SHIFT;
        }
        if has(SDL_KMOD_GUI) && system_keys {
            modifiers |= modifier::META;
        }
        if k.down {
            self.keys_down.insert(code);
        } else {
            self.keys_down.remove(&code);
        }
        self.session.input(|q| {
            let _ = q.send_keyboard(0x8000 | code, k.down, modifiers, flags);
        });
    }

    /// Ends the stream: releases input, stops the session.
    pub fn finish(mut self, window: *mut SDL_Window, pads: &mut Gamepads) {
        self.raise_all_keys();
        self.set_capture(window, false);
        // SAFETY: a live window.
        unsafe { SDL_ShowCursor() };
        pads.reset();
        self.texture.destroy();
        self.session.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display() -> Display {
        Display {
            width: 2560,
            height: 1440,
            refresh_hz: 143.9,
        }
    }

    #[test]
    fn native_mode_follows_the_display() {
        let (w, h, fps, kbps) = stream_mode(&Settings::default(), display());
        assert_eq!((w, h, fps), (2560, 1440, 144));
        assert_eq!(kbps, config::default_bitrate_kbps(2560, 1440, 144));
        let settings = Settings {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: 30000,
            ..Default::default()
        };
        assert_eq!(stream_mode(&settings, display()), (1920, 1080, 60, 30000));
    }

    #[test]
    fn formats_respect_the_host() {
        let all = SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10 | SCM_AV1_MAIN8 | SCM_AV1_MAIN10;
        assert_eq!(
            video_formats(CodecPreference::Auto, false, all),
            VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265
        );
        assert_eq!(
            video_formats(CodecPreference::Av1, true, all),
            VIDEO_FORMAT_H264 | VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10
        );
        assert_eq!(video_formats(CodecPreference::Hevc, false, SCM_H264), VIDEO_FORMAT_H264);
        assert_eq!(video_formats(CodecPreference::H264, true, all), VIDEO_FORMAT_H264);
    }

    #[test]
    fn rtp_clock_unwraps() {
        let mut c = RtpClock::default();
        let a = c.host_us(u32::MAX - 899);
        let b = c.host_us(600);
        // 1500 ticks of 90 kHz across the wrap, give or take the rounding.
        assert!((b - a - 16_667).abs() <= 1, "{}", b - a);
    }

    #[test]
    fn terminations_read_well() {
        assert!(termination_text(0).is_none());
        assert!(termination_text(-100).unwrap().contains("UDP"));
    }
}
