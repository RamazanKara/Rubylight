//! Windows Advanced Color ICC associations, owned by the last streaming lease.
//! ABI: https://learn.microsoft.com/windows/win32/api/icm/nf-icm-colorprofilegetdisplaydefault

use crate::display::Monitor;
use anyhow::{Context, Result, bail};
use libloading::Library;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};
use windows::{
    Win32::Foundation::{HLOCAL, LUID, LocalFree},
    core::HRESULT,
};

type Get = unsafe extern "system" fn(u32, LUID, u32, u32, u32, *mut *mut u16) -> i32;
type Add = unsafe extern "system" fn(u32, *const u16, LUID, u32, i32, i32) -> i32;
type Remove = unsafe extern "system" fn(u32, *const u16, LUID, u32, i32) -> i32;
struct Api {
    _library: Library,
    get: Get,
    add: Add,
    remove: Remove,
}
impl Api {
    fn load() -> Result<Self> {
        let directory = std::env::var_os("SystemRoot").context("Windows directory unavailable")?;
        // SAFETY: the Get, Add and Remove types match mscms.dll's exported signatures, and
        // `library` is kept in `Self` for as long as the copied function pointers.
        unsafe {
            let library = Library::new(Path::new(&directory).join("System32").join("mscms.dll"))?;
            Ok(Self {
                get: *library.get(b"ColorProfileGetDisplayDefault\0")?,
                add: *library.get(b"ColorProfileAddDisplayAssociation\0")?,
                remove: *library.get(b"ColorProfileRemoveDisplayAssociation\0")?,
                _library: library,
            })
        }
    }
}
/// A display's advanced-colour profile associations: mscms, or a fake in tests.
trait Associations {
    fn get(&self, monitor: &Monitor, system: bool) -> Result<Option<String>>;
    fn add(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()>;
    fn remove(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()>;
}
impl Associations for Api {
    fn get(&self, monitor: &Monitor, system: bool) -> Result<Option<String>> {
        // SAFETY: `pointer` receives a NUL-terminated name from LocalAlloc, read only up to its
        // terminator and freed once with LocalFree.
        unsafe {
            let mut pointer = std::ptr::null_mut();
            let status = HRESULT((self.get)(
                scope(system),
                monitor.adapter,
                monitor.source,
                0,
                8,
                &mut pointer,
            ));
            // Windows returns no association either as a null name or NOT_FOUND.
            if pointer.is_null() {
                if status.0 == 0x80070490u32 as i32 || status.0 == 0x80070002u32 as i32 {
                    return Ok(None);
                }
                status.ok()?;
                return Ok(None);
            }
            let result = (|| {
                status.ok()?;
                let length = (0..32768)
                    .find(|&n| *pointer.add(n) == 0)
                    .context("color profile name is too long")?;
                let name = String::from_utf16(std::slice::from_raw_parts(pointer, length))?;
                Ok((!name.is_empty()).then_some(name))
            })();
            let _ = LocalFree(Some(HLOCAL(pointer.cast())));
            result
        }
    }
    fn add(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()> {
        let name = wide(name)?;
        // SAFETY: `name` is NUL-terminated and outlives the call, whose type matches the export.
        unsafe {
            HRESULT((self.add)(
                scope(system),
                name.as_ptr(),
                monitor.adapter,
                monitor.source,
                1,
                1,
            ))
            .ok()?;
        }
        Ok(())
    }
    fn remove(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()> {
        let name = wide(name)?;
        // SAFETY: `name` is NUL-terminated and outlives the call, whose type matches the export.
        unsafe {
            HRESULT((self.remove)(
                scope(system),
                name.as_ptr(),
                monitor.adapter,
                monitor.source,
                1,
            ))
            .ok()?;
        }
        Ok(())
    }
}
fn scope(system: bool) -> u32 {
    if system { 0 } else { 1 }
}
fn wide(name: &str) -> Result<Vec<u16>> {
    if name.is_empty() || name.len() > 32767 || name.contains('\0') {
        bail!("invalid color profile name");
    }
    Ok(name.encode_utf16().chain(Some(0)).collect())
}
fn filename(selection: &str) -> Result<&str> {
    if selection.is_empty()
        || selection.contains(['/', '\\', ':', '\0'])
        || matches!(selection, "." | "..")
    {
        bail!("color profile must be an installed filename");
    }
    Ok(selection)
}
fn installed(selection: &str) -> Result<String> {
    let selection = filename(selection)?;
    let root = std::env::var_os("SystemRoot").context("Windows directory unavailable")?;
    let directory = PathBuf::from(root)
        .join("System32")
        .join("spool")
        .join("drivers")
        .join("color");
    let names = if Path::new(selection).extension().is_none() {
        vec![
            selection.into(),
            format!("{selection}.icm"),
            format!("{selection}.icc"),
        ]
    } else {
        vec![selection.into()]
    };
    names
        .into_iter()
        .find(|name| directory.join(name).is_file())
        .context("selected ICC profile is not installed")
}
/// MHC2 stores the calibration peak as signed 16.16 nits, in big endian.
pub fn peak_luminance(selection: &str) -> Result<Option<u32>> {
    // Effective settings are polled during streaming. A calibration file is
    // normally unchanged; avoid repeated file reads on the encode thread.
    type Peaks = BTreeMap<String, (std::time::Instant, Option<u32>)>;
    static PEAKS: std::sync::OnceLock<Mutex<Peaks>> = std::sync::OnceLock::new();
    let cache = PEAKS.get_or_init(Mutex::default);
    if let Some((at, peak)) = cache.lock().unwrap().get(selection)
        && at.elapsed() < std::time::Duration::from_secs(30)
    {
        return Ok(*peak);
    }
    let name = installed(selection)?;
    let path =
        PathBuf::from(std::env::var_os("SystemRoot").context("Windows directory unavailable")?)
            .join("System32/spool/drivers/color")
            .join(name);
    let mut file = std::fs::File::open(path)?;
    if !(132..=32 * 1024 * 1024).contains(&file.metadata()?.len()) {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(&mut file, 32 * 1024 * 1024 + 1),
        &mut bytes,
    )?;
    let peak = mhc2_peak(&bytes);
    let mut cache = cache.lock().unwrap();
    if cache.len() >= 128 {
        cache.clear();
    }
    cache.insert(selection.into(), (std::time::Instant::now(), peak));
    Ok(peak)
}
fn mhc2_peak(bytes: &[u8]) -> Option<u32> {
    if !(132..=32 * 1024 * 1024).contains(&bytes.len()) {
        return None;
    }
    let word = |offset: usize| -> Option<u32> {
        Some(u32::from_be_bytes(
            bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
        ))
    };
    let count = word(128)? as usize;
    if count > (bytes.len() - 132) / 12 {
        return None;
    }
    for index in 0..count {
        let entry = 132 + index * 12;
        if bytes.get(entry..entry + 4)? != b"MHC2" {
            continue;
        }
        let offset = word(entry + 4)? as usize;
        let size = word(entry + 8)? as usize;
        if size < 20
            || offset.checked_add(size)? > bytes.len()
            || bytes.get(offset..offset.checked_add(4)?)? != b"MHC2"
        {
            return None;
        }
        let peak = word(offset + 16)? as i32 as f64 / 65536.;
        return (peak > 0. && peak <= 100000.).then(|| peak.round() as u32);
    }
    None
}
/// A Windows HDR Calibration profile: a display's advanced-colour profile
/// with the MHC2 tag the calibration app writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calibration {
    /// The physical display it calibrates.
    pub display: String,
    pub profile: String,
    pub peak: u32,
}
/// The calibration of the physical HDR display a virtual display stands in
/// for, the primary display first. Read before the virtual display exists,
/// while the physical displays are on. A SYSTEM host reads the signed-in
/// user's associations as that user.
pub fn calibration() -> Result<Option<Calibration>> {
    let api = Api::load()?;
    let displays: Vec<_> = crate::display::monitors()?
        .into_iter()
        .filter(|monitor| monitor.hdr_enabled && !monitor.is_rubylight())
        .collect();
    crate::user_folder::as_session_user(|| {
        find_calibration(&api, &displays, |name| {
            peak_luminance(name)
                .inspect_err(|error| tracing::warn!(%error, profile = name, "HDR calibration profile unreadable"))
                .ok()
                .flatten()
        })
    })
    .context("the signed-in user's display profiles are unreadable")?
}
/// The first display with a calibration, primary first: the user's
/// association before the system's, as Windows shows a user's own choice.
fn find_calibration(
    api: &impl Associations,
    displays: &[Monitor],
    peak: impl Fn(&str) -> Option<u32>,
) -> Result<Option<Calibration>> {
    let mut ordered: Vec<_> = displays.iter().collect();
    ordered.sort_by_key(|monitor| !monitor.primary);
    for monitor in ordered {
        for system in [false, true] {
            let Some(profile) = api.get(monitor, system)? else {
                continue;
            };
            if let Some(peak) = peak(&profile) {
                return Ok(Some(Calibration {
                    display: if monitor.friendly_name.is_empty() {
                        monitor.display_name.clone()
                    } else {
                        monitor.friendly_name.clone()
                    },
                    profile,
                    peak,
                }));
            }
        }
    }
    Ok(None)
}
struct State {
    users: usize,
    previous: Option<String>,
    applied: String,
    system: bool,
}
static LEASES: Mutex<BTreeMap<String, State>> = Mutex::new(BTreeMap::new());
pub struct Lease {
    identity: String,
}
impl Lease {
    pub fn acquire(output: &str, selection: &str) -> Result<Self> {
        let applied = installed(selection)?;
        let monitor = crate::display::monitors()?
            .into_iter()
            .find(|m| m.display_name == output || m.device_id == output)
            .context("ICC output disappeared")?;
        let mut leases = LEASES.lock().unwrap();
        if let Some(state) = leases.get_mut(&monitor.device_id) {
            if !state.applied.eq_ignore_ascii_case(&applied) {
                bail!("another client owns a different ICC profile on this output");
            }
            state.users += 1;
        } else {
            let api = Api::load()?;
            let system = crate::process::is_system();
            let current = api.get(&monitor, system)?;
            let previous = previous_profile(
                crate::display_recovery::pending(&monitor.device_id).profile,
                current.clone(),
            );
            if !current
                .as_ref()
                .is_some_and(|name| name.eq_ignore_ascii_case(&applied))
            {
                crate::display_recovery::profile(
                    &monitor.device_id,
                    &monitor.display_name,
                    current,
                    &applied,
                    system,
                )?;
                // Preserve the recovery entry if a failed call partially changes Windows.
                if let Err(error) = api.add(&monitor, system, &applied) {
                    if restore(&monitor, previous.as_deref(), &applied, system).is_ok() {
                        let _ = crate::display_recovery::release_profile(&monitor.device_id);
                    }
                    return Err(error);
                }
            }
            leases.insert(
                monitor.device_id.clone(),
                State {
                    users: 1,
                    previous,
                    applied,
                    system,
                },
            );
        }
        Ok(Self {
            identity: monitor.device_id,
        })
    }
}
/// The profile to put back after streaming. A display that was away when an
/// earlier stream ended still has that stream's profile, and its journal
/// entry holds the original: that wins, or this lease's release would clear
/// the entry and leave the stream's profile on the display for good.
fn previous_profile(pending: Option<Option<String>>, current: Option<String>) -> Option<String> {
    pending.unwrap_or(current)
}
/// Used by the separate recovery process as well as normal lease teardown.
pub fn restore(
    monitor: &Monitor,
    previous: Option<&str>,
    applied: &str,
    system: bool,
) -> Result<()> {
    if previous.is_some_and(|name| name.eq_ignore_ascii_case(applied)) {
        return Ok(());
    }
    restore_with(&Api::load()?, monitor, previous, applied, system)
}
/// Puts `previous` back, or removes `applied`, unless the display has since
/// been given another profile.
fn restore_with(
    api: &impl Associations,
    monitor: &Monitor,
    previous: Option<&str>,
    applied: &str,
    system: bool,
) -> Result<()> {
    if previous.is_some_and(|name| name.eq_ignore_ascii_case(applied)) {
        return Ok(());
    }
    if api
        .get(monitor, system)?
        .as_deref()
        .is_some_and(|current| current.eq_ignore_ascii_case(applied))
    {
        if let Some(previous) = previous {
            api.add(monitor, system, previous)?;
        } else {
            api.remove(monitor, system, applied)?;
        }
    }
    Ok(())
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut leases = LEASES.lock().unwrap();
        let Some(state) = leases.get_mut(&self.identity) else {
            return;
        };
        state.users -= 1;
        if state.users != 0 {
            return;
        }
        let state = leases.remove(&self.identity).unwrap();
        let result = (|| {
            // Windows keeps the profile with the display. One that is off or
            // unplugged now keeps its journal entry for the next recovery.
            let Some(monitor) = crate::display::monitors()?
                .into_iter()
                .find(|m| m.device_id == self.identity)
            else {
                tracing::info!(display = %self.identity, "display is gone; its colour profile is restored when it returns");
                return Ok(());
            };
            restore(
                &monitor,
                state.previous.as_deref(),
                &state.applied,
                state.system,
            )?;
            crate::display_recovery::release_profile(&self.identity)
        })();
        if let Err(error) = result {
            tracing::warn!(%error, "HDR profile restoration deferred to recovery");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    /// Associations by display and scope (system or the user's).
    #[derive(Default)]
    struct Fake(RefCell<BTreeMap<(String, bool), String>>);
    impl Fake {
        fn with(self, monitor: &Monitor, system: bool, name: &str) -> Self {
            self.add(monitor, system, name).unwrap();
            self
        }
    }
    impl Associations for Fake {
        fn get(&self, monitor: &Monitor, system: bool) -> Result<Option<String>> {
            Ok(self
                .0
                .borrow()
                .get(&(monitor.device_id.clone(), system))
                .cloned())
        }
        fn add(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()> {
            self.0
                .borrow_mut()
                .insert((monitor.device_id.clone(), system), name.into());
            Ok(())
        }
        fn remove(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()> {
            let mut associations = self.0.borrow_mut();
            let key = (monitor.device_id.clone(), system);
            anyhow::ensure!(
                associations
                    .get(&key)
                    .is_some_and(|current| current.eq_ignore_ascii_case(name)),
                "{name} is not associated"
            );
            associations.remove(&key);
            Ok(())
        }
    }
    fn monitor(id: &str, primary: bool) -> Monitor {
        Monitor {
            device_id: id.into(),
            monitor_device_path: String::new(),
            display_name: format!(r"\\.\{id}"),
            friendly_name: format!("{id} panel"),
            hdr_supported: true,
            hdr_enabled: true,
            primary,
            adapter: LUID::default(),
            target: 0,
            source: 0,
        }
    }
    /// Profiles named `*-cal.icm` carry an MHC2 calibration.
    fn peak(name: &str) -> Option<u32> {
        name.ends_with("-cal.icm").then_some(800)
    }
    #[test]
    fn the_physical_calibration_is_the_primary_displays_and_the_users_own_first() -> Result<()> {
        let (side, main) = (monitor("side", false), monitor("main", true));
        let displays = [side.clone(), main.clone()];
        // A profile without MHC2 is no calibration.
        let fake = Fake::default()
            .with(&side, false, "side-cal.icm")
            .with(&main, true, "srgb.icm");
        let found = find_calibration(&fake, &displays, peak)?.expect("side calibration");
        assert_eq!(
            (found.display.as_str(), found.profile.as_str(), found.peak),
            ("side panel", "side-cal.icm", 800)
        );
        let fake = fake
            .with(&main, true, "system-cal.icm")
            .with(&main, false, "user-cal.icm");
        assert_eq!(
            find_calibration(&fake, &displays, peak)?.map(|found| found.profile),
            Some("user-cal.icm".into())
        );
        let fake = Fake::default().with(&main, false, "srgb.icm");
        assert_eq!(find_calibration(&fake, &displays, peak)?, None);
        assert_eq!(find_calibration(&fake, &[], peak)?, None);
        Ok(())
    }
    #[test]
    fn the_virtual_display_gets_back_what_it_had_unless_someone_chose_another() -> Result<()> {
        let display = monitor("virtual", false);
        // None before: the calibration is removed again.
        let fake = Fake::default().with(&display, true, "main-cal.icm");
        restore_with(&fake, &display, None, "main-cal.icm", true)?;
        assert_eq!(fake.get(&display, true)?, None);
        // A profile before comes back.
        let fake = Fake::default().with(&display, true, "main-cal.icm");
        restore_with(&fake, &display, Some("old.icm"), "main-cal.icm", true)?;
        assert_eq!(fake.get(&display, true)?, Some("old.icm".into()));
        // One chosen while streaming stays, in either scope.
        let fake =
            Fake::default()
                .with(&display, true, "mine.icm")
                .with(&display, false, "main-cal.icm");
        restore_with(&fake, &display, None, "main-cal.icm", true)?;
        assert_eq!(fake.get(&display, true)?, Some("mine.icm".into()));
        assert_eq!(fake.get(&display, false)?, Some("main-cal.icm".into()));
        // Nothing changed: nothing to put back.
        restore_with(&fake, &display, Some("MINE.icm"), "mine.icm", true)?;
        assert_eq!(fake.get(&display, true)?, Some("mine.icm".into()));
        Ok(())
    }
    #[test]
    fn mhc2_peak_reads_calibration_and_rejects_truncated_or_negative_tags() {
        let mut bytes = vec![0; 164];
        bytes[128..132].copy_from_slice(&1u32.to_be_bytes());
        bytes[132..136].copy_from_slice(b"MHC2");
        bytes[136..140].copy_from_slice(&144u32.to_be_bytes());
        bytes[140..144].copy_from_slice(&20u32.to_be_bytes());
        bytes[144..148].copy_from_slice(b"MHC2");
        bytes[160..164].copy_from_slice(&(1234i32 * 65536 + 32768).to_be_bytes());
        assert_eq!(mhc2_peak(&bytes), Some(1235));
        assert_eq!(mhc2_peak(&bytes[..163]), None);
        bytes[160..164].copy_from_slice(&(-65536i32).to_be_bytes());
        assert_eq!(mhc2_peak(&bytes), None);
        bytes[136..140].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(mhc2_peak(&bytes), None);
    }
    #[test]
    fn a_pending_original_profile_wins_over_the_one_left_on_the_display() {
        let left = Some("hdr.icm".to_string());
        assert_eq!(previous_profile(Some(None), left.clone()), None);
        assert_eq!(
            previous_profile(Some(Some("calibrated.icm".into())), left.clone()),
            Some("calibrated.icm".into())
        );
        assert_eq!(previous_profile(None, left.clone()), left);
    }
    #[test]
    fn installed_profile_selection_cannot_escape_the_color_directory() {
        for bad in [
            "",
            ".",
            "..",
            "..\\other.icc",
            "C:\\other.icc",
            "other:stream",
            "a/b.icc",
            "a\0b.icc",
        ] {
            assert!(filename(bad).is_err());
        }
        assert_eq!(
            filename("My HDR Profile.icc").unwrap(),
            "My HDR Profile.icc"
        );
    }
}
