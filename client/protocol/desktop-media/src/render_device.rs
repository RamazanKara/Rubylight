//! Render-device loss detection, recovery guard and reconnect state across the launcher and stream.

use sdl3_sys::everything::*;
#[cfg(any(windows, test, feature = "test-support"))]
use std::collections::VecDeque;
#[cfg(any(windows, test, feature = "test-support"))]
use std::time::{Duration, Instant};

use crate::media;
#[cfg(windows)]
use crate::stream::LaunchPlan;

/// SDL's events after which the renderer's device is gone: made again by SDL with every texture
/// lost (`SDL_EVENT_RENDER_DEVICE_RESET`), or not at all (`SDL_EVENT_RENDER_DEVICE_LOST`), when
/// SDL's Direct3D 11 renderer draws and presents nothing ever again.
pub fn renderer_gone(kind: SDL_EventType) -> bool {
    matches!(kind, SDL_EVENT_RENDER_DEVICE_RESET | SDL_EVENT_RENDER_DEVICE_LOST)
}

/// Whether SDL already queued a render-device reset or loss event.
/// Checked before reading Direct3D 11 renderer properties or calling `SDL_SetRenderVSync`: once
/// SDL3's `D3D11_HandleDeviceLost` ran, its device or swap-chain pointer may be null or freed.
pub fn event_pending() -> bool {
    // SAFETY: `SDL_HasEvent` only inspects SDL's event queue on the main thread.
    unsafe {
        SDL_HasEvent(SDL_EVENT_RENDER_DEVICE_RESET.0)
            || SDL_HasEvent(SDL_EVENT_RENDER_DEVICE_LOST.0)
    }
}

/// Events still forwarded to a running stream after its render device is lost (window/app close
/// and SDL's own device-loss events), avoiding renderer/swapchain-touching events like display or
/// HDR changes.
pub fn is_exit_event(kind: SDL_EventType) -> bool {
    matches!(
        kind,
        SDL_EVENT_QUIT
            | SDL_EVENT_WINDOW_CLOSE_REQUESTED
            | SDL_EVENT_RENDER_DEVICE_RESET
            | SDL_EVENT_RENDER_DEVICE_LOST
    )
}

/// Why the stream ends after SDL's render device event `kind`: the stream's textures and the
/// decoder's GPU planes are gone with the device, and the picture would freeze without a word.
pub(crate) fn ended_reason(kind: SDL_EventType) -> Option<&'static str> {
    match kind {
        SDL_EVENT_RENDER_DEVICE_RESET => {
            Some("The graphics driver was reset, which ended the stream. Connect again.")
        }
        SDL_EVENT_RENDER_DEVICE_LOST => {
            Some("The graphics device was lost (the driver stopped or the GPU was removed), which ended the stream. Connect again.")
        }
        _ => None,
    }
}

/// The message for a stream ended by a lost render device when detected outside SDL's events
/// (for instance `SDL_RenderPresent` failing on `DXGI_ERROR_DEVICE_RESET`).
pub fn lost_reason() -> &'static str {
    "The graphics driver was reset, which ended the stream. Connect again."
}

/// Returns a renderer pointer property unless SDL already queued a render-device reset/loss event
/// (after `D3D11_HandleDeviceLost` fails, SDL's device and swap-chain pointers may be dangling).
#[cfg(windows)]
pub fn live_renderer_pointer(
    renderer: *mut SDL_Renderer,
    name: *const std::ffi::c_char,
) -> *mut std::ffi::c_void {
    if event_pending() {
        return std::ptr::null_mut();
    }
    crate::sdl::renderer_pointer(renderer, name)
}

/// Pure decision behind [`after_present_lost`]: whether a present observed render-device loss,
/// including when `SDL_RenderPresent` returned `true` after `D3D11_HandleDeviceLost` queued
/// `SDL_EVENT_RENDER_DEVICE_RESET` or when MSYS2 SDL3 hit `DXGI_ERROR_DEVICE_RESET` without an event.
pub(crate) fn after_present_lost_for(present_failed: bool, pending: bool, removed: bool) -> bool {
    let _ = present_failed;
    pending || removed
}

/// Checks whether a present observed render-device loss (whether `SDL_RenderPresent` returned
/// `false` on `DXGI_ERROR_DEVICE_RESET`/`REMOVED`, or returned `true` after SDL's own
/// `D3D11_HandleDeviceLost` queued `SDL_EVENT_RENDER_DEVICE_RESET`), logging the diagnostic reason.
pub(crate) fn after_present_lost(present_failed: bool, device: &media::DeviceRef) -> bool {
    let pending = event_pending();
    let removed = device.removed_reason();
    if after_present_lost_for(present_failed, pending, removed.is_some()) {
        if let Some(why) = &removed {
            eprintln!("nightjar: render device lost during present ({why})");
        } else if present_failed {
            eprintln!(
                "nightjar: present failed after render device loss ({})",
                crate::sdl::error()
            );
        } else {
            eprintln!("nightjar: render device reset/lost queued during present");
        }
        true
    } else {
        if present_failed {
            eprintln!("nightjar: SDL_RenderPresent failed: {}", crate::sdl::error());
        }
        false
    }
}

/// How often Windows' renderer may be made again (`App::recover_renderer`): a driver that loses
/// every new device would otherwise have Nightjar make it again forever, showing nothing.
#[cfg(any(windows, test, feature = "test-support"))]
#[derive(Default)]
pub struct RemakeGuard(VecDeque<Instant>);

#[cfg(any(windows, test, feature = "test-support"))]
impl RemakeGuard {
    pub const MOST: usize = 5;
    pub const WITHIN: Duration = Duration::from_secs(60);

    /// Whether the renderer may be made again at `now`, counted when it may: at most `MOST` times
    /// within `WITHIN`.
    pub fn allow(&mut self, now: Instant) -> bool {
        while self
            .0
            .front()
            .is_some_and(|&at| now.saturating_duration_since(at) >= Self::WITHIN)
        {
            self.0.pop_front();
        }
        if self.0.len() >= Self::MOST {
            return false;
        }
        self.0.push_back(now);
        true
    }
}

/// Windows-only renderer recovery state stored on `App`.
#[cfg(windows)]
#[derive(Default)]
pub struct Recovery {
    /// SDL's Direct3D 11 device was lost or made again (`renderer_gone`), or `SDL_RenderPresent`
    /// observed a removed device; the launcher's next frame runs `recover_renderer`.
    pub lost: bool,
    /// Rate limit for renderer remakes (`RemakeGuard`).
    pub remakes: RemakeGuard,
    /// References to lost D3D11 devices from ended streams or failed launcher presents, kept until
    /// `recover_renderer` logs their removal reason and flushes deferred releases on them.
    pub lost_devices: Vec<media::DeviceRef>,
    /// Mid-stream reconnect waiting for `recover_renderer` to replace the lost D3D11 renderer.
    pub pending_reconnect: Option<LaunchPlan>,
}

#[cfg(windows)]
impl Recovery {
    pub fn is_lost(&self) -> bool {
        self.lost || event_pending()
    }

    pub fn remember_device(&mut self, device: media::DeviceRef) {
        if device.is_some() {
            self.lost_devices.push(device);
        }
    }

    pub fn defer_reconnect(&mut self, mut plan: LaunchPlan) {
        plan.device = media::DeviceRef::default();
        self.pending_reconnect = Some(plan);
    }

    pub fn check_launcher_present_failure(&mut self, present_failed: bool, device_raw: *mut std::ffi::c_void) -> bool {
        if event_pending() {
            return true;
        }
        if present_failed {
            let device = media::DeviceRef::of_device(device_raw);
            if device.is_removed() {
                self.remember_device(device);
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HostRecord, Settings};
    use crate::stream::{color_probe, connect, present::PacingMode, recovery, Display, Exit, LaunchPlan};
    use nightjar_core::http::{App, AppRole, ClientIdentity, ServerInfo};
    use std::sync::Arc;

    /// Tier 1.5 + Tier 3.2: a lost render device no longer tells the user to restart Nightjar, and
    /// `Exit::RenderDeviceLost` triggers automatic reconnect on Windows only once video started.
    #[test]
    fn render_device_lost_exit_retries_after_video_started_on_windows_and_never_says_restart() {
        for kind in [SDL_EVENT_RENDER_DEVICE_RESET, SDL_EVENT_RENDER_DEVICE_LOST] {
            let reason = ended_reason(kind).expect("ends the stream");
            assert!(
                !reason.contains("Restart Nightjar"),
                "recovery remakes the renderer without restarting: {reason}"
            );
        }
        let exit = Exit::RenderDeviceLost(lost_reason().into());
        assert_eq!(
            exit.retryable(true),
            cfg!(windows),
            "Windows reconnects after renderer recovery"
        );
        assert!(
            !exit.retryable(false),
            "never auto-reconnects when the stream never showed video"
        );
    }

    /// Tier 3.2: mid-stream device-loss reconnect obeys `auto_reconnect`, `away` and `AppRole`,
    /// and requests an immediate first attempt (`retry_now`) once `recover_renderer` succeeds.
    #[test]
    fn device_loss_reconnect_plan_respects_auto_reconnect_away_and_role() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>7</currentgame><state>SUNSHINE_SERVER_BUSY</state><RustHostVersion>3.0.0</RustHostVersion></root>"#;
        let plan = LaunchPlan {
            host_id: 1,
            host_name: "PC".into(),
            record: HostRecord::default(),
            info: ServerInfo::parse(xml, 47989).expect("the answer parses"),
            native: None,
            identity: Arc::new(ClientIdentity::generate().expect("identity")),
            app: App {
                id: 7,
                name: "Game".into(),
                hdr_supported: false,
                uuid: String::new(),
                host_index: None,
                art_version: String::new(),
                control: nightjar_core::http::AppControl::None,
                monitor_resume: false,
            },
            settings: Settings::default(),
            controllers_from: crate::app_types::ControllersFrom::Settings,
            display: Display {
                width: 1920,
                height: 1080,
                refresh_hz: 60.0,
                refresh_reported: true,
            },
            display_luminance: None,
            pacing: PacingMode::Immediate,
            vrr: false,
            hardware: crate::media::Hardware::None,
            device: crate::media::DeviceRef::default(),
            gpu_decoder: None,
            dmabuf: false,
            kms_planes: false,
            color_decodable: (0, String::new()),
            base_decodable: color_probe::Base::software(),
            packed_modes: 0,
            gamepad_mask: 0,
            playstation_mask: 0,
            prepared_controllers: String::new(),
            resume: false,
            join: false,
            confirm_replace: false,
            network: crate::network::Network::default(),
            host_link_mbps: 0,
            pace: None,
            bitrate_limit_kbps: 0,
            small_stream_input_only: false,
            mic_only: false,
            microphone_muted: false,
            listed: true,
            negotiated_packet_size: 0,
            resume_bitrate_kbps: None,
            bitrate_before_manual: None,
            resume_quality: None,
            recovery: None,
            video_shown: false,
        };
        let now = recovery::now_ms();
        let mut rejoined =
            connect::automatic_rejoin(&plan, AppRole::Stream, true, false, 10_000, true, true)
                .expect("mid-stream device loss produces an automatic resume plan");
        assert!(rejoined.resume && rejoined.video_shown);
        let budget = rejoined.recovery.as_mut().expect("recovery backoff attached");
        assert_eq!(
            budget.next_delay(now),
            Some(0),
            "retry_now starts immediately after renderer recovery"
        );
        assert!(
            connect::automatic_rejoin(&plan, AppRole::Stream, false, false, 10_000, true, true)
                .is_none(),
            "auto_reconnect off"
        );
        assert!(
            connect::automatic_rejoin(&plan, AppRole::Stream, true, true, 10_000, true, true)
                .is_none(),
            "window minimized/hidden"
        );
        assert!(
            connect::automatic_rejoin(&plan, AppRole::RemoteMonitor, true, false, 10_000, true, true)
                .is_none(),
            "remote monitor"
        );
    }

    /// Tier 1.5: `Teardown::join_until` waits for a teardown worker and preserves the handle on
    /// timeout (used on Windows by `App::recover_renderer` before destroying the old renderer).
    #[test]
    fn teardown_join_until_waits_for_worker_and_preserves_handle_on_timeout() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let mut td = crate::stream::spawn_background(move || {
            let _ = rx.recv();
        });
        assert!(
            !td.join_until(Instant::now() + Duration::from_millis(10)),
            "times out while worker runs"
        );
        assert!(!td.finished(), "handle preserved when still running");
        tx.send(()).unwrap();
        assert!(
            td.join_until(Instant::now() + Duration::from_secs(5)),
            "joins cleanly once finished"
        );
        assert!(td.finished());
    }

    /// Tier 1.3: `after_present_lost_for` catches SDL3's `D3D11_HandleDeviceLost` even when
    /// `SDL_RenderPresent` returned `true` (queuing `SDL_EVENT_RENDER_DEVICE_RESET`), as well as
    /// MSYS2 SDL3's `DXGI_ERROR_DEVICE_RESET` (false without a queued event).
    #[test]
    fn after_present_lost_detects_device_loss_even_when_present_returned_true() {
        assert!(!after_present_lost_for(false, false, false));
        assert!(!after_present_lost_for(true, false, false), "transient non-loss failure");
        assert!(after_present_lost_for(false, true, false), "SDL queued RENDER_DEVICE_RESET and returned true");
        assert!(after_present_lost_for(true, true, false), "SDL queued RENDER_DEVICE_LOST and returned false");
        assert!(after_present_lost_for(true, false, true), "DXGI_ERROR_DEVICE_RESET without SDL event");
        assert!(after_present_lost_for(false, false, true), "device already removed");
    }
}
