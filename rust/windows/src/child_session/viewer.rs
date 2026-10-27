//! The RDP viewer that keeps the child session attached: `butterpollo.exe
//! --child-session-viewer`, which the host starts as the signed-in user in
//! the console session. The host's [`ViewerCommand`]s arrive on standard
//! input and the viewer's [`ViewerStatus`] lines leave on standard output:
//! anonymous pipes only the host holds, so no other process can send it a
//! command or read the password.
//!
//! The Microsoft RDP client control (MsTscAx) runs in an ATL ActiveX window
//! and is driven through IDispatch, as Annex's `LauncherApplication` drives
//! it (`src/LauncherApplication.cs`, `src/LauncherPolicy.cs`, MIT, see the
//! module above): loopback, the user's own account, CredSSP, the child-session
//! flag and `KeyboardHookMode = 2`, so Windows-key shortcuts stay on the
//! user's own desktop. On top of Annex, the RDP port setup recorded,
//! `127.0.0.1` rather than `localhost`, and a check right before every connect
//! that Windows' own RDP server, and nothing else, listens on that port over
//! TCP or UDP ([`check_rdp_server`]): with `AuthenticationLevel = 0` the client
//! does not authenticate the server itself. The client's own automatic
//! reconnect is off, so no connect skips that check.

use crate::text::to_wide;
use anyhow::{Context, Result, bail};
use butterpollo_core::child_session::{
    Confirmation, DisplaySettings, LineReader, PasswordStore, RdpState, Secret, Viewer,
    ViewerAction, ViewerCommand, ViewerStatus, check_rdp_server,
};
use std::{
    ffi::c_void,
    fs::File,
    io::{ErrorKind, Read, Write},
    mem::ManuallyDrop,
    os::windows::io::{AsRawHandle, FromRawHandle},
    path::PathBuf,
    sync::mpsc,
    time::Instant,
};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, VARIANT_FALSE, VARIANT_TRUE, WPARAM},
        System::{
            Com::{
                CoTaskMemFree, DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET,
                DISPATCH_PROPERTYPUT, DISPPARAMS, IDispatch,
            },
            LibraryLoader::{
                GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
            },
            Ole::{DISPID_PROPERTYPUT, OleInitialize},
            SystemInformation::{ComputerNameNetBIOS, GetComputerNameExW},
            Variant::{
                VARENUM, VARIANT, VARIANT_0_0_0, VT_BOOL, VT_BSTR, VT_DISPATCH, VT_I4, VT_UI4,
                VariantChangeType, VariantClear,
            },
            WindowsProgramming::GetUserNameW,
        },
        UI::{
            Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
            WindowsAndMessaging::*,
        },
    },
    core::{
        BSTR, GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, PCWSTR, PWSTR, interface, s, w,
    },
};
use zeroize::Zeroizing;

/// Microsoft RDP Client Control (MsTscAx), the CLSID Annex hosts.
const RDP_CONTROL: &str = "{8B918B82-7985-4C24-89DF-C33AD2BBFBCD}";
/// The viewer polls the RDP client this often (Annex: 2 s, plus a 250 ms watch).
const POLL_MS: u32 = 500;
const WAKE: u32 = WM_APP + 1;
const CLASS: PCWSTR = w!("RubylightChildSessionViewer");
/// The longest command the viewer reads; a start with any Windows password
/// fits.
const COMMAND_CAP: usize = 16 * 1024;

/// `IMsRdpExtendedSettings` (mstsax.idl); only its first method, the
/// property put, is declared because only that one is called.
#[interface("302D8188-0052-4807-806A-362B628F9AC5")]
unsafe trait IMsRdpExtendedSettings: IUnknown {
    fn put_property(&self, name: *const u16, value: *const VARIANT) -> HRESULT;
}

/// A VARIANT that is cleared on drop.
struct Value(VARIANT);
impl Drop for Value {
    fn drop(&mut self) {
        // SAFETY: the VARIANT was built by `Value::new` or filled by a COM call, so it owns what
        // its type names.
        let _ = unsafe { VariantClear(&mut self.0) };
    }
}
impl Value {
    fn new(vt: VARENUM, fill: impl FnOnce(&mut VARIANT_0_0_0)) -> Self {
        let mut value = VARIANT::default();
        // SAFETY: a zeroed VARIANT is VT_EMPTY; the type tag and its matching member are set
        // together.
        unsafe {
            let inner = &mut *value.Anonymous.Anonymous;
            inner.vt = vt;
            fill(&mut inner.Anonymous);
        }
        Self(value)
    }
    fn int(value: i32) -> Self {
        Self::new(VT_I4, |v| v.lVal = value)
    }
    fn uint(value: u32) -> Self {
        Self::new(VT_UI4, |v| v.ulVal = value)
    }
    fn yes() -> Self {
        Self::new(VT_BOOL, |v| v.boolVal = VARIANT_TRUE)
    }
    fn text(value: &str) -> Self {
        Self::new(VT_BSTR, |v| {
            v.bstrVal = ManuallyDrop::new(BSTR::from(value))
        })
    }
    fn no() -> Self {
        Self::new(VT_BOOL, |v| v.boolVal = VARIANT_FALSE)
    }
    /// A BSTR whose characters are wiped before it is freed. The UTF-16 it is
    /// made from is wiped too: [`wide_secret`] never grows it, so no copy is
    /// left behind in a freed buffer.
    fn secret(value: &Secret) -> Secret2 {
        let wide = wide_secret(value.expose());
        Secret2(Self::new(VT_BSTR, |v| {
            v.bstrVal = ManuallyDrop::new(BSTR::from_wide(&wide))
        }))
    }
    fn as_i32(&self) -> Result<i32> {
        let mut converted = Value(VARIANT::default());
        // SAFETY: both VARIANTs are live; the conversion writes `converted` only.
        unsafe { VariantChangeType(&mut converted.0, &self.0, Default::default(), VT_I4) }?;
        // SAFETY: VariantChangeType succeeded, so `converted` is VT_I4.
        Ok(unsafe { converted.0.Anonymous.Anonymous.Anonymous.lVal })
    }
    fn into_dispatch(self) -> Result<IDispatch> {
        // SAFETY: the type tag is read before the member it names.
        unsafe {
            let inner = &self.0.Anonymous.Anonymous;
            if inner.vt != VT_DISPATCH {
                bail!("the RDP client returned no object");
            }
            (*inner.Anonymous.pdispVal)
                .clone()
                .context("the RDP client returned no object")
        }
    }
}
/// Configure the client, check the server, connect: in that order, so the check is the last
/// step before the password leaves, and a refused check never connects. The notices
/// `configure` returned.
fn connect_checked(
    configure: impl FnOnce() -> Result<Vec<String>>,
    check: impl FnOnce() -> Result<()>,
    connect: impl FnOnce() -> Result<()>,
) -> Result<Vec<String>> {
    let notices = configure()?;
    check()?;
    connect()?;
    Ok(notices)
}
/// `text` as UTF-16 in a buffer sized up front, wiped on drop.
fn wide_secret(text: &str) -> Zeroizing<Vec<u16>> {
    let mut wide = Zeroizing::new(Vec::with_capacity(text.encode_utf16().count()));
    wide.extend(text.encode_utf16());
    wide
}
/// The password VARIANT: its BSTR is zeroed before [`Value`] frees it.
struct Secret2(Value);
impl Drop for Secret2 {
    fn drop(&mut self) {
        // SAFETY: the VARIANT is the VT_BSTR `Value::text` made; its characters are this
        // process's own allocation, overwritten in place before VariantClear frees it.
        unsafe {
            let bstr = &*self.0.0.Anonymous.Anonymous.Anonymous.bstrVal;
            let length = bstr.len();
            std::ptr::write_bytes(bstr.as_ptr().cast_mut(), 0, length);
        }
    }
}

/// IDispatch late binding, as Annex's `dynamic`.
fn invoke(
    object: &IDispatch,
    name: &str,
    flags: DISPATCH_FLAGS,
    argument: Option<&Value>,
) -> Result<Value> {
    let wide = to_wide(name);
    let mut id = 0;
    // SAFETY: `wide` is NUL-terminated and outlives the call; one name, one ID.
    unsafe { object.GetIDsOfNames(&GUID::zeroed(), &PCWSTR(wide.as_ptr()), 1, 0x0400, &mut id) }
        .with_context(|| format!("the RDP client has no {name}"))?;
    let mut put = DISPID_PROPERTYPUT;
    let parameters = match argument {
        // The control only reads its arguments.
        Some(value) => DISPPARAMS {
            rgvarg: (&value.0 as *const VARIANT).cast_mut(),
            rgdispidNamedArgs: &mut put,
            cArgs: 1,
            cNamedArgs: u32::from(flags == DISPATCH_PROPERTYPUT),
        },
        None => DISPPARAMS::default(),
    };
    let mut result = Value(VARIANT::default());
    // SAFETY: the parameters point at live locals for the whole call and `result` is a VARIANT
    // the call fills.
    unsafe {
        object.Invoke(
            id,
            &GUID::zeroed(),
            0x0400,
            flags,
            &parameters,
            Some(&mut result.0),
            None,
            None,
        )
    }
    .with_context(|| format!("the RDP client refused {name}"))?;
    Ok(result)
}
fn get(object: &IDispatch, name: &str) -> Result<Value> {
    invoke(object, name, DISPATCH_PROPERTYGET, None)
}
fn put(object: &IDispatch, name: &str, value: &Value) -> Result<()> {
    invoke(object, name, DISPATCH_PROPERTYPUT, Some(value)).map(drop)
}
fn call(object: &IDispatch, name: &str) -> Result<()> {
    invoke(object, name, DISPATCH_METHOD, None).map(drop)
}
/// `object.group.name = value`, e.g. `AdvancedSettings7.EnableCredSspSupport`.
fn put_in(object: &IDispatch, group: &str, name: &str, value: &Value) -> Result<()> {
    put(&get(object, group)?.into_dispatch()?, name, value)
        .with_context(|| format!("setting {group}.{name}"))
}

/// The RDP client control in its ATL window.
pub(crate) struct Rdp {
    control: IDispatch,
    extended: IMsRdpExtendedSettings,
}
impl Rdp {
    /// Host the control in an `AtlAxWin` window under `parent`. ATL 3.0 (`atl.dll`) ships with
    /// every Windows version; it is loaded from System32 only.
    pub(crate) fn create(parent: HWND) -> Result<(Self, HWND)> {
        type Init = unsafe extern "system" fn() -> windows::core::BOOL;
        type GetControl = unsafe extern "system" fn(HWND, *mut *mut c_void) -> HRESULT;
        // SAFETY: atl.dll comes from System32 and stays loaded for the process; the exports are
        // called with the signatures atlbase.h gives them; the window is a child of `parent` on
        // this thread, and AtlAxGetControl returns an owned reference.
        unsafe {
            let atl = LoadLibraryExW(w!("atl.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
                .context("loading atl.dll, which hosts the RDP client")?;
            let init: Init = std::mem::transmute(
                GetProcAddress(atl, s!("AtlAxWinInit")).context("atl.dll has no AtlAxWinInit")?,
            );
            let get_control: GetControl = std::mem::transmute(
                GetProcAddress(atl, s!("AtlAxGetControl"))
                    .context("atl.dll has no AtlAxGetControl")?,
            );
            if !init().as_bool() {
                bail!("ATL could not register its ActiveX window");
            }
            let clsid = to_wide(RDP_CONTROL);
            let module = GetModuleHandleW(None)?;
            let window = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("AtlAxWin"),
                PCWSTR(clsid.as_ptr()),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                1280,
                720,
                Some(parent),
                None,
                Some(HINSTANCE(module.0)),
                None,
            )
            .context("creating the RDP client window")?;
            let mut raw = std::ptr::null_mut();
            get_control(window, &mut raw)
                .ok()
                .context("the RDP client control did not start (mstscax.dll)")?;
            let unknown = IUnknown::from_raw(raw);
            Ok((
                Self {
                    control: unknown.cast().context("the RDP client has no IDispatch")?,
                    extended: unknown
                        .cast()
                        .context("the RDP client has no extended settings")?,
                },
                window,
            ))
        }
    }
    fn extended(&self, name: &str, value: &Value) -> Result<()> {
        let wide = BSTR::from(name);
        // SAFETY: the BSTR and the VARIANT outlive the call, which only reads them.
        unsafe { self.extended.put_property(wide.as_ptr(), &value.0) }
            .ok()
            .with_context(|| format!("setting {name}"))
    }
    /// The `Connected` property: 0 disconnected, 1 connected, else connecting.
    pub(crate) fn state(&self) -> Result<RdpState> {
        Ok(RdpState::from_connected(
            get(&self.control, "Connected")?.as_i32()?,
        ))
    }
    fn disconnect_reason(&self) -> Option<i32> {
        get(&self.control, "ExtendedDisconnectReason")
            .and_then(|value| value.as_i32())
            .ok()
    }
    /// Everything Annex's `Connect` sets, short of the connect call itself. Settings the
    /// session cannot work without fail; the others come back as notices.
    pub(crate) fn configure(
        &self,
        account: &str,
        password: &Secret,
        display: DisplaySettings,
        rdp_port: u16,
    ) -> Result<Vec<String>> {
        let control = &self.control;
        // The IPv4 loopback address [`check_rdp_server`] checks, never a name to resolve.
        put(control, "Server", &Value::text("127.0.0.1"))?;
        // The port setup recorded and keeps closed to the network: the client's default would
        // reach whatever listens on 3389 instead.
        put_in(
            control,
            "AdvancedSettings2",
            "RDPPort",
            &Value::int(i32::from(rdp_port)),
        )?;
        put(control, "UserName", &Value::text(account))?;
        put_in(
            control,
            "AdvancedSettings2",
            "ClearTextPassword",
            &Value::secret(password).0,
        )?;
        put(control, "DesktopWidth", &Value::int(display.width as i32))?;
        put(control, "DesktopHeight", &Value::int(display.height as i32))?;
        // Without CredSSP the loopback sign-in has no network-level authentication.
        put_in(
            control,
            "AdvancedSettings7",
            "EnableCredSspSupport",
            &Value::yes(),
        )?;
        // 0: connect without authenticating the server. The loopback server's certificate is
        // self-signed and has no Kerberos, so 1 always refuses, and 2 opens a modal warning
        // behind the minimized, never-activated window that stalls the viewer. The server is
        // checked instead, before every connect ([`check_rdp_server`]).
        put_in(
            control,
            "AdvancedSettings5",
            "AuthenticationLevel",
            &Value::int(0),
        )?;
        // The client's own reconnect would skip [`check_rdp_server`]: every reconnect goes
        // through the viewer's connect instead.
        put_in(
            control,
            "AdvancedSettings2",
            "EnableAutoReconnect",
            &Value::no(),
        )?;
        self.extended("ConnectToChildSession", &Value::yes())?;
        let mut notices = Vec::new();
        let (desktop, device) = display.rdp_scale_factors();
        let optional = [
            put_in(control, "AdvancedSettings2", "SmartSizing", &Value::yes()),
            put(control, "ColorDepth", &Value::int(32)),
            self.extended("DesktopScaleFactor", &Value::uint(desktop)),
            self.extended("DeviceScaleFactor", &Value::uint(device)),
            // 2: Windows-key shortcuts go to the child session only in full screen, which the
            // viewer never uses, so Win+Shift+S, Win+D etc. stay on the user's own desktop.
            put_in(
                control,
                "SecuredSettings2",
                "KeyboardHookMode",
                &Value::int(2),
            )
            .context("Windows-key shortcuts may reach the child session"),
        ];
        for result in optional {
            if let Err(error) = result {
                notices.push(format!("{error:#}"));
            }
        }
        Ok(notices)
    }
    fn connect(&self) -> Result<()> {
        call(&self.control, "Connect")
    }
    fn disconnect(&self) -> Result<()> {
        call(&self.control, "Disconnect")
    }
}

/// What [`App`] does to the RDP client and this PC's RDP server: [`Rdp`] and
/// [`check_rdp_server`] here, a fake in the tests, so the order of the check
/// and the connect is tested without a real client.
trait Client {
    fn state(&self) -> Result<RdpState>;
    fn disconnect_reason(&self) -> Option<i32>;
    fn configure(
        &self,
        account: &str,
        password: &Secret,
        display: DisplaySettings,
        rdp_port: u16,
    ) -> Result<Vec<String>>;
    /// That Windows' own RDP server, and nothing else, listens on `rdp_port`.
    fn check_server(&self, rdp_port: u16) -> Result<()>;
    fn connect(&self) -> Result<()>;
    fn disconnect(&self) -> Result<()>;
}
impl Client for Rdp {
    fn state(&self) -> Result<RdpState> {
        Rdp::state(self)
    }
    fn disconnect_reason(&self) -> Option<i32> {
        Rdp::disconnect_reason(self)
    }
    fn configure(
        &self,
        account: &str,
        password: &Secret,
        display: DisplaySettings,
        rdp_port: u16,
    ) -> Result<Vec<String>> {
        Rdp::configure(self, account, password, display, rdp_port)
    }
    fn check_server(&self, rdp_port: u16) -> Result<()> {
        check_rdp_server(rdp_port, &super::rdp_server()?)
    }
    fn connect(&self) -> Result<()> {
        Rdp::connect(self)
    }
    fn disconnect(&self) -> Result<()> {
        Rdp::disconnect(self)
    }
}

unsafe extern "system" fn window(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: called by the system on this window's thread with its own arguments.
    unsafe {
        match message {
            WM_SIZE => {
                let mut client = RECT::default();
                if GetClientRect(hwnd, &mut client).is_ok()
                    && let Ok(control) = GetWindow(hwnd, GW_CHILD)
                {
                    let _ = MoveWindow(control, 0, 0, client.right, client.bottom, true);
                }
                LRESULT(0)
            }
            // Closing the window would drop the child session's display: it minimizes, and the
            // console's Stop detaches.
            WM_CLOSE => {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

fn create_window(show: bool) -> Result<HWND> {
    // SAFETY: the class and window belong to this thread, which runs their message loop.
    unsafe {
        let module = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window),
            hInstance: HINSTANCE(module.0),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0
            && windows::Win32::Foundation::GetLastError()
                != windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS
        {
            bail!("cannot register the viewer window");
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("Rubylight background desktop"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1280,
            760,
            None,
            None,
            Some(HINSTANCE(module.0)),
            None,
        )
        .context("creating the viewer window")?;
        // Minimized and not activated: the game on the user's own desktop keeps the focus. The
        // RDP client keeps rendering while minimized (setup's RemoteDesktop_SuppressWhenMinimized).
        if show {
            let _ = ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
        }
        Ok(hwnd)
    }
}

/// This process's account as the RDP client names it, `COMPUTER\user`
/// (Annex: `Environment.MachineName + "\" + Environment.UserName`).
fn account() -> Result<String> {
    let (mut computer, mut user) = ([0u16; 256], [0u16; 257]);
    let (mut computer_size, mut user_size) = (computer.len() as u32, user.len() as u32);
    // SAFETY: each buffer is writable for the size passed with it.
    unsafe {
        GetComputerNameExW(
            ComputerNameNetBIOS,
            Some(PWSTR(computer.as_mut_ptr())),
            &mut computer_size,
        )
        .context("reading this PC's name")?;
        GetUserNameW(Some(PWSTR(user.as_mut_ptr())), &mut user_size)
            .context("reading the Windows account name")?;
    }
    Ok(format!(
        r"{}\{}",
        crate::text::from_wide(&computer),
        crate::text::from_wide(&user)
    ))
}

struct App<C: Client = Rdp> {
    rdp: C,
    hwnd: HWND,
    viewer: Viewer,
    store: PasswordStore<super::UserDpapi>,
    /// The password in use: entered by the console or loaded from the store.
    password: Option<Secret>,
    display: DisplaySettings,
    rdp_port: u16,
    clock: Instant,
    current: u32,
    console: u32,
    notice: Option<String>,
    reported: Option<ViewerStatus>,
    quit: bool,
}
impl<C: Client> App<C> {
    fn new(
        rdp: C,
        hwnd: HWND,
        store: PasswordStore<super::UserDpapi>,
        current: u32,
        console: u32,
    ) -> Self {
        Self {
            rdp,
            hwnd,
            viewer: Viewer::default(),
            store,
            password: None,
            display: DisplaySettings::default(),
            rdp_port: 3389,
            clock: Instant::now(),
            current,
            console,
            notice: None,
            reported: None,
            quit: false,
        }
    }
    fn now(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }
    fn state(&mut self) -> RdpState {
        match self.rdp.state() {
            Ok(state) => state,
            Err(error) => {
                self.notice = Some(format!("RDP state: {error:#}"));
                RdpState::Disconnected
            }
        }
    }
    /// The saved password when none is in use; Some(false) when there is one now.
    fn saved_password(&mut self) -> Option<bool> {
        if self.password.is_none() {
            match self.store.load() {
                Ok(saved) => self.password = saved,
                Err(error) => self.notice = Some(format!("{error:#}")),
            }
        }
        self.password.is_some().then_some(false)
    }
    fn command(&mut self, command: ViewerCommand) {
        let (now, state) = (self.now(), self.state());
        let actions = match command {
            ViewerCommand::Start {
                display,
                rdp_port,
                password,
            } => {
                self.display = display;
                self.rdp_port = rdp_port;
                self.notice = None;
                let kind = match password.filter(|p| !p.is_empty()) {
                    Some(password) => {
                        self.password = Some(password);
                        Some(true)
                    }
                    None => self.saved_password(),
                };
                self.viewer.connect(now, state, kind)
            }
            ViewerCommand::Stop => self.viewer.detach(state),
            ViewerCommand::Reconnect => {
                let kind = self.saved_password();
                self.viewer.reconnect(now, state, kind)
            }
            ViewerCommand::Show => {
                // SAFETY: the window belongs to this thread.
                unsafe {
                    let _ = ShowWindow(self.hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(self.hwnd);
                }
                Vec::new()
            }
            ViewerCommand::EndSession { child, generation } => {
                let actual = super::child_session_id().unwrap_or(0);
                match self.viewer.end_session(
                    now,
                    Confirmation::from_status(child, generation),
                    actual,
                    self.current,
                    self.console,
                ) {
                    Ok(actions) => actions,
                    Err(error) => {
                        self.notice = Some(error.to_string());
                        Vec::new()
                    }
                }
            }
            ViewerCommand::ForgetPassword => {
                if let Err(error) = self.store.forget() {
                    self.notice = Some(format!("{error:#}"));
                }
                Vec::new()
            }
            ViewerCommand::Quit => {
                self.quit = true;
                self.viewer.detach(state)
            }
        };
        self.apply(actions);
    }
    fn poll(&mut self) {
        let state = self.state();
        let reason = (state == RdpState::Disconnected)
            .then(|| self.rdp.disconnect_reason())
            .flatten();
        let actions = self.viewer.poll(
            self.now(),
            state,
            reason,
            super::child_session_id(),
            self.current,
            self.console,
        );
        self.apply(actions);
    }
    fn apply(&mut self, actions: Vec<ViewerAction>) {
        for action in actions {
            let done = match action {
                ViewerAction::Connect => self.connect(),
                ViewerAction::Disconnect => self.rdp.disconnect(),
                ViewerAction::SavePassword => match &self.password {
                    Some(password) => self
                        .store
                        .save(password)
                        .context("the password signed in but could not be saved"),
                    None => Ok(()),
                },
                ViewerAction::DropPassword => {
                    self.password = None;
                    Ok(())
                }
                ViewerAction::SignOut(child) => {
                    if let Err(error) = super::sign_out(child) {
                        self.viewer.end_failed(&format!("{error:#}"));
                    }
                    Ok(())
                }
            };
            if let Err(error) = done {
                self.notice = Some(format!("{error:#}"));
            }
        }
    }
    fn connect(&mut self) -> Result<()> {
        let result = (|| {
            let password = self
                .password
                .as_ref()
                .context("no Windows password to sign in with")?;
            // Every connect, the automatic reconnects too (the client's own are off), right
            // before the password leaves for the server: a refusal is not retried.
            connect_checked(
                || {
                    self.rdp
                        .configure(&account()?, password, self.display, self.rdp_port)
                },
                || self.rdp.check_server(self.rdp_port),
                || self.rdp.connect(),
            )
        })();
        match &result {
            Ok(notices) if !notices.is_empty() => self.notice = Some(notices.join("; ")),
            Ok(_) => {}
            Err(error) => {
                // A refused server or a setting the session needs: no retry loop, and the
                // console shows why rather than a failed sign-in.
                let state = self.state();
                let actions = self.viewer.refuse(state, &format!("{error:#}"));
                self.apply(actions);
            }
        }
        result
            .map(drop)
            .context("the child session's desktop could not be opened")
    }
    fn status(&self) -> ViewerStatus {
        ViewerStatus {
            phase: self.viewer.phase(),
            message: self.viewer.message().into(),
            child: self.viewer.child(),
            generation: self.viewer.gate().generation(),
            password_saved: self.store.saved(),
            notice: self.notice.clone(),
        }
    }
    /// One status line after every change; false when the host is gone.
    fn report(&mut self, out: &mut impl Write) -> bool {
        let status = self.status();
        if self.reported.as_ref() == Some(&status) {
            return true;
        }
        let Ok(line) = serde_json::to_string(&status) else {
            return true;
        };
        self.reported = Some(status);
        writeln!(out, "{line}").and_then(|()| out.flush()).is_ok()
    }
}

/// Read commands until the host closes the pipe (then quit).
fn read_commands(hwnd: isize, sender: mpsc::Sender<Result<ViewerCommand, String>>) {
    let wake = || {
        // SAFETY: posting to a window handle that may be gone fails harmlessly.
        let _ =
            unsafe { PostMessageW(Some(HWND(hwnd as *mut c_void)), WAKE, WPARAM(0), LPARAM(0)) };
    };
    // The raw handle, unbuffered: std's stdin keeps a buffer of its own that is never wiped.
    // SAFETY: the standard input handle stays open for the whole process, and ManuallyDrop
    // never closes it.
    let stdin =
        ManuallyDrop::new(unsafe { File::from_raw_handle(std::io::stdin().as_raw_handle()) });
    if forward_commands(&*stdin, &sender, wake) {
        let _ = sender.send(Ok(ViewerCommand::Quit));
        wake();
    }
}
/// The commands on `input` to the window thread until the pipe closes (true)
/// or the window thread is gone (false). Each line is read into one buffer
/// that never grows and is wiped once the next line is read, and parsed from
/// there: the password is never copied into a string.
fn forward_commands(
    input: impl Read,
    sender: &mpsc::Sender<Result<ViewerCommand, String>>,
    wake: impl Fn(),
) -> bool {
    let mut lines = LineReader::new(input, COMMAND_CAP);
    loop {
        let command = match lines.read_line() {
            Ok(None) => return true,
            Err(error) if error.kind() == ErrorKind::InvalidData => Err(format!(
                "the host sent a command longer than {} KiB; the viewer did not read it",
                COMMAND_CAP / 1024
            )),
            Err(_) => return true,
            Ok(Some(line)) if line.trim_ascii().is_empty() => continue,
            // Only the position: serde's message can quote the line, which may hold the password.
            Ok(Some(line)) => {
                serde_json::from_slice::<ViewerCommand>(line.trim_ascii()).map_err(|error| {
                    format!(
                        "the host sent a command the viewer cannot read (column {})",
                        error.column()
                    )
                })
            }
        };
        if sender.send(command).is_err() {
            return false;
        }
        wake();
    }
}

/// The saved password's file in the signed-in user's own profile
/// (`%LOCALAPPDATA%\Rubylight`), DPAPI-protected with that account's key.
fn password_path() -> Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    // SAFETY: no token means this process's own account; the string Windows returns is
    // copied, then freed once.
    let folder = unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None)
            .context("finding this account's local app data folder")?;
        let copy = std::ffi::OsString::from_wide(path.as_wide());
        CoTaskMemFree(Some(path.0 as *const _));
        PathBuf::from(copy)
    };
    Ok(folder.join("Rubylight").join("child-session-password.bin"))
}

/// Run the viewer until the host quits it or closes its pipe.
pub fn run() -> Result<()> {
    let password = password_path()?;
    let current = crate::process::current_session()?;
    let console = crate::process::console_session();
    // The child session is the console user's own; a viewer anywhere else would attach the
    // wrong account's desktop.
    if current != console {
        bail!(
            "the child-session viewer runs only in the session at this PC's screen (session {console}), not in session {current}"
        );
    }
    // SAFETY: the RDP control needs an apartment-threaded OLE on this, its only, thread.
    unsafe { OleInitialize(None) }.context("OLE did not start")?;
    let hwnd = create_window(true)?;
    let (rdp, _control) = Rdp::create(hwnd)?;
    // SAFETY: the window belongs to this thread; a zero result means no timer, which the
    // command wake-ups still cover.
    unsafe { SetTimer(Some(hwnd), 1, POLL_MS, None) };
    let mut app = App::new(
        rdp,
        hwnd,
        PasswordStore::new(password, super::UserDpapi),
        current,
        console,
    );
    let (sender, commands) = mpsc::channel();
    let handle = hwnd.0 as isize;
    std::thread::spawn(move || read_commands(handle, sender));
    let mut out = std::io::stdout();
    app.report(&mut out);
    let mut message = MSG::default();
    // SAFETY: this thread owns the window and pumps its messages.
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
        let ours = message.hwnd == hwnd && matches!(message.message, WAKE | WM_TIMER);
        // SAFETY: the message came from GetMessageW on this thread.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if !ours {
            continue;
        }
        // Commands wait in the channel; a wake-up lost in a modal loop is picked up at the
        // next poll.
        while let Ok(command) = commands.try_recv() {
            match command {
                Ok(command) => app.command(command),
                Err(problem) => app.notice = Some(problem),
            }
        }
        if !app.quit {
            app.poll();
        }
        if !app.report(&mut out) || app.quit {
            break;
        }
    }
    let _ = app.rdp.disconnect();
    // SAFETY: the window belongs to this thread.
    let _ = unsafe { DestroyWindow(hwnd) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rdp_control_takes_annexs_settings_without_connecting() {
        // Read-only: the control is created and configured, never connected, so this PC's RDP
        // state does not change. The password is a placeholder.
        // SAFETY: test thread; the window is destroyed before it returns.
        unsafe { OleInitialize(None) }.unwrap();
        // Never shown: nothing appears on this PC's screen.
        let hwnd = create_window(false).unwrap();
        let (rdp, _) = Rdp::create(hwnd).unwrap();
        assert_eq!(rdp.state().unwrap(), RdpState::Disconnected);
        let notices = rdp
            .configure(
                r"PC\nobody",
                &Secret::new("placeholder".into()),
                DisplaySettings::default(),
                3390,
            )
            .unwrap();
        assert!(notices.is_empty(), "{notices:?}");
        let read = |group: &str, name: &str| {
            get(
                &get(&rdp.control, group).unwrap().into_dispatch().unwrap(),
                name,
            )
            .unwrap()
            .as_i32()
            .unwrap()
        };
        assert_eq!(read("AdvancedSettings2", "RDPPort"), 3390);
        assert_eq!(read("AdvancedSettings5", "AuthenticationLevel"), 0);
        assert_eq!(read("AdvancedSettings2", "EnableAutoReconnect"), 0);
        let server = get(&rdp.control, "Server").unwrap();
        // SAFETY: the control returns the server name as a VT_BSTR.
        let server = unsafe {
            assert_eq!(server.0.Anonymous.Anonymous.vt, VT_BSTR);
            server.0.Anonymous.Anonymous.Anonymous.bstrVal.to_string()
        };
        assert_eq!(server, "127.0.0.1");
        assert_eq!(rdp.state().unwrap(), RdpState::Disconnected);
        // SAFETY: the window belongs to this thread.
        unsafe { DestroyWindow(hwnd) }.unwrap();
    }

    #[test]
    fn commands_come_off_the_pipe_capped_and_never_quote_it() {
        let start = ViewerCommand::Start {
            display: DisplaySettings::default(),
            rdp_port: 3390,
            password: Some(Secret::new("correct horse".into())),
        }
        .line()
        .unwrap();
        let long = format!("{}SECRET\n", "x".repeat(COMMAND_CAP));
        let input = format!(
            "{}{long}\n{{\"command\":\"stop\"}}\n{{bad\n",
            start.as_str()
        );
        let (sender, commands) = mpsc::channel();
        let woken = std::cell::Cell::new(0);
        assert!(forward_commands(input.as_bytes(), &sender, || woken.set(woken.get() + 1)));
        let commands: Vec<_> = commands.try_iter().collect();
        assert_eq!(commands.len(), 4);
        assert_eq!(woken.get(), 4);
        match &commands[0] {
            Ok(ViewerCommand::Start {
                rdp_port: 3390,
                password: Some(password),
                ..
            }) => assert_eq!(password.expose(), "correct horse"),
            other => panic!("{other:?}"),
        }
        let too_long = commands[1].as_ref().unwrap_err();
        assert!(too_long.contains("longer than 16 KiB"), "{too_long}");
        assert!(matches!(commands[2], Ok(ViewerCommand::Stop)));
        let bad = commands[3].as_ref().unwrap_err();
        assert!(bad.contains("cannot read (column"), "{bad}");
        for problem in [too_long, bad] {
            assert!(
                !problem.contains("SECRET") && !problem.contains("bad"),
                "{problem}"
            );
        }
        // The window thread gone: the reader stops without a quit.
        drop(commands);
        let (sender, gone) = mpsc::channel();
        drop(gone);
        assert!(!forward_commands(
            "{\"command\":\"stop\"}\n".as_bytes(),
            &sender,
            || ()
        ));
    }

    #[test]
    fn the_server_is_checked_right_before_connecting() {
        let steps = std::cell::RefCell::new(Vec::new());
        let step = |name: &'static str| steps.borrow_mut().push(name);
        let notices = connect_checked(
            || {
                step("configure");
                Ok(vec!["notice".into()])
            },
            || {
                step("check");
                Ok(())
            },
            || {
                step("connect");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(notices, ["notice"]);
        assert_eq!(*steps.borrow(), ["configure", "check", "connect"]);
        // A refused server never connects.
        steps.borrow_mut().clear();
        let error = connect_checked(
            || {
                step("configure");
                Ok(Vec::new())
            },
            || bail!("another program (process 6666) listens on port 3390"),
            || {
                step("connect");
                Ok(())
            },
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("process 6666"), "{error:#}");
        assert_eq!(*steps.borrow(), ["configure"]);
    }

    /// The RDP client and this PC's server as the steps the viewer takes; the
    /// check refuses when told to.
    #[derive(Default)]
    struct Steps {
        steps: std::cell::RefCell<Vec<String>>,
        refuse: std::cell::Cell<bool>,
    }
    impl Steps {
        fn step(&self, step: String) {
            self.steps.borrow_mut().push(step);
        }
        fn take(&self) -> Vec<String> {
            self.steps.take()
        }
    }
    impl Client for Steps {
        fn state(&self) -> Result<RdpState> {
            Ok(RdpState::Disconnected)
        }
        fn disconnect_reason(&self) -> Option<i32> {
            None
        }
        fn configure(
            &self,
            _: &str,
            password: &Secret,
            _: DisplaySettings,
            rdp_port: u16,
        ) -> Result<Vec<String>> {
            self.step(format!("configure {rdp_port} {}", password.expose()));
            Ok(Vec::new())
        }
        fn check_server(&self, rdp_port: u16) -> Result<()> {
            self.step(format!("check {rdp_port}"));
            if self.refuse.get() {
                bail!("another program (process 6666) listens on port {rdp_port}");
            }
            Ok(())
        }
        fn connect(&self) -> Result<()> {
            self.step("connect".into());
            Ok(())
        }
        fn disconnect(&self) -> Result<()> {
            self.step("disconnect".into());
            Ok(())
        }
    }

    #[test]
    fn the_viewer_checks_the_server_right_before_every_connect() {
        let folder = tempfile::tempdir().unwrap();
        let store = PasswordStore::new(folder.path().join("password.bin"), super::super::UserDpapi);
        let mut app = App::new(Steps::default(), HWND::default(), store, 1, 1);
        app.command(ViewerCommand::Start {
            display: DisplaySettings::default(),
            rdp_port: 3390,
            password: Some(Secret::new("correct horse".into())),
        });
        let connected = ["configure 3390 correct horse", "check 3390", "connect"];
        assert_eq!(app.rdp.take(), connected);
        // The viewer's own reconnects are checked too.
        app.command(ViewerCommand::Reconnect);
        assert_eq!(app.rdp.take(), connected);
        // A refused server is never connected to, and the console says why.
        app.rdp.refuse.set(true);
        app.command(ViewerCommand::Reconnect);
        assert_eq!(
            app.rdp.take(),
            ["configure 3390 correct horse", "check 3390"]
        );
        let status = app.status();
        assert_eq!(status.phase, butterpollo_core::child_session::Phase::Idle);
        assert!(
            status.message.contains("process 6666"),
            "{}",
            status.message
        );
        // Not retried on its own.
        app.poll();
        assert_eq!(app.rdp.take(), Vec::<String>::new());
    }

    #[test]
    fn the_password_is_encoded_without_a_spare_copy() {
        let wide = wide_secret("p\u{e4}ss\u{1f600}");
        assert_eq!(
            *wide,
            "p\u{e4}ss\u{1f600}".encode_utf16().collect::<Vec<_>>()
        );
        // Sized up front: extending never reallocated and left the old buffer unwiped.
        assert_eq!(wide.capacity(), wide.len());
        let value = Value::secret(&Secret::new("p\u{e4}ss".into()));
        // SAFETY: Value::secret makes a VT_BSTR.
        let text = unsafe { value.0.0.Anonymous.Anonymous.Anonymous.bstrVal.to_string() };
        assert_eq!(text, "p\u{e4}ss");
    }

    #[test]
    fn values_convert() {
        assert_eq!(Value::int(-7).as_i32().unwrap(), -7);
        assert_eq!(Value::uint(180).as_i32().unwrap(), 180);
        assert!(Value::text("x").into_dispatch().is_err());
    }
}
