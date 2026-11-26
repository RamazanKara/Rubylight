//! Cross-crate platform wiring bridging graphics, display, and I/O leaf crates in `rust/host`.
use anyhow::Result;
use butterpollo_config::config::Config;
use butterpollo_media_policy::{WarningSink, rtsp::Negotiated};
use butterpollo_session::session::Warnings;
use butterpollo_win_display::{
    display::{
        self, RenderAdapter,
        rubylight_vdd::{self, MonitorStatus, SetFenceRingRequest, SetFrameRingRequest},
    },
    display_recovery::{self, RecoveryHooks},
};
use butterpollo_win_graphics::{
    capture::{
        self, Capture, CaptureEnv, GpuImage,
        idd_push::{VddBridge, VddControl, VddOwned},
    },
    encoder::Encoder,
    timing::StreamingScope,
    user_helper::{HelperPipe, PipedUserChild, UserChild, UserLauncher},
};
use butterpollo_win_io::{
    audio_route::{self, hooks::RecoveryJournalHooks},
    input::{self, RectHooks},
    ipc::Pipe,
    limiter,
    process::{self, Process, Target},
    wifi_scan,
};
use std::{
    collections::BTreeMap, ffi::OsString, os::windows::io::OwnedHandle, path::Path, sync::Arc,
    time::Duration,
};

struct HostPipe(Pipe);
impl HelperPipe for HostPipe {
    fn connected(&self, expected_pid: u32) -> Result<bool> {
        self.0.connected(expected_pid)
    }
    fn send_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.0.send_bytes(bytes)
    }
    fn receive_bytes(&self) -> Result<Option<Vec<u8>>> {
        self.0.receive_bytes()
    }
}

struct HostChild(Process);
impl UserChild for HostChild {
    fn pid(&self) -> u32 {
        self.0.pid
    }
    fn exit_code(&self) -> Result<Option<u32>> {
        self.0.exit_code()
    }
    fn wait(&self, timeout: Duration) -> Result<u32> {
        self.0.wait(timeout)
    }
    fn terminate(&self) -> Result<()> {
        self.0.terminate()
    }
    fn duplicate_into(&self, raw_handle: usize) -> Result<u64> {
        self.0.duplicate_into_raw(raw_handle)
    }
    fn duplicate_resource(&self, raw_handle: usize) -> Result<OwnedHandle> {
        self.0.duplicate_resource(raw_handle)
    }
}

pub struct HostUserLauncher;
impl UserLauncher for HostUserLauncher {
    fn pipe_server(&self, prefix: &str) -> Result<(Box<dyn HelperPipe>, String)> {
        let (pipe, name) = Pipe::server(prefix)?;
        Ok((Box::new(HostPipe(pipe)), name))
    }
    fn pipe_client(&self, name: &str, parent: u32, prefix: &str) -> Result<Box<dyn HelperPipe>> {
        Ok(Box::new(HostPipe(Pipe::client(name, parent, prefix)?)))
    }
    fn spawn_user(&self, program: &Path, args: &[OsString]) -> Result<Box<dyn UserChild>> {
        Ok(Box::new(HostChild(Process::spawn(
            program,
            args,
            None,
            Target::User { elevated: false },
            &BTreeMap::new(),
            true,
        )?)))
    }
    fn spawn_user_piped(&self, program: &Path, args: &[OsString]) -> Result<PipedUserChild> {
        let child = Process::spawn_piped(program, args, Target::User { elevated: false })?;
        Ok(PipedUserChild {
            process: Box::new(HostChild(child.process)),
            stdin: child.stdin,
            stdout: child.stdout,
            stderr: child.stderr,
        })
    }
    fn user_sid(&self) -> Option<String> {
        process::user_sid()
    }
}

struct HostVddControl(rubylight_vdd::Control);
impl VddControl for HostVddControl {
    fn caps(&self) -> u32 {
        self.0.info.caps
    }
    fn query(&self, session_id: u64) -> Result<MonitorStatus> {
        self.0.query(session_id)
    }
    fn set_frame_ring(&self, request: &SetFrameRingRequest) -> Result<()> {
        self.0.set_frame_ring(request)
    }
    fn set_fence_ring(&self, request: &SetFenceRingRequest) -> Result<()> {
        self.0.set_fence_ring(request)
    }
    fn clear_frame_ring(&self, session_id: u64) -> Result<()> {
        self.0.clear_frame_ring(session_id)
    }
    fn clear_frame_ring_generation(&self, session_id: u64, generation: u32) -> Result<()> {
        self.0.clear_frame_ring_generation(session_id, generation)
    }
}

pub(crate) struct HostVddBridge;
impl VddBridge for HostVddBridge {
    fn owned_display(&self, name: &str) -> Result<Option<VddOwned>> {
        Ok(rubylight_vdd::owned_display(name)?.map(|owned| VddOwned {
            session_id: owned.session_id,
            adapter: owned.adapter,
            target: owned.target,
            wudf_pid: owned.wudf_pid,
            registration: owned.registration,
        }))
    }
    fn open_control(&self) -> Result<Box<dyn VddControl>> {
        Ok(Box::new(HostVddControl(rubylight_vdd::Control::open()?)))
    }
}

pub(crate) fn user_launcher() -> Arc<dyn UserLauncher> {
    Arc::new(HostUserLauncher)
}

pub(crate) fn vdd_bridge() -> Arc<dyn VddBridge> {
    Arc::new(HostVddBridge)
}

pub(crate) fn capture_env() -> CaptureEnv {
    CaptureEnv {
        vdd: Some(vdd_bridge()),
        user_launcher: Some(user_launcher()),
    }
}

pub fn recovery_hooks() -> RecoveryHooks {
    RecoveryHooks {
        limiter: limiter::recover,
        audio: audio_route::recover,
        microphone: audio_route::recover_microphone,
    }
}

pub fn install_hooks() {
    audio_route::hooks::set_recovery_hooks(RecoveryJournalHooks {
        audio: display_recovery::audio,
        microphone: display_recovery::microphone,
    });
    limiter::hooks::set_recovery_hook(display_recovery::external);
    input::set_rect_hooks(RectHooks {
        window_rect: capture::window_target_rect,
        display_name_for_id: |id| {
            display::monitors()
                .ok()
                .and_then(|all| all.into_iter().find(|m| m.matches(id)))
                .map(|m| m.display_name)
        },
    });
}

/// The GPU a virtual display renders on: the one the Graphics card setting
/// names, else the one Automatic pins, resolved as `idd-push` capture
/// resolves the GPU it opens on (`capture::render_gpu`).
fn render_adapter(name: &str, pnp_id: &str) -> Result<Option<RenderAdapter>> {
    let gpus = capture::gpus()?;
    Ok(
        capture::render_gpu(&gpus, name, pnp_id)?.map(|gpu| RenderAdapter {
            name: gpu.name.clone(),
            luid: gpu.luid,
        }),
    )
}

pub(crate) fn configured_render_adapter(config: &Config) -> Option<RenderAdapter> {
    display::configured_render_adapter_with(config, render_adapter)
}

/// The console warning a chosen GPU that is gone after a driver restart sets.
const RENDER_GPU_WARNING: &str = "display_render_gpu";

/// Resolves the virtual display's GPU again, from the same setting, after a
/// driver restart or a GPU reset; a chosen GPU that is gone is a warning on
/// the stream's console (`warnings`) until it returns.
pub(crate) fn render_resolver(
    config: &Config,
    warnings: Option<Arc<Warnings>>,
) -> display::RenderResolver {
    let [name, pnp_id] =
        ["adapter_name", "adapter_pnp_id"].map(|key| config.get(key, "").to_string());
    display::RenderResolver::new(move || {
        let resolved = render_adapter(&name, &pnp_id);
        if let Some(warnings) = &warnings {
            match &resolved {
                Ok(_) => warnings.clear(RENDER_GPU_WARNING),
                Err(error) => warnings.set(RENDER_GPU_WARNING, format!("{error:#}")),
            }
        }
        resolved
    })
}

pub(crate) fn enter_streaming_scope() -> StreamingScope {
    StreamingScope::enter_with(|| {
        wifi_scan::enter_streaming_mode().map(|guard| Box::new(guard) as Box<dyn Send>)
    })
}

pub(crate) fn probe_packed(
    config: &Config,
    stream: &Negotiated,
    stop: &dyn Fn() -> bool,
) -> Result<()> {
    butterpollo_win_graphics::codec_probe::packed_with(config, stream, stop, &HostUserLauncher)
}

pub(crate) fn probe_pyrowave(config: &Config, stop: &dyn Fn() -> bool) -> Result<u32> {
    butterpollo_win_graphics::codec_probe::pyrowave_with(config, stop, &HostUserLauncher)
}

pub fn open_capture(
    target: &str,
    kind: &str,
    hdr: bool,
    config: &Config,
    warnings: Arc<Warnings>,
) -> Result<Capture> {
    Capture::open_for_stream_with_env(target, kind, hdr, config, warnings, &capture_env())
}

pub(crate) fn switch_capture(
    capture: &mut Capture,
    target: &str,
    kind: &str,
    hdr: bool,
    config: &Config,
    warnings: Arc<Warnings>,
) -> Option<Result<Capture>> {
    capture.switch_on_device_with_vdd(target, kind, hdr, config, warnings, Some(&HostVddBridge))
}

pub(crate) fn new_encoder_gpu(
    stream: &Negotiated,
    image: &GpuImage,
    config: &Config,
    warnings: &Arc<Warnings>,
) -> Result<Encoder> {
    let sink: Arc<dyn WarningSink> = warnings.clone();
    Encoder::new_gpu_reported(stream, image, config, &sink)
}
