//! Windows side of background streaming in a child session: the edition, the
//! child-session calls, the registry, firewall and DPAPI behind the seams of
//! [`butterpollo_core::child_session`], the GPU the session renders on, and
//! the RDP viewer that keeps the child session attached ([`viewer`]).
//!
//! Ported from Annex (mattxslv/annex, MIT, Copyright (c) 2026 mattxslv; the
//! license text is in `third-party/licenses/annex-LICENSE.txt`): the Windows
//! settings of `src/DesktopSetup.cs` and `scripts/setup-admin.ps1`, the
//! child-session calls of `src/Annex.cs` and the RDP client hosting of
//! `src/LauncherApplication.cs`.

use crate::text::{from_wide, to_wide};
use anyhow::{Context, Result, bail};
use butterpollo_core::child_session::{Adapter, Hive, SystemSettings};
use windows::{
    Win32::{
        Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, HLOCAL, LocalFree, VARIANT_FALSE,
            VARIANT_TRUE, WIN32_ERROR,
        },
        Graphics::{
            Direct3D::D3D_FEATURE_LEVEL_11_0,
            Direct3D12::{
                D3D12_FEATURE_ARCHITECTURE, D3D12_FEATURE_DATA_ARCHITECTURE, D3D12CreateDevice,
                ID3D12Device,
            },
            Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter1, IDXGIFactory1},
        },
        NetworkManagement::WindowsFirewall::*,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
        System::{
            Com::*,
            Ole::IEnumVARIANT,
            Registry::{
                HKEY, HKEY_LOCAL_MACHINE, HKEY_USERS, REG_DWORD, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
                RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
            },
            RemoteDesktop::{
                WTSEnableChildSessions, WTSGetChildSessionId, WTSIsChildSessionsEnabled,
                WTSLogoffSession,
            },
            Variant::{VARIANT, VariantClear},
        },
    },
    core::{BOOL, BSTR, Interface, PCWSTR},
};
use zeroize::Zeroizing;

/// `EditionID` under `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`,
/// e.g. `Core` (Home) or `Professional`; empty when it can't be read.
pub fn edition_id() -> String {
    let key = to_wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let name = to_wide("EditionID");
    let mut buffer = [0u16; 128];
    let mut size = size_of_val(&buffer) as u32;
    // SAFETY: `key` and `name` are NUL-terminated and outlive the call, and `buffer` is writable
    // for the `size` bytes passed; RegGetValueW NUL-terminates REG_SZ data it returns.
    let read = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if read.is_ok() {
        from_wide(&buffer)
    } else {
        String::new()
    }
}

/// Whether Windows child sessions are on (`WTSIsChildSessionsEnabled`).
pub fn child_sessions_enabled() -> Result<bool> {
    let mut enabled = BOOL(0);
    // SAFETY: `enabled` is a live local the call writes once.
    unsafe { WTSIsChildSessionsEnabled(&mut enabled) }
        .ok()
        .context("Windows could not report whether child sessions are on")?;
    Ok(enabled.as_bool())
}

/// The child session of this process's session, None when there is none.
pub fn child_session_id() -> Option<u32> {
    let mut session = 0;
    // SAFETY: `session` is a live local the call writes once.
    unsafe { WTSGetChildSessionId(&mut session) }
        .as_bool()
        .then_some(session)
        .filter(|session| *session != 0 && *session != u32::MAX)
}

/// Sign out a session, closing every app in it. Callers check the session
/// with [`butterpollo_core::child_session::end_confirmed_session`] first.
pub fn sign_out(session: u32) -> Result<()> {
    // SAFETY: no server handle means this machine; the call does not keep any argument.
    unsafe { WTSLogoffSession(None, session, false) }
        .with_context(|| format!("Windows did not sign out session {session}"))
}

fn root(hive: &Hive, path: &str) -> (HKEY, Vec<u16>) {
    match hive {
        Hive::LocalMachine => (HKEY_LOCAL_MACHINE, to_wide(path)),
        Hive::User(sid) => (HKEY_USERS, to_wide(&format!(r"{sid}\{path}"))),
    }
}
fn absent(error: WIN32_ERROR) -> bool {
    error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND
}
fn hive_name(hive: &Hive) -> String {
    match hive {
        Hive::LocalMachine => "HKLM".into(),
        Hive::User(sid) => format!(r"HKU\{sid}"),
    }
}

/// The real registry, child-session switch and Windows Firewall. Setup and
/// restore run as an administrator (the SYSTEM host or the uninstaller).
pub struct NativeSettings {
    policy: INetFwPolicy2,
}
impl NativeSettings {
    pub fn new() -> Result<Self> {
        // SAFETY: COM is initialised on this thread before the in-process firewall object is made.
        unsafe {
            // Already initialised in another mode is fine for this in-process object.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            Ok(Self {
                policy: CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)
                    .context("opening Windows Firewall")?,
            })
        }
    }
    fn rules(&self) -> Result<Vec<INetFwRule>> {
        // SAFETY: the Rules enumerator yields VT_DISPATCH variants, so `pdispVal` is read before
        // VariantClear releases it.
        unsafe {
            let items: IEnumVARIANT = self.policy.Rules()?._NewEnum()?.cast()?;
            let mut rules = Vec::new();
            loop {
                let mut item = [VARIANT::default()];
                let mut fetched = 0;
                if items.Next(&mut item, &mut fetched).is_err() || fetched == 0 {
                    break;
                }
                let rule = item[0]
                    .Anonymous
                    .Anonymous
                    .Anonymous
                    .pdispVal
                    .as_ref()
                    .and_then(|dispatch| dispatch.cast::<INetFwRule>().ok());
                let _ = VariantClear(&mut item[0]);
                rules.extend(rule);
            }
            Ok(rules)
        }
    }
    fn in_group(rule: &INetFwRule, group: &str) -> bool {
        // SAFETY: `rule` is a live firewall rule; Grouping returns an owned BSTR.
        unsafe { rule.Grouping() }.is_ok_and(|g| g.to_string().eq_ignore_ascii_case(group))
    }
    fn name(rule: &INetFwRule) -> String {
        // SAFETY: `rule` is a live firewall rule; Name returns an owned BSTR.
        unsafe { rule.Name() }
            .map(|name| name.to_string())
            .unwrap_or_default()
    }
}
const PROTOCOLS: [(NET_FW_IP_PROTOCOL, &str); 2] = [
    (NET_FW_IP_PROTOCOL_TCP, "TCP"),
    (NET_FW_IP_PROTOCOL_UDP, "UDP"),
];
impl SystemSettings for NativeSettings {
    fn read_dword(&self, hive: &Hive, path: &str, name: &str) -> Result<Option<u32>> {
        let (root, key) = root(hive, path);
        let value = to_wide(name);
        let mut data = 0u32;
        let mut size = size_of::<u32>() as u32;
        // SAFETY: `key` and `value` are NUL-terminated and outlive the call, and `data` is the
        // 4-byte buffer `size` names.
        let read = unsafe {
            RegGetValueW(
                root,
                PCWSTR(key.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_DWORD,
                None,
                Some((&mut data as *mut u32).cast()),
                Some(&mut size),
            )
        };
        match read {
            error if error.is_ok() => Ok(Some(data)),
            error if absent(error) => Ok(None),
            error => Err(windows::core::Error::from(error.to_hresult()))
                .with_context(|| format!(r"reading {}\{path}\{name}", hive_name(hive))),
        }
    }
    fn write_dword(&mut self, hive: &Hive, path: &str, name: &str, value: u32) -> Result<()> {
        let (root, key) = root(hive, path);
        let wide = to_wide(name);
        // SAFETY: `key` and `wide` are NUL-terminated and outlive the call, and `value` is the
        // 4-byte REG_DWORD the size names.
        unsafe {
            RegSetKeyValueW(
                root,
                PCWSTR(key.as_ptr()),
                PCWSTR(wide.as_ptr()),
                REG_DWORD.0,
                Some((&value as *const u32).cast()),
                size_of::<u32>() as u32,
            )
        }
        .ok()
        .with_context(|| format!(r"writing {}\{path}\{name}", hive_name(hive)))
    }
    fn delete_value(&mut self, hive: &Hive, path: &str, name: &str) -> Result<()> {
        let (root, key) = root(hive, path);
        let wide = to_wide(name);
        // SAFETY: `key` and `wide` are NUL-terminated and outlive the call.
        match unsafe { RegDeleteKeyValueW(root, PCWSTR(key.as_ptr()), PCWSTR(wide.as_ptr())) } {
            error if error.is_ok() || absent(error) => Ok(()),
            error => Err(windows::core::Error::from(error.to_hresult()))
                .with_context(|| format!(r"removing {}\{path}\{name}", hive_name(hive))),
        }
    }
    fn child_sessions_enabled(&self) -> Result<bool> {
        child_sessions_enabled()
    }
    fn set_child_sessions(&mut self, enabled: bool) -> Result<()> {
        // SAFETY: the call takes a plain flag.
        unsafe { WTSEnableChildSessions(enabled) }
            .ok()
            .context("WTSEnableChildSessions failed")
    }
    fn firewall_on(&self) -> Result<bool> {
        for profile in [
            NET_FW_PROFILE2_DOMAIN,
            NET_FW_PROFILE2_PRIVATE,
            NET_FW_PROFILE2_PUBLIC,
        ] {
            // SAFETY: `policy` is a live firewall policy object.
            if !unsafe { self.policy.get_FirewallEnabled(profile) }?.as_bool() {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn enabled_rules(&self, group: &str) -> Result<Vec<String>> {
        let mut names: Vec<String> = self
            .rules()?
            .iter()
            .filter(|rule| Self::in_group(rule, group))
            // SAFETY: `rule` is a live firewall rule.
            .filter(|rule| unsafe { rule.Enabled() }.is_ok_and(|e| e.as_bool()))
            .map(Self::name)
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }
    fn set_rule_enabled(&mut self, group: &str, name: &str, enabled: bool) -> Result<()> {
        let mut found = false;
        for rule in self.rules()? {
            if Self::in_group(&rule, group) && Self::name(&rule) == name {
                found = true;
                // SAFETY: `rule` is a live firewall rule.
                unsafe { rule.SetEnabled(if enabled { VARIANT_TRUE } else { VARIANT_FALSE }) }
                    .with_context(|| format!("switching the firewall rule {name}"))?;
            }
        }
        if !found {
            bail!("the firewall rule {name} no longer exists");
        }
        Ok(())
    }
    fn add_block_rule(&mut self, name: &str, port: u16) -> Result<()> {
        self.remove_rule(name)?;
        for (protocol, label) in PROTOCOLS {
            // SAFETY: COM is initialised (see `new`); the rule is a fresh in-process object and
            // each BSTR outlives its call.
            unsafe {
                let rule: INetFwRule = CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER)?;
                rule.SetName(&BSTR::from(format!("{name} ({label})")))?;
                rule.SetDescription(&BSTR::from(
                    "Keeps Remote Desktop, which Rubylight's child session turns on, reachable only from this PC.",
                ))?;
                rule.SetGrouping(&BSTR::from("Rubylight"))?;
                rule.SetProtocol(protocol.0)?;
                rule.SetLocalPorts(&BSTR::from(port.to_string()))?;
                rule.SetDirection(NET_FW_RULE_DIR_IN)?;
                rule.SetAction(NET_FW_ACTION_BLOCK)?;
                rule.SetProfiles(NET_FW_PROFILE2_ALL.0)?;
                rule.SetEnabled(VARIANT_TRUE)?;
                self.policy
                    .Rules()?
                    .Add(&rule)
                    .with_context(|| format!("adding the firewall rule {name} ({label})"))?;
            }
        }
        Ok(())
    }
    fn remove_rule(&mut self, name: &str) -> Result<()> {
        for (_, label) in PROTOCOLS {
            let full = format!("{name} ({label})");
            // Several rules can share a name; remove until none is left.
            for _ in 0..16 {
                // SAFETY: `policy` is a live firewall policy object and the BSTR outlives the call.
                unsafe {
                    let rules = self.policy.Rules()?;
                    let name = BSTR::from(full.as_str());
                    if rules.Item(&name).is_err() {
                        break;
                    }
                    rules
                        .Remove(&name)
                        .with_context(|| format!("removing the firewall rule {full}"))?;
                }
            }
        }
        Ok(())
    }
}

/// DPAPI with the current Windows user's key: only that user, signed in on
/// this PC, can decrypt the blob.
pub struct UserDpapi;
const ENTROPY: &[u8] = b"Rubylight child-session password";
fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr().cast_mut(),
    }
}
impl butterpollo_core::child_session::Dpapi for UserDpapi {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>> {
        let (input, entropy) = (blob(plain), blob(ENTROPY));
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: the input blobs point at live slices the call only reads, and the output is
        // copied out before it is freed once with LocalFree.
        unsafe {
            CryptProtectData(
                &input,
                PCWSTR::null(),
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
            .context("DPAPI could not encrypt the password")?;
            let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
            Ok(bytes)
        }
    }
    fn unprotect(&self, data: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let (input, entropy) = (blob(data), blob(ENTROPY));
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: as in `protect`; the plaintext is copied into a zeroizing buffer and wiped in
        // place before LocalFree.
        unsafe {
            CryptUnprotectData(
                &input,
                None,
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
            .context("DPAPI could not decrypt the password")?;
            let plain = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
            let bytes = Zeroizing::new(plain.to_vec());
            zeroize::Zeroize::zeroize(plain);
            let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
            Ok(bytes)
        }
    }
}

/// The GPUs DXGI reports, and the one whose output this session's desktop
/// is on: in a remote session the display hangs off the adapter that
/// renders it.
pub fn adapters() -> Result<(Vec<Adapter>, Option<Adapter>)> {
    // SAFETY: the factory and adapters are live COM objects returned by these calls, GetDesc1
    // returns an owned struct, and the D3D12 feature query writes the struct whose size it gets.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let (mut all, mut render) = (Vec::new(), None);
        for index in 0..32 {
            let Ok(adapter) = factory.EnumAdapters1(index) else {
                break;
            };
            let desc = adapter.GetDesc1()?;
            let found = Adapter {
                name: from_wide(&desc.Description),
                vendor: desc.VendorId,
                dedicated_memory: desc.DedicatedVideoMemory as u64,
                software: desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
                uma: uma(&adapter),
            };
            if render.is_none() && adapter.EnumOutputs(0).is_ok() {
                render = Some(found.clone());
            }
            all.push(found);
        }
        Ok((all, render))
    }
}
fn uma(adapter: &IDXGIAdapter1) -> Option<bool> {
    // SAFETY: `adapter` is live; the feature struct is the size passed and outlives the call.
    unsafe {
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, &mut device).ok()?;
        let mut data = D3D12_FEATURE_DATA_ARCHITECTURE::default();
        device?
            .CheckFeatureSupport(
                D3D12_FEATURE_ARCHITECTURE,
                (&mut data as *mut D3D12_FEATURE_DATA_ARCHITECTURE).cast(),
                size_of::<D3D12_FEATURE_DATA_ARCHITECTURE>() as u32,
            )
            .ok()?;
        Some(data.UMA.as_bool())
    }
}

#[cfg(test)]
mod tests {
    use butterpollo_core::child_session::{Dpapi, PasswordStore, Secret, SystemSettings};

    #[test]
    fn the_edition_is_readable() {
        // Every Windows install names its edition; the mode decision depends on it.
        assert!(!super::edition_id().is_empty());
    }

    #[test]
    fn dpapi_roundtrips_for_this_user() {
        let blob = super::UserDpapi.protect(b"not a real password").unwrap();
        assert!(!blob.windows(19).any(|w| w == b"not a real password"));
        assert_eq!(
            super::UserDpapi.unprotect(&blob).unwrap().as_slice(),
            b"not a real password"
        );
        let folder = tempfile::tempdir().unwrap();
        let store = PasswordStore::new(folder.path().join("password.bin"), super::UserDpapi);
        store.save(&Secret::new("also not real".into())).unwrap();
        assert_eq!(store.load().unwrap().unwrap().expose(), "also not real");
        assert!(store.forget().unwrap());
    }

    #[test]
    fn reading_settings_changes_nothing() {
        // Read-only: this PC's registry, RDP and firewall settings stay as they are.
        let settings = super::NativeSettings::new().unwrap();
        let value = settings
            .read_dword(
                &butterpollo_core::child_session::Hive::LocalMachine,
                butterpollo_core::child_session::TERMINAL_SERVER,
                "fDenyTSConnections",
            )
            .unwrap();
        assert!(matches!(value, None | Some(0) | Some(1)));
        assert_eq!(
            settings
                .read_dword(
                    &butterpollo_core::child_session::Hive::LocalMachine,
                    r"SOFTWARE\Rubylight\No\Such\Key",
                    "Missing",
                )
                .unwrap(),
            None
        );
        settings.child_sessions_enabled().unwrap();
        let (adapters, _) = super::adapters().unwrap();
        assert!(!adapters.is_empty());
    }
}
