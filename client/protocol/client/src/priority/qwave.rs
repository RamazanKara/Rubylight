//! Windows qWAVE control-socket QoS flow (`qwave.dll`), loaded dynamically at run time.

use std::ffi::c_void;
use std::io;
use std::net::SocketAddr;
use std::os::windows::io::AsRawSocket;
use std::sync::OnceLock;

/// `QOSTrafficTypeControl`: DSCP 56, the highest of the traffic types.
const TRAFFIC_TYPE_CONTROL: i32 = 5;
/// `QOS_NON_ADAPTIVE_FLOW`: no network experiments, as ENet asks.
const NON_ADAPTIVE_FLOW: u32 = 0x2;

#[repr(C)]
struct QosVersion {
    major: u16,
    minor: u16,
}

type Handle = *mut c_void;
type CreateHandle = unsafe extern "system" fn(*const QosVersion, *mut Handle) -> i32;
type CloseHandle = unsafe extern "system" fn(Handle) -> i32;
type AddSocketToFlow =
    unsafe extern "system" fn(Handle, usize, *const c_void, i32, u32, *mut u32) -> i32;
type RemoveSocketFromFlow = unsafe extern "system" fn(Handle, usize, u32, u32) -> i32;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

struct Qwave {
    create: CreateHandle,
    close: CloseHandle,
    add: AddSocketToFlow,
    remove: RemoveSocketFromFlow,
}

/// qwave.dll's functions, loaded once and kept for the process.
fn qwave() -> Result<&'static Qwave, String> {
    static QWAVE: OnceLock<Result<Qwave, String>> = OnceLock::new();
    QWAVE
        .get_or_init(|| {
            let name: Vec<u16> = "qwave.dll\0".encode_utf16().collect();
            // SAFETY: a NUL-terminated name; the module is never freed.
            let module = unsafe { LoadLibraryW(name.as_ptr()) };
            if module.is_null() {
                return Err(format!("qwave.dll: {}", io::Error::last_os_error()));
            }
            let find = |symbol: &[u8]| {
                // SAFETY: a NUL-terminated name in a loaded module.
                let address = unsafe { GetProcAddress(module, symbol.as_ptr()) };
                (!address.is_null()).then_some(address).ok_or_else(|| {
                    let name = String::from_utf8_lossy(&symbol[..symbol.len() - 1]);
                    format!("qwave.dll has no {name}")
                })
            };
            // SAFETY: the documented signatures of these qWAVE exports.
            unsafe {
                Ok(Qwave {
                    create: std::mem::transmute::<*mut c_void, CreateHandle>(find(
                        b"QOSCreateHandle\0",
                    )?),
                    close: std::mem::transmute::<*mut c_void, CloseHandle>(find(
                        b"QOSCloseHandle\0",
                    )?),
                    add: std::mem::transmute::<*mut c_void, AddSocketToFlow>(find(
                        b"QOSAddSocketToFlow\0",
                    )?),
                    remove: std::mem::transmute::<*mut c_void, RemoveSocketFromFlow>(find(
                        b"QOSRemoveSocketFromFlow\0",
                    )?),
                })
            }
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// One qWAVE handle and the control socket's flow on it.
pub struct ControlFlow {
    qwave: &'static Qwave,
    handle: Handle,
    /// The socket that joined (or tried to join) the flow, and the flow's id when it did.
    joined: Option<(usize, Option<u32>)>,
}

// SAFETY: the handle is used from one thread at a time (the control thread owns it).
unsafe impl Send for ControlFlow {}

impl ControlFlow {
    /// `QOSCreateHandle`; Err says why the control socket goes unmarked.
    pub fn open() -> io::Result<Self> {
        let qwave = qwave().map_err(io::Error::other)?;
        let mut handle: Handle = std::ptr::null_mut();
        let version = QosVersion { major: 1, minor: 0 };
        // SAFETY: valid pointers to a version and a handle slot.
        if unsafe { (qwave.create)(&version, &mut handle) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            qwave,
            handle,
            joined: None,
        })
    }

    /// Adds `socket` to the flow toward `to` before its first send; Ok(false) when this
    /// socket already joined or tried to. A refusal is returned once and not retried, as ENet
    /// does: the socket then sends unmarked.
    pub fn add(&mut self, socket: &impl AsRawSocket, to: SocketAddr) -> io::Result<bool> {
        let raw = socket.as_raw_socket() as usize;
        if self.joined.is_some_and(|(joined, _)| joined == raw) {
            return Ok(false);
        }
        self.remove();
        let destination = socket2::SockAddr::from(to);
        let mut flow = 0u32;
        // SAFETY: a live socket, a valid address of its family and a zeroed flow id.
        let added = unsafe {
            (self.qwave.add)(
                self.handle,
                raw,
                destination.as_ptr().cast(),
                TRAFFIC_TYPE_CONTROL,
                NON_ADAPTIVE_FLOW,
                &mut flow,
            )
        } != 0;
        let error = (!added).then(io::Error::last_os_error);
        self.joined = Some((raw, added.then_some(flow)));
        error.map_or(Ok(true), Err)
    }

    /// Takes the socket that joined out of the flow, before it is closed or replaced.
    pub fn remove(&mut self) {
        if let Some((socket, Some(flow))) = self.joined.take() {
            // SAFETY: the socket joined this flow on this handle and is still open.
            unsafe { (self.qwave.remove)(self.handle, socket, flow, 0) };
        }
    }

    /// The flow's id while a socket is in it.
    pub fn flow_id(&self) -> Option<u32> {
        self.joined.and_then(|(_, flow)| flow)
    }
}

impl Drop for ControlFlow {
    fn drop(&mut self) {
        self.remove();
        // SAFETY: the handle from QOSCreateHandle, closed once.
        unsafe { (self.qwave.close)(self.handle) };
    }
}
