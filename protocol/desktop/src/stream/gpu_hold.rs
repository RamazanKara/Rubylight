//! High-performance GPU power while streaming (Linux, `high_performance_gpu_power`, off by default):
//! while a stream runs, each AMD GPU that drives a connected display is held at its peak stable
//! power state, so its clocks do not drop between the bursts of decode, render and composition work
//! and a frame that arrived on time is not shown late. AMD on Linux exposes this per process: an
//! amdgpu context asks for the stable pstate, and the request lasts while that context is open. The
//! kernel restores the previous level when the context is freed, including when the process dies.
//! A level set by hand (anything other than `auto`) is left alone, and a GPU that another process
//! holds in a stable pstate (EBUSY) is skipped. The sysfs reading and the ABI constants are tested
//! everywhere; the requests are Linux's.
//!
//! Ported from Nonary's moonlight-qt (GPL-3.0, commit 19758e5e6), app/streaming/gpuperformancehold.{h,cpp}.

use std::path::{Path, PathBuf};

const DRM_CLASS: &str = "/sys/class/drm";
const DEV_DRI: &str = "/dev/dri";
const LEVEL_FILE: &str = "power_dpm_force_performance_level";
const AUTO: &str = "auto";

/// One render node of amdgpu whose device has a connected connector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The render node's name in sysfs, `renderD128`.
    pub name: String,
    /// The node to open, `/dev/dri/renderD128`.
    pub node: PathBuf,
    /// The device folder, resolved.
    pub device: PathBuf,
    /// `power_dpm_force_performance_level`, trimmed; empty when it cannot be read.
    pub level: String,
}

impl Candidate {
    /// Whether the level is `auto`, the only one the hold takes over; any other was set by hand.
    pub fn level_is_auto(&self) -> bool {
        self.level == AUTO
    }
}

/// The first line of a sysfs file, trimmed; empty when the file cannot be read.
fn read_line(path: &Path) -> String {
    std::fs::read_to_string(path).ok().and_then(|text| text.lines().next().map(|line| line.trim().to_owned())).unwrap_or_default()
}

/// The names of the entries of `dir` that start with `prefix`, sorted by name.
fn entries(dir: &Path, prefix: &str) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = read
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(prefix))
        .collect();
    names.sort();
    names
}

/// Whether one of the device's cards (`cardN`, not `cardN-connector`) has a connector that reads `connected`.
fn has_connected_connector(device: &Path) -> bool {
    let drm = device.join("drm");
    for card in entries(&drm, "card") {
        if card.contains('-') {
            continue;
        }
        let card_dir = drm.join(&card);
        for connector in entries(&card_dir, &format!("{card}-")) {
            if read_line(&card_dir.join(&connector).join("status")) == "connected" {
                return true;
            }
        }
    }
    false
}

/// Whether the device is bound to the amdgpu driver (its `driver` link resolves to `amdgpu`).
fn is_amdgpu(device: &Path) -> bool {
    std::fs::canonicalize(device.join("driver")).ok().and_then(|driver| driver.file_name().map(|name| name == "amdgpu")).unwrap_or(false)
}

/// The amdgpu render nodes under `sys_class_drm` (`/sys/class/drm`) whose device has a connected
/// connector, by name. Each carries its level; whether it is held is the caller's to judge.
pub fn display_gpus(sys_class_drm: &Path) -> Vec<Candidate> {
    let mut found = Vec::new();
    for name in entries(sys_class_drm, "renderD") {
        let Ok(device) = std::fs::canonicalize(sys_class_drm.join(&name).join("device")) else { continue };
        if !is_amdgpu(&device) || !has_connected_connector(&device) {
            continue;
        }
        let level = read_line(&device.join(LEVEL_FILE));
        found.push(Candidate { node: Path::new(DEV_DRI).join(&name), name, device, level });
    }
    found
}

// The amdgpu UAPI (`drm/amdgpu_drm.h`), as the kernel defines it.
pub const DRM_COMMAND_BASE: u32 = 0x40;
pub const DRM_AMDGPU_CTX: u32 = 0x02;
pub const AMDGPU_CTX_OP_ALLOC_CTX: u32 = 1;
pub const AMDGPU_CTX_OP_FREE_CTX: u32 = 2;
pub const AMDGPU_CTX_OP_GET_STABLE_PSTATE: u32 = 5;
pub const AMDGPU_CTX_OP_SET_STABLE_PSTATE: u32 = 6;
pub const AMDGPU_CTX_PRIORITY_NORMAL: i32 = 0;
pub const AMDGPU_CTX_STABLE_PSTATE_PEAK: u32 = 4;
pub const AMDGPU_CTX_STABLE_PSTATE_FLAGS_MASK: u32 = 0xf;

/// `union drm_amdgpu_ctx`: the request's `in` fields, and the answer, which the kernel writes over
/// the same 16 bytes. The answer's first word (`out.alloc.ctx_id` or `out.pstate.flags`) is the
/// request's `op` field, so it is read through `out_word`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrmAmdgpuCtx {
    pub op: u32,
    pub flags: u32,
    pub ctx_id: u32,
    pub priority: i32,
}

impl DrmAmdgpuCtx {
    /// `out.alloc.ctx_id` after an alloc, or `out.pstate.flags` after a get.
    pub fn out_word(&self) -> u32 {
        self.op
    }
}

/// `_IOWR('d', nr, size)` in the generic Linux `_IOC` encoding: read and write (3) in the top
/// two bits, the size of the argument, the type 'd' and the number.
const fn drm_iowr(nr: u32, size: u32) -> u32 {
    const IOC_READ_WRITE: u32 = 3;
    const IOC_TYPE_DRM: u32 = b'd' as u32;
    (IOC_READ_WRITE << 30) | (size << 16) | (IOC_TYPE_DRM << 8) | nr
}

/// `DRM_IOCTL_AMDGPU_CTX`, `DRM_IOWR(DRM_COMMAND_BASE + DRM_AMDGPU_CTX, union drm_amdgpu_ctx)`.
pub const DRM_IOCTL_AMDGPU_CTX: u32 = drm_iowr(DRM_COMMAND_BASE + DRM_AMDGPU_CTX, std::mem::size_of::<DrmAmdgpuCtx>() as u32);

/// The GPUs one stream holds: each with its render node open and the context that asked for the
/// peak pstate. Dropping it frees the contexts, which restores the levels, and closes the nodes.
#[cfg(target_os = "linux")]
pub struct GpuHold {
    holds: Vec<Hold>,
}

#[cfg(target_os = "linux")]
struct Hold {
    name: String,
    file: std::fs::File,
    context: u32,
}

/// One ioctl on the render node, retried while interrupted; the errno on failure.
#[cfg(target_os = "linux")]
fn context_op(fd: std::os::unix::io::RawFd, args: &mut DrmAmdgpuCtx) -> Result<(), i32> {
    let args_ptr = args as *mut DrmAmdgpuCtx;
    loop {
        // SAFETY: `args_ptr` points at a live 16-byte `union drm_amdgpu_ctx`, which the kernel reads and
        // writes for this call only, and `fd` is an open render node for the whole call.
        let result = unsafe { libc::ioctl(fd, DRM_IOCTL_AMDGPU_CTX as _, args_ptr) };
        if result >= 0 {
            return Ok(());
        }
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO);
        if errno != libc::EINTR && errno != libc::EAGAIN {
            return Err(errno);
        }
    }
}

/// Holds each display GPU whose level is `auto` at its peak stable pstate, for the stream's lifetime.
/// A GPU that cannot be held is logged and left out; the stream goes on without it.
#[cfg(target_os = "linux")]
pub fn acquire() -> GpuHold {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;

    let mut holds = Vec::new();
    for gpu in display_gpus(Path::new(DRM_CLASS)) {
        let name = &gpu.name;
        if !gpu.level_is_auto() {
            eprintln!("nightjar: GPU performance hold: {name} left at externally set level '{}'", gpu.level);
            continue;
        }
        let file = match std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_CLOEXEC).open(&gpu.node) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("nightjar: GPU performance hold: cannot open {name}: {error}");
                continue;
            }
        };
        let fd = file.as_raw_fd();

        let mut args = DrmAmdgpuCtx { op: AMDGPU_CTX_OP_ALLOC_CTX, priority: AMDGPU_CTX_PRIORITY_NORMAL, ..Default::default() };
        let mut result = context_op(fd, &mut args);
        let context = args.out_word();
        if result.is_ok() {
            args = DrmAmdgpuCtx { op: AMDGPU_CTX_OP_SET_STABLE_PSTATE, ctx_id: context, flags: AMDGPU_CTX_STABLE_PSTATE_PEAK, ..Default::default() };
            result = context_op(fd, &mut args);
        }
        if result.is_ok() {
            args = DrmAmdgpuCtx { op: AMDGPU_CTX_OP_GET_STABLE_PSTATE, ctx_id: context, ..Default::default() };
            result = context_op(fd, &mut args);
            if result.is_ok() && args.out_word() & AMDGPU_CTX_STABLE_PSTATE_FLAGS_MASK != AMDGPU_CTX_STABLE_PSTATE_PEAK {
                result = Err(libc::EPERM);
            }
        }
        if let Err(errno) = result {
            // EBUSY: another process holds a stable pstate. EINVAL: the kernel predates the request (5.19).
            // Closing the node frees the context.
            let why = std::io::Error::from_raw_os_error(errno);
            eprintln!("nightjar: GPU performance hold: {name} request failed: {why}");
            continue;
        }

        let level = read_line(&gpu.device.join(LEVEL_FILE));
        eprintln!("nightjar: GPU performance hold: {name} pinned at '{level}' for this stream");
        holds.push(Hold { name: name.clone(), file, context });
    }
    GpuHold { holds }
}

#[cfg(target_os = "linux")]
impl Drop for GpuHold {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;

        for hold in &self.holds {
            // Freeing the context restores the level that was active when it was created. A level written
            // through sysfs meanwhile takes ownership and is not overridden. The node closes after this.
            let mut args = DrmAmdgpuCtx { op: AMDGPU_CTX_OP_FREE_CTX, ctx_id: hold.context, ..Default::default() };
            let _ = context_op(hold.file.as_raw_fd(), &mut args);
            eprintln!("nightjar: GPU performance hold: {} released", hold.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A symlink to a folder, where the platform allows one (Windows needs developer mode for it).
    fn link_dir(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(target, link).is_ok();
        made
    }

    /// A fake `/sys/class/drm` with four render nodes: an amdgpu device with a connected connector
    /// (level `auto`), a device with no connected connector, a non-amdgpu device, and an amdgpu
    /// device with a connected connector whose level was set by hand.
    fn fake_sysfs(root: &Path) -> Option<PathBuf> {
        let class = root.join("class");
        let bus = root.join("bus");
        let drivers = root.join("drivers");
        let device = |name: &str, driver: &str, status: &str, level: Option<&str>| -> Option<()> {
            let dev = bus.join(name);
            std::fs::create_dir_all(dev.join("drm").join("card1").join("card1-DP-1")).ok()?;
            std::fs::create_dir_all(drivers.join(driver)).ok()?;
            std::fs::write(dev.join("drm").join("card1").join("card1-DP-1").join("status"), format!("{status}\n")).ok()?;
            if let Some(level) = level {
                std::fs::write(dev.join(LEVEL_FILE), format!("{level}\n")).ok()?;
            }
            link_dir(&drivers.join(driver), &dev.join("driver")).then_some(())
        };
        let node = |name: &str, bus_name: &str| -> Option<()> {
            let folder = class.join(name);
            std::fs::create_dir_all(&folder).ok()?;
            link_dir(&bus.join(bus_name), &folder.join("device")).then_some(())
        };
        device("0000:03:00.0", "amdgpu", "connected", Some("auto"))?;
        device("0000:04:00.0", "amdgpu", "disconnected", Some("auto"))?;
        device("0000:05:00.0", "nouveau", "connected", Some("auto"))?;
        device("0000:06:00.0", "amdgpu", "connected", Some("manual"))?;
        node("renderD128", "0000:03:00.0")?;
        node("renderD129", "0000:04:00.0")?;
        node("renderD130", "0000:05:00.0")?;
        node("renderD131", "0000:06:00.0")?;
        Some(class)
    }

    #[test]
    fn display_gpus_are_the_amdgpu_nodes_with_a_connected_connector() {
        let root = std::env::temp_dir().join(format!("nj-gpu-hold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let Some(class) = fake_sysfs(&root) else {
            eprintln!("nightjar: symlinks are not allowed here; the sysfs part of the GPU hold test is skipped");
            let _ = std::fs::remove_dir_all(&root);
            return;
        };
        let found = display_gpus(&class);
        let names: Vec<&str> = found.iter().map(|gpu| gpu.name.as_str()).collect();
        assert_eq!(names, ["renderD128", "renderD131"], "no connector, non-amdgpu and missing ones are not taken");
        assert_eq!(found[0].node, Path::new("/dev/dri/renderD128"));
        assert!(found[0].level_is_auto(), "auto is held");
        assert_eq!(found[1].level, "manual");
        assert!(!found[1].level_is_auto(), "a level set by hand is skipped");
        assert!(display_gpus(&root.join("missing")).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_ioctl_number_and_the_struct_match_the_kernel_abi() {
        assert_eq!(DRM_IOCTL_AMDGPU_CTX, 0xC010_6442);
        assert_eq!(std::mem::size_of::<DrmAmdgpuCtx>(), 16);
        assert_eq!(drm_iowr(0x40 + 0x02, 16), 0xC010_6442);
    }
}
