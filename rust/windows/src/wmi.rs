//! WMI through COM, for what Windows offers only there: the network adapters
//! as `Get-NetAdapter` lists them (`root\StandardCimv2`), their drivers' wake
//! settings (`root\wmi`) and firewall rules by their ID. Setup shares this
//! file (`#[path]` in rust/setup/src/main.rs), so it uses only `windows` and
//! `anyhow`.
//!
//! The PowerShell cmdlets these replace pass their switches (`-Physical`,
//! `-AllProperties`, `-PolicyStore`) to the same providers as operation
//! options; [`context`] carries them the same way.

use anyhow::{Context, Result, bail};
use std::{
    mem::ManuallyDrop,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{RecvTimeoutError, channel},
    },
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::{VARIANT_FALSE, VARIANT_TRUE},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoSetProxyBlanket, CoUninitialize, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
            },
            Ole::{SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound},
            Variant::{
                VARENUM, VARIANT, VT_ARRAY, VT_BOOL, VT_BSTR, VT_EMPTY, VT_I1, VT_I2, VT_I4,
                VT_NULL, VT_UI1, VT_UI2, VT_UI4, VariantClear,
            },
            Wmi::{
                IEnumWbemClassObject, IWbemClassObject, IWbemContext, IWbemLocator, IWbemServices,
                IWbemStatusCodeText, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
                WBEM_FLAG_RETURN_WBEM_COMPLETE, WBEM_FLAG_UPDATE_ONLY, WBEM_GENERIC_FLAG_TYPE,
                WBEM_INFINITE, WbemContext, WbemLocator, WbemStatusCodeText,
            },
        },
    },
    core::{BSTR, HRESULT, HSTRING, IUnknown, Interface, PCWSTR},
};

/// `RPC_C_AUTHN_WINNT` and `RPC_C_AUTHZ_NONE` (`Win32_System_Rpc`).
const AUTHN_WINNT: u32 = 10;
const AUTHZ_NONE: u32 = 0;

/// The calls that ran out of time, by `what`, each with the flag its thread
/// sets when it ends after all.
static LATE: Mutex<Vec<(String, Arc<AtomicBool>)>> = Mutex::new(Vec::new());

/// Sets its flag when dropped, so a thread that panics has ended too.
struct Ended(Arc<AtomicBool>);

impl Drop for Ended {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Runs `work` on a thread of its own with COM initialised, and returns its
/// result, or an error naming `what` when it hasn't finished within
/// `timeout`. A late `work` goes on unwatched until its process ends; while
/// it does, another call for the same `what` fails the same way without
/// starting a thread, so a hung WMI never collects threads.
pub fn on_com_thread<T: Send + 'static>(
    what: &str,
    timeout: Duration,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    {
        let mut late = LATE.lock().unwrap_or_else(PoisonError::into_inner);
        late.retain(|(_, ended)| !ended.load(Ordering::Acquire));
        if late.iter().any(|(name, _)| name == what) {
            bail!(
                "{what} did not finish within {timeout:?}: the previous one is still running past its deadline"
            );
        }
    }
    let ended = Arc::new(AtomicBool::new(false));
    let guard = Ended(ended.clone());
    let (send, receive) = channel();
    std::thread::Builder::new()
        .name("wmi".into())
        .spawn(move || {
            let _ended = guard;
            // SAFETY: No pointers; a successful call is balanced by CoUninitialize below.
            let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok();
            let uninitialize = initialized.is_ok();
            let result = match initialized {
                // Flattened to text here: an error may hold COM error info,
                // which must not be released once COM is gone.
                Ok(()) => work().map_err(|error| anyhow::anyhow!("{error:#}")),
                Err(error) => Err(anyhow::Error::from(error).context("initialising COM")),
            };
            if uninitialize {
                // SAFETY: Balances the CoInitializeEx above; every COM object `work` made
                // is gone with it.
                unsafe { CoUninitialize() };
            }
            let _ = send.send(result);
        })
        .with_context(|| format!("starting a thread for {what}"))?;
    match receive.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => {
            LATE.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((what.to_owned(), ended));
            bail!("{what} did not finish within {timeout:?}")
        }
        Err(RecvTimeoutError::Disconnected) => bail!("{what} stopped without a result"),
    }
}

/// A connection to one WMI namespace of this PC.
pub struct Wmi {
    services: IWbemServices,
}

impl Wmi {
    /// Connects to `namespace`, such as `root\wmi`, on this thread, which has
    /// COM initialised ([`on_com_thread`]).
    pub fn connect(namespace: &str) -> Result<Self> {
        let none = BSTR::new();
        // SAFETY: COM is initialised on this thread; every argument outlives its call.
        unsafe {
            let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)
                .context("starting WMI")?;
            let services = locator
                .ConnectServer(&BSTR::from(namespace), &none, &none, &none, 0, &none, None)
                .map_err(failure)
                .with_context(|| format!("connecting to WMI's {namespace}"))?;
            impersonate(&services)?;
            Ok(Self { services })
        }
    }

    /// Every object `wql` selects, read to the end.
    pub fn query(
        &self,
        wql: &str,
        context: Option<&IWbemContext>,
    ) -> Result<Vec<IWbemClassObject>> {
        // SAFETY: The services and context are live COM objects; the strings outlive the call.
        let items: IEnumWbemClassObject = unsafe {
            self.services.ExecQuery(
                &BSTR::from("WQL"),
                &BSTR::from(wql),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                context,
            )
        }
        .map_err(failure)
        .with_context(|| format!("WMI query {wql}"))?;
        impersonate(&items)?;
        let mut objects = Vec::new();
        loop {
            let mut row = [None];
            let mut returned = 0;
            // SAFETY: `row` holds one slot and `returned` is a live local.
            unsafe { items.Next(WBEM_INFINITE, &mut row, &mut returned) }
                .ok()
                .map_err(failure)
                .with_context(|| format!("WMI query {wql}"))?;
            match row[0].take() {
                Some(object) if returned == 1 => objects.push(object),
                _ => break,
            }
        }
        Ok(objects)
    }

    /// The object at `path`, such as an instance's `__RELPATH`.
    pub fn get(&self, path: &str) -> Result<IWbemClassObject> {
        let mut object = None;
        // SAFETY: `object` is a live local the call fills; the path outlives the call.
        unsafe {
            self.services.GetObject(
                &BSTR::from(path),
                WBEM_FLAG_RETURN_WBEM_COMPLETE,
                None,
                Some(&mut object),
                None,
            )
        }
        .map_err(failure)
        .with_context(|| format!("reading {path}"))?;
        object.with_context(|| format!("WMI returned nothing for {path}"))
    }

    /// Saves the changed properties of an existing instance
    /// (`Set-CimInstance`).
    pub fn update(&self, object: &IWbemClassObject) -> Result<()> {
        // SAFETY: Both objects are live COM objects.
        unsafe {
            self.services.PutInstance(
                object,
                WBEM_GENERIC_FLAG_TYPE(WBEM_FLAG_UPDATE_ONLY.0),
                None,
                None,
            )
        }
        .map_err(failure)
    }

    /// Calls `method`, without parameters, on the object at `path`; a
    /// `ReturnValue` other than 0 is an error.
    pub fn call(&self, path: &str, method: &str, context: Option<&IWbemContext>) -> Result<()> {
        let mut out = None;
        // SAFETY: The strings outlive the call and `out` is a live local it fills.
        unsafe {
            self.services.ExecMethod(
                &BSTR::from(path),
                &BSTR::from(method),
                WBEM_GENERIC_FLAG_TYPE(0),
                context,
                None,
                Some(&mut out),
                None,
            )
        }
        .map_err(failure)
        .with_context(|| format!("{method} on {path}"))?;
        if let Some(out) = out
            && let Some(code) = number(&out, "ReturnValue")?
            && code != 0
        {
            bail!("{method} on {path} returned {code}");
        }
        Ok(())
    }
}

/// Sets the security WMI needs on a proxy: calls made as this process's
/// account, which the provider may impersonate.
fn impersonate(proxy: &impl Interface) -> Result<()> {
    // The interface's own proxy, not a queried IUnknown: a blanket holds per
    // interface proxy.
    let raw = proxy.as_raw();
    // SAFETY: `raw` is the live interface pointer `proxy` holds, borrowed for this call.
    let unknown = unsafe { IUnknown::from_raw_borrowed(&raw) }.context("no WMI proxy")?;
    // SAFETY: `unknown` is a live COM proxy; no pointers besides it.
    unsafe {
        CoSetProxyBlanket(
            unknown,
            AUTHN_WINNT,
            AUTHZ_NONE,
            PCWSTR::null(),
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )
    }
    .context("setting the security of a WMI connection")
}

/// A WMI error with WMI's own words for its code, which Windows' message
/// table doesn't have (`WBEM_E_NOT_FOUND` is "Not found").
fn failure(error: windows::core::Error) -> anyhow::Error {
    if !error.message().trim().is_empty() {
        return error.into();
    }
    match status_text(error.code()) {
        Some(text) => anyhow::anyhow!("{text} ({:#010x})", error.code().0),
        None => error.into(),
    }
}

fn status_text(code: HRESULT) -> Option<String> {
    // SAFETY: COM is initialised on this thread (Wmi exists); no pointers.
    unsafe {
        let text: IWbemStatusCodeText =
            CoCreateInstance(&WbemStatusCodeText, None, CLSCTX_INPROC_SERVER).ok()?;
        let text = text.GetErrorCodeText(code, 0, 0).ok()?.to_string();
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    }
}

/// A value for [`context`].
pub enum Value {
    Flag(bool),
    Text(&'static str),
}

/// The operation options a cmdlet's switches become, such as `Physical`
/// for `Get-NetAdapter -Physical`.
pub fn context(options: &[(&str, Value)]) -> Result<IWbemContext> {
    // SAFETY: COM is initialised on this thread; each VARIANT is live during its call and
    // cleared after it.
    unsafe {
        let context: IWbemContext = CoCreateInstance(&WbemContext, None, CLSCTX_INPROC_SERVER)
            .context("making a WMI context")?;
        for (name, value) in options {
            let mut variant = Variant(VARIANT::default());
            let inner = &mut *variant.0.Anonymous.Anonymous;
            match value {
                Value::Flag(flag) => {
                    inner.vt = VT_BOOL;
                    inner.Anonymous.boolVal = if *flag { VARIANT_TRUE } else { VARIANT_FALSE };
                }
                Value::Text(text) => {
                    inner.vt = VT_BSTR;
                    inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(*text));
                }
            }
            context
                .SetValue(&HSTRING::from(*name), 0, &variant.0)
                .with_context(|| format!("setting the WMI option {name}"))?;
        }
        Ok(context)
    }
}

/// A VARIANT cleared when dropped.
struct Variant(VARIANT);

impl Drop for Variant {
    fn drop(&mut self) {
        // SAFETY: The VARIANT is initialised (zeroed is VT_EMPTY) and owned here.
        let _ = unsafe { VariantClear(&mut self.0) };
    }
}

impl Variant {
    fn of(object: &IWbemClassObject, name: &str) -> Result<Self> {
        let mut value = Self(VARIANT::default());
        // SAFETY: `value` is a live VARIANT the call fills; the name outlives the call.
        unsafe { object.Get(&HSTRING::from(name), 0, &mut value.0, None, None) }
            .map_err(failure)
            .with_context(|| format!("reading the WMI property {name}"))?;
        Ok(value)
    }
    fn vt(&self) -> VARENUM {
        // SAFETY: Every VARIANT starts with its type.
        unsafe { self.0.Anonymous.Anonymous.vt }
    }
    fn missing(&self) -> bool {
        matches!(self.vt(), VT_EMPTY | VT_NULL)
    }
}

/// A string property; `None` when it is null.
pub fn string(object: &IWbemClassObject, name: &str) -> Result<Option<String>> {
    let value = Variant::of(object, name)?;
    match value.vt() {
        _ if value.missing() => Ok(None),
        // SAFETY: The type says the BSTR view holds the value.
        VT_BSTR => Ok(Some(unsafe {
            value.0.Anonymous.Anonymous.Anonymous.bstrVal.to_string()
        })),
        other => bail!(
            "the WMI property {name} is not a string but type {}",
            other.0
        ),
    }
}

/// A boolean property; `None` when it is null.
pub fn boolean(object: &IWbemClassObject, name: &str) -> Result<Option<bool>> {
    let value = Variant::of(object, name)?;
    match value.vt() {
        _ if value.missing() => Ok(None),
        // SAFETY: The type says the VARIANT_BOOL view holds the value.
        VT_BOOL => Ok(Some(
            unsafe { value.0.Anonymous.Anonymous.Anonymous.boolVal }.0 != 0,
        )),
        other => bail!(
            "the WMI property {name} is not a boolean but type {}",
            other.0
        ),
    }
}

/// An integer property that fits in 32 bits; `None` when it is null. WMI
/// hands out `uint32` as `VT_I4`.
pub fn number(object: &IWbemClassObject, name: &str) -> Result<Option<u32>> {
    let value = Variant::of(object, name)?;
    // SAFETY: Each arm reads the view its type names.
    let number = unsafe {
        let union = &value.0.Anonymous.Anonymous.Anonymous;
        match value.vt() {
            _ if value.missing() => return Ok(None),
            VT_I4 => union.lVal as u32,
            VT_UI4 => union.ulVal,
            VT_I2 => union.iVal as u16 as u32,
            VT_UI2 => union.uiVal as u32,
            VT_I1 => union.cVal as u8 as u32,
            VT_UI1 => union.bVal as u32,
            other => bail!(
                "the WMI property {name} is not a number but type {}",
                other.0
            ),
        }
    };
    Ok(Some(number))
}

/// A string array property; empty when it is null.
pub fn strings(object: &IWbemClassObject, name: &str) -> Result<Vec<String>> {
    let value = Variant::of(object, name)?;
    if value.missing() {
        return Ok(Vec::new());
    }
    if value.vt().0 != VT_ARRAY.0 | VT_BSTR.0 {
        bail!(
            "the WMI property {name} is not a string array but type {}",
            value.vt().0
        );
    }
    // SAFETY: The type says `parray` is a one-dimensional SAFEARRAY of BSTRs, alive with
    // `value`; each element is copied into a BSTR that frees it.
    unsafe {
        let array = value.0.Anonymous.Anonymous.Anonymous.parray;
        if array.is_null() {
            return Ok(Vec::new());
        }
        let (first, last) = (SafeArrayGetLBound(array, 1)?, SafeArrayGetUBound(array, 1)?);
        let mut strings = Vec::new();
        for index in first..=last {
            let mut text = BSTR::new();
            SafeArrayGetElement(array, &index, (&raw mut text).cast())?;
            strings.push(text.to_string());
        }
        Ok(strings)
    }
}

/// Sets a boolean property on `object`; [`Wmi::update`] saves it.
pub fn set_boolean(object: &IWbemClassObject, name: &str, flag: bool) -> Result<()> {
    let mut value = Variant(VARIANT::default());
    // SAFETY: `value` is a live VARIANT of the type set here; the name outlives the call.
    unsafe {
        let inner = &mut *value.0.Anonymous.Anonymous;
        inner.vt = VT_BOOL;
        inner.Anonymous.boolVal = if flag { VARIANT_TRUE } else { VARIANT_FALSE };
        object.Put(&HSTRING::from(name), 0, &value.0, 0)
    }
    .map_err(failure)
    .with_context(|| format!("setting the WMI property {name}"))
}

/// `text` as a WQL string literal.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wql_literals_escape_quotes_and_backslashes() {
        assert_eq!(
            quote("@FirewallAPI.dll,-28752"),
            "'@FirewallAPI.dll,-28752'"
        );
        assert_eq!(quote(r"PCI\VEN_10EC"), r"'PCI\\VEN_10EC'");
        assert_eq!(quote("Bob's LAN"), r"'Bob\'s LAN'");
    }

    #[test]
    fn work_past_its_timeout_is_an_error_and_a_panic_is_no_result() {
        let error = on_com_thread("a slow read", Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(2));
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "a slow read did not finish within 50ms");
        let error = on_com_thread("a crash", Duration::from_secs(30), || -> Result<()> {
            panic!("on purpose")
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "a crash stopped without a result");
        assert_eq!(
            on_com_thread("a sum", Duration::from_secs(30), || Ok(2 + 2)).unwrap(),
            4
        );
    }

    #[test]
    fn a_late_call_starts_no_other_until_it_ends() {
        use std::sync::atomic::AtomicBool;
        let what = "a hung read";
        let error = on_com_thread(what, Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_millis(1500));
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "a hung read did not finish within 50ms");
        // The same call again fails at once and starts nothing.
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        let error = on_com_thread(what, Duration::from_secs(30), move || {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        })
        .unwrap_err();
        assert!(error.to_string().contains("still running"), "{error:#}");
        std::thread::sleep(Duration::from_millis(100));
        assert!(!ran.load(Ordering::SeqCst));
        // Another call is not held up by it.
        assert_eq!(
            on_com_thread("another read", Duration::from_secs(30), || Ok(1)).unwrap(),
            1
        );
        // Once the late one has ended, the next goes ahead.
        std::thread::sleep(Duration::from_millis(2000));
        assert_eq!(
            on_com_thread(what, Duration::from_secs(30), || Ok(2)).unwrap(),
            2
        );
    }

    #[test]
    fn this_pcs_wmi_is_read_and_its_errors_are_worded() -> Result<()> {
        // Read-only: the operating system's own instance.
        let (caption, missing) = on_com_thread("reading WMI", Duration::from_secs(60), || {
            let wmi = Wmi::connect(r"root\cimv2")?;
            let systems = wmi.query("SELECT Caption FROM Win32_OperatingSystem", None)?;
            let caption = string(systems.first().context("no operating system")?, "Caption")?;
            let missing = wmi
                .get("Win32_Service.Name='RubylightNoSuchService'")
                .unwrap_err();
            Ok((caption, format!("{missing:#}")))
        })?;
        assert!(caption.is_some_and(|c| c.contains("Windows")));
        // WMI's words for WBEM_E_NOT_FOUND, in the PC's language, before the code.
        let worded = missing
            .rsplit(": ")
            .next()
            .and_then(|last| last.strip_suffix("(0x80041002)"))
            .is_some_and(|text| !text.trim().is_empty());
        assert!(worded, "{missing}");
        Ok(())
    }
}
