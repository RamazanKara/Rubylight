//! The host side of Rubylight's own virtual display driver (rust/drivers/rubylight-vdisplay):
//! its control IOCTLs, the WUDFHost the frame ring's handles are duplicated into, and the
//! monitors this process created on it.
//!
//! It is the only virtual display driver: every display the host creates comes from it.

use super::*;
use rubylight_vdisplay_proto::control::{
    self as proto, ADD_FLAG_EXACT_ID, AddReply, AddRequest, ClearFrameRingRequest, InfoReply,
    MonitorStatus, RenderAdapterRequest, SessionRequest, SetFenceRingRequest, UpdateModesRequest,
};
use rubylight_vdisplay_proto::modes;
use std::sync::Mutex;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE};

/// An open handle to the driver's control interface, checked for a matching protocol.
pub struct Control {
    handle: HANDLE,
    pub info: InfoReply,
}
// SAFETY: a device handle is a process-wide token, and DeviceIoControl on it is thread-safe;
// the value is closed once, in Drop.
unsafe impl Send for Control {}
// SAFETY: as above; shared use only issues independent synchronous IOCTLs.
unsafe impl Sync for Control {}

impl Control {
    pub fn open() -> Result<Self> {
        let handle = crate::input::open_interface(GUID::from_u128(
            rubylight_vdisplay_proto::INTERFACE_GUID_U128,
        ))
        .context("the Rubylight virtual display driver is not installed")?;
        let mut control = Self {
            handle,
            info: InfoReply {
                magic: 0,
                protocol_version: 0,
                watchdog_timeout_ms: 0,
                max_monitors: 0,
                max_ring_slots: 0,
                caps: 0,
                driver_version: 0,
            },
        };
        let info =
            InfoReply::decode(&control.ioctl(proto::IOCTL_GET_INFO, &[], InfoReply::SIZE)?)
                .map_err(|e| anyhow::anyhow!("Rubylight display driver info: {e}"))?;
        anyhow::ensure!(
            info.compatible(),
            "the Rubylight display driver speaks protocol {}, this host {}; reinstall Rubylight",
            info.protocol_version,
            rubylight_vdisplay_proto::PROTOCOL_VERSION
        );
        control.info = info;
        Ok(control)
    }

    fn ioctl(&self, code: u32, input: &[u8], size: usize) -> Result<Vec<u8>> {
        let mut output = vec![0; size];
        let mut bytes = 0;
        // SAFETY: the owned device handle and both buffers stay valid for this synchronous
        // IOCTL, with their exact lengths.
        unsafe {
            DeviceIoControl(
                self.handle,
                code,
                (!input.is_empty()).then(|| input.as_ptr().cast()),
                input.len() as u32,
                (size != 0).then(|| output.as_mut_ptr().cast()),
                size as u32,
                Some(&mut bytes),
                None,
            )?;
        }
        if bytes as usize > size {
            bail!("invalid Rubylight display driver response length");
        }
        output.truncate(bytes as usize);
        Ok(output)
    }

    /// Create and plug in a monitor; the reply's raw bytes, for [`decode_add`].
    pub fn add_bytes(&self, request: &AddRequest) -> Result<Vec<u8>> {
        self.ioctl(proto::IOCTL_ADD, &request.encode(), AddReply::SIZE)
    }

    pub fn remove(&self, session_id: u64) -> Result<()> {
        self.ioctl(
            proto::IOCTL_REMOVE,
            &SessionRequest { session_id }.encode(),
            0,
        )
        .map(drop)
    }

    /// The lease check: fails with `ERROR_NOT_FOUND` once the monitor is gone.
    pub fn heartbeat(&self, session_id: u64) -> Result<()> {
        self.ioctl(
            proto::IOCTL_HEARTBEAT,
            &SessionRequest { session_id }.encode(),
            0,
        )
        .map(drop)
    }

    pub fn update_modes(&self, session_id: u64, mode: (u32, u32, u32)) -> Result<()> {
        let request = UpdateModesRequest {
            session_id,
            width: mode.0,
            height: mode.1,
            refresh_mhz: mode.2,
        };
        self.ioctl(proto::IOCTL_UPDATE_MODES, &request.encode(), 0)
            .map(drop)
    }

    pub fn set_render_adapter(&self, luid: (u32, i32)) -> Result<()> {
        let request = RenderAdapterRequest {
            luid_low: luid.0,
            luid_high: luid.1,
        };
        self.ioctl(proto::IOCTL_SET_RENDER_ADAPTER, &request.encode(), 0)
            .map(drop)
    }

    /// Hand a fence ring over. On error the driver owns none of the request's handles.
    pub fn set_fence_ring(&self, request: &SetFenceRingRequest) -> Result<()> {
        self.ioctl(proto::IOCTL_SET_FENCE_RING, &request.encode(), 0)
            .map(drop)
    }

    pub fn clear_frame_ring(&self, session_id: u64) -> Result<()> {
        self.ioctl(
            proto::IOCTL_CLEAR_FRAME_RING,
            &SessionRequest { session_id }.encode(),
            0,
        )
        .map(drop)
    }

    /// Clear the monitor's ring only while it is the ring of `generation`
    /// (`CAP_SCOPED_CLEAR`).
    pub fn clear_frame_ring_generation(&self, session_id: u64, generation: u32) -> Result<()> {
        let request = ClearFrameRingRequest {
            session_id,
            generation,
        };
        self.ioctl(
            proto::IOCTL_CLEAR_FRAME_RING_GENERATION,
            &request.encode(),
            0,
        )
        .map(drop)
    }

    pub fn query(&self, session_id: u64) -> Result<MonitorStatus> {
        let reply = self.ioctl(
            proto::IOCTL_QUERY_MONITOR,
            &SessionRequest { session_id }.encode(),
            MonitorStatus::SIZE,
        )?;
        MonitorStatus::decode(&reply)
            .map_err(|e| anyhow::anyhow!("Rubylight display status reply: {e}"))
    }

    /// Forward the driver's queued log lines to the host log.
    pub fn drain_log(&self) {
        let Ok(buf) = self.ioctl(proto::IOCTL_DRAIN_LOG, &[], 16 * 1024) else {
            return;
        };
        for (level, line) in proto::log_lines(&buf) {
            match level {
                proto::LOG_ERROR => tracing::error!(target: "rubylight_vdisplay", "{line}"),
                proto::LOG_WARN => tracing::warn!(target: "rubylight_vdisplay", "{line}"),
                _ => tracing::info!(target: "rubylight_vdisplay", "{line}"),
            }
        }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        // SAFETY: this value owns the device handle and closes it exactly once.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// The ADD request for the display `id` (the host's per-client 64-bit display id) at `mode`
/// (millihertz refresh). The monitor id follows `id`, and must be that one, so Windows sees
/// the same monitor across a host restart: the driver answers busy until a predecessor's
/// monitor with it has departed.
pub(super) fn add_request(id: u64, mode: (u32, u32, u32), peak_nits: u32) -> AddRequest {
    AddRequest {
        session_id: id,
        width: mode.0,
        height: mode.1,
        refresh_mhz: mode.2,
        preferred_monitor_id: modes::preferred_id(id),
        flags: ADD_FLAG_EXACT_ID,
        max_luminance_nits: peak_nits.clamp(400, 2000),
        max_frame_avg_nits: 0,
        min_luminance_millinits: 0,
    }
}

/// The driver's WUDFHost, opened to duplicate handles into it.
pub struct Remote {
    process: HANDLE,
}
// SAFETY: a process handle is a process-wide token, closed once in Drop.
unsafe impl Send for Remote {}

impl Remote {
    pub fn open(pid: u32) -> Result<Self> {
        // SAFETY: plain FFI; the returned handle is owned and closed in Drop.
        let process = unsafe { OpenProcess(PROCESS_DUP_HANDLE, false, pid) }
            .with_context(|| format!("open the display driver's host process {pid}"))?;
        Ok(Self { process })
    }

    /// Duplicate `handle` into the driver's process and return its value there.
    pub fn duplicate(&self, handle: HANDLE) -> Result<u64> {
        let mut remote = HANDLE::default();
        // SAFETY: `handle` is live in this process; `self.process` grants PROCESS_DUP_HANDLE.
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                handle,
                self.process,
                &mut remote,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )?;
        }
        Ok(remote.0 as usize as u64)
    }

    /// Close a handle value this process duplicated into the driver's, after the driver
    /// refused it.
    pub fn close(&self, value: u64) {
        // SAFETY: `value` was duplicated into that process by `duplicate` and the driver did
        // not adopt it, so this is its only close; DUPLICATE_CLOSE_SOURCE closes it there.
        unsafe {
            let _ = DuplicateHandle(
                self.process,
                HANDLE(value as usize as *mut _),
                HANDLE::default(),
                std::ptr::null_mut(),
                0,
                false,
                DUPLICATE_CLOSE_SOURCE,
            );
        }
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        // SAFETY: this value owns the process handle and closes it once.
        unsafe {
            let _ = CloseHandle(self.process);
        }
    }
}

/// A monitor this process created on the Rubylight driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owned {
    pub session_id: u64,
    pub adapter: (u32, i32),
    pub target: u32,
    pub wudf_pid: u32,
}

static OWNED: Mutex<Vec<Owned>> = Mutex::new(Vec::new());

pub(super) fn register(owned: Owned) {
    let mut all = OWNED.lock().unwrap();
    all.retain(|o| o.session_id != owned.session_id);
    all.push(owned);
}

pub(super) fn unregister(session_id: u64) {
    OWNED.lock().unwrap().retain(|o| o.session_id != session_id);
}

fn find(all: &[Owned], adapter: (u32, i32), target: u32) -> Option<Owned> {
    all.iter()
        .find(|o| o.adapter == adapter && o.target == target)
        .copied()
}

/// The Rubylight monitor behind the GDI display `name`, if this process created it.
pub fn owned_display(name: &str) -> Result<Option<Owned>> {
    let Some(monitor) = monitors()?
        .into_iter()
        .find(|m| m.display_name.eq_ignore_ascii_case(name))
    else {
        return Ok(None);
    };
    Ok(find(
        &OWNED.lock().unwrap(),
        (monitor.adapter.LowPart, monitor.adapter.HighPart),
        monitor.target,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_requests_keep_the_display_identity_and_the_stream_mode() {
        let request = add_request(0x6000_0000_0000_002A, (3840, 2160, 479_960), 4000);
        assert_eq!(request.session_id, 0x6000_0000_0000_002A);
        assert_eq!(
            (request.width, request.height, request.refresh_mhz),
            (3840, 2160, 479_960)
        );
        assert_eq!(request.flags, ADD_FLAG_EXACT_ID);
        assert_eq!(request.max_luminance_nits, 2000);
        // Same display, same monitor id: Windows keeps its settings across reconnects.
        assert_eq!(
            request.preferred_monitor_id,
            add_request(0x6000_0000_0000_002A, (1280, 720, 60_000), 1000).preferred_monitor_id
        );
        assert!((1..=modes::MAX_MONITOR_ID).contains(&request.preferred_monitor_id));
        assert!(request.validate().is_ok());
        // The encoded request is the driver's IOCTL input byte for byte.
        assert_eq!(AddRequest::decode(&request.encode()), Ok(request));
        assert_eq!(proto::function_of(proto::IOCTL_ADD), 0xB01);
    }

    #[test]
    fn capture_finds_only_this_processs_monitor_on_the_exact_path() {
        let a = Owned {
            session_id: 1,
            adapter: (0x1234, 0),
            target: 260,
            wudf_pid: 99,
        };
        let b = Owned {
            session_id: 2,
            adapter: (0x1234, 0),
            target: 261,
            wudf_pid: 99,
        };
        let all = [a, b];
        assert_eq!(find(&all, (0x1234, 0), 261), Some(b));
        assert_eq!(find(&all, (0x1235, 0), 260), None, "another adapter");
        assert_eq!(find(&all, (0x1234, 1), 260), None, "another LUID high part");
        assert_eq!(find(&all, (0x1234, 0), 262), None);
    }
}
