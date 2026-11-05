//! "Start Nightjar when I sign in": the HKCU Run value on Windows, an XDG autostart entry on Linux,
//! and the Background portal in a Flatpak, whose sandbox has neither the host's autostart folder nor
//! a command the host could run directly. [`Writer`] keeps tests off the real key and home folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The app id, as the Flatpak and the `.desktop` file name it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const APP_ID: &str = "io.github.ramazankara.nightjar";

/// On the command the system starts at sign-in: the network may still be on its way.
const SIGN_IN_ARG: &str = "--autostart";

/// Takes the sign-in start's word out of the command line, and says whether it was there.
pub fn split_sign_in(mut args: Vec<std::ffi::OsString>) -> (Vec<std::ffi::OsString>, bool) {
    let before = args.len();
    args.retain(|arg| arg != SIGN_IN_ARG);
    let found = args.len() != before;
    (args, found)
}

/// Where this system keeps the sign-in start.
pub trait Writer: Send + Sync {
    /// Whether this copy starts at sign-in. An entry of this copy that no longer says what it should
    /// (another version moved it, or wrote other arguments) is brought up to date on the way.
    fn is_on(&self) -> Result<bool, String>;
    fn set(&self, on: bool) -> Result<(), String>;
}

/// This system's writer; None where Nightjar has no way to set it.
#[cfg(windows)]
pub fn system(_config_dir: &Path) -> Option<Arc<dyn Writer>> {
    let exe = std::env::current_exe().ok()?;
    Some(Arc::new(run_key::RunKey { command: run_command(&exe) }))
}

#[cfg(target_os = "linux")]
pub fn system(config_dir: &Path) -> Option<Arc<dyn Writer>> {
    linux_writer(
        Path::new("/.flatpak-info").exists(),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("APPIMAGE").map(PathBuf::from).or_else(|| std::env::current_exe().ok()),
        config_dir,
        portal::request_background,
    )
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn system(_config_dir: &Path) -> Option<Arc<dyn Writer>> {
    None
}

/// The Run value's command line: the path quoted, as Windows wants one with spaces.
#[cfg_attr(not(windows), allow(dead_code))]
fn run_command(exe: &Path) -> String {
    format!("\"{}\" {SIGN_IN_ARG}", exe.display())
}

/// `work`'s answer, or None if it takes longer than `limit`; it then ends on its own, unheard.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn within<T: Send + 'static>(limit: Duration, work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (sender, answer) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    answer.recv_timeout(limit).ok()
}

/// What the saved entry is to this copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Saved {
    Off,
    Current,
    /// This program's with other words, or of a program that is gone: it is written again.
    Stale,
    /// Another copy's, which still exists: not this copy's to take without the player asking.
    Others,
}

fn compare(saved: Option<&str>, command: &str, program_of: fn(&str) -> Option<String>, exists: impl Fn(&str) -> bool) -> Saved {
    let Some(saved) = saved else { return Saved::Off };
    if saved == command {
        return Saved::Current;
    }
    let same = |a: &str, b: &str| if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b };
    match (program_of(saved), program_of(command)) {
        (Some(theirs), Some(ours)) if same(&theirs, &ours) || !exists(&theirs) => Saved::Stale,
        _ => Saved::Others,
    }
}

/// `is_on`'s answer. A failed refresh leaves the entry on as it was, which the log says.
fn settle(saved: Saved, refresh: impl FnOnce() -> Result<(), String>) -> bool {
    match saved {
        Saved::Off | Saved::Others => false,
        Saved::Current => true,
        Saved::Stale => {
            if let Err(e) = refresh() {
                eprintln!("nightjar: refreshing the sign-in start failed: {e}");
            }
            true
        }
    }
}

/// The program of a Run value: quoted, or up to the first space.
#[cfg_attr(not(windows), allow(dead_code))]
fn run_program(command: &str) -> Option<String> {
    let command = command.trim();
    let program = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next(),
        None => command.split_whitespace().next(),
    };
    program.filter(|p| !p.is_empty()).map(str::to_owned)
}

/// The program of an `Exec` line, undoing [`exec_field`].
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn exec_program(exec: &str) -> Option<String> {
    let exec = exec.trim();
    let Some(quoted) = exec.strip_prefix('"') else {
        return exec.split_whitespace().next().map(|p| p.replace("%%", "%"));
    };
    let mut program = String::new();
    let mut chars = quoted.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(program).filter(|p| !p.is_empty()),
            '\\' => {
                // Both unescaping rounds of exec_field.
                let next = chars.next()?;
                let next = if next == '\\' { chars.next()? } else { next };
                program.push(next);
            }
            '%' => {
                chars.next();
                program.push('%');
            }
            c => program.push(c),
        }
    }
    None
}

/// The writer for a Linux session: the portal in a Flatpak, else a file in the autostart folder.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_writer(
    flatpak: bool,
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
    exe: Option<PathBuf>,
    config_dir: &Path,
    request: fn(bool) -> Result<(), String>,
) -> Option<Arc<dyn Writer>> {
    if flatpak {
        return Some(Arc::new(Portal { marker: config_dir.join("sign_in_portal"), request }));
    }
    let config = xdg_config_home.filter(|p| p.is_absolute()).or_else(|| home.map(|h| h.join(".config")))?;
    Some(Arc::new(XdgFile {
        path: config.join("autostart").join(format!("{APP_ID}.desktop")),
        exec: format!("{} {SIGN_IN_ARG}", exec_field(&exe?)),
    }))
}

/// A path as a `.desktop` file's `Exec` wants it: quoted, its specials escaped, `%` doubled.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn exec_field(exe: &Path) -> String {
    let mut quoted = String::from("\"");
    for c in exe.to_string_lossy().chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                // The key file format unescapes once before Exec's own quoting does.
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn desktop_entry(exec: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nName=Nightjar\nExec={exec}\nIcon={APP_ID}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n")
}

/// `~/.config/autostart/<app id>.desktop`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct XdgFile {
    path: PathBuf,
    exec: String,
}

impl Writer for XdgFile {
    fn is_on(&self) -> Result<bool, String> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", self.path.display())),
        };
        let saved = text.as_ref().map(|text| text.lines().find_map(|l| l.strip_prefix("Exec=")).unwrap_or(""));
        let state = compare(saved, &self.exec, exec_program, |p| Path::new(p).exists());
        Ok(settle(state, || self.set(true)))
    }

    fn set(&self, on: bool) -> Result<(), String> {
        let failed = |e: std::io::Error| format!("{}: {e}", self.path.display());
        if on {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir).map_err(failed)?;
            }
            std::fs::write(&self.path, desktop_entry(&self.exec)).map_err(failed)
        } else {
            match std::fs::remove_file(&self.path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(failed(e)),
                _ => Ok(()),
            }
        }
    }
}

/// In a Flatpak the portal writes the autostart entry on the host. It cannot be asked whether one is
/// set, so a marker in Nightjar's config folder remembers what it last said.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct Portal {
    marker: PathBuf,
    request: fn(bool) -> Result<(), String>,
}

impl Writer for Portal {
    /// Its command names no path, so it cannot go stale, and asking again could put a question up.
    fn is_on(&self) -> Result<bool, String> {
        self.marker.try_exists().map_err(|e| format!("{}: {e}", self.marker.display()))
    }

    fn set(&self, on: bool) -> Result<(), String> {
        (self.request)(on)?;
        let failed = |e: std::io::Error| format!("{}: {e}", self.marker.display());
        if on {
            std::fs::write(&self.marker, b"").map_err(failed)
        } else {
            match std::fs::remove_file(&self.marker) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(failed(e)),
                _ => Ok(()),
            }
        }
    }
}

#[cfg(windows)]
mod run_key {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
    use windows::Win32::System::Registry::{RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ};

    use super::{compare, run_program, settle};

    const RUN: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
    const VALUE: PCWSTR = w!("Nightjar");

    pub struct RunKey {
        pub command: String,
    }

    fn checked(error: WIN32_ERROR) -> Result<(), String> {
        if error == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(windows::core::Error::from_hresult(error.to_hresult()).message())
        }
    }

    fn saved() -> Result<Option<String>, String> {
        let mut size = 0u32;
        // SAFETY: constant strings, and the out size lives across the call.
        let answer = unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN, VALUE, RRF_RT_REG_SZ, None, None, Some(&mut size)) };
        if answer == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        checked(answer)?;
        let mut text = vec![0u16; (size as usize).div_ceil(2)];
        // SAFETY: as above, and the buffer holds `size` bytes.
        checked(unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN, VALUE, RRF_RT_REG_SZ, None, Some(text.as_mut_ptr().cast()), Some(&mut size)) })?;
        let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
        Ok(Some(String::from_utf16_lossy(&text[..end])))
    }

    impl super::Writer for RunKey {
        fn is_on(&self) -> Result<bool, String> {
            let saved = saved()?;
            let state = compare(saved.as_deref(), &self.command, run_program, |p| std::path::Path::new(p).exists());
            Ok(settle(state, || self.set(true)))
        }

        fn set(&self, on: bool) -> Result<(), String> {
            if !on {
                // SAFETY: constant strings.
                let answer = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, VALUE) };
                return if answer == ERROR_FILE_NOT_FOUND { Ok(()) } else { checked(answer) };
            }
            let text = HSTRING::from(self.command.as_str());
            let bytes = (text.len() + 1) * 2;
            // SAFETY: an HSTRING's buffer ends in a NUL, so `bytes` of it can be read.
            checked(unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, RUN, VALUE, REG_SZ.0, Some(text.as_ptr().cast()), bytes as u32) })
        }
    }
}

#[cfg(target_os = "linux")]
mod portal {
    use std::collections::HashMap;
    use std::time::Duration;
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    const DESTINATION: &str = "org.freedesktop.portal.Desktop";

    /// Long enough for a person to answer the desktop's question.
    const ANSWER_LIMIT: Duration = Duration::from_secs(120);

    /// `org.freedesktop.portal.Background`: asks for (or takes back) the start at sign-in. The
    /// desktop may ask the user first, so this waits for its answer, within reason; call it off the UI thread.
    pub fn request_background(on: bool) -> Result<(), String> {
        super::within(ANSWER_LIMIT, move || request_once(on)).unwrap_or_else(|| Err("The desktop portal did not answer.".into()))
    }

    fn request_once(on: bool) -> Result<(), String> {
        let failed = |e: zbus::Error| format!("The desktop portal did not answer: {e}");
        let connection = Connection::session().map_err(failed)?;
        let sender = connection.unique_name().ok_or("no D-Bus name")?.trim_start_matches(':').replace('.', "_");
        let token = format!("nightjar{}", std::process::id());
        // Listen before asking, so the answer cannot go by first.
        let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
        let request = Proxy::new(&connection, DESTINATION, path, "org.freedesktop.portal.Request").map_err(failed)?;
        let mut answers = request.receive_signal("Response").map_err(failed)?;
        let background = Proxy::new(&connection, DESTINATION, "/org/freedesktop/portal/desktop", "org.freedesktop.portal.Background").map_err(failed)?;
        let mut options: HashMap<&str, Value> = HashMap::new();
        options.insert("handle_token", Value::from(token.as_str()));
        options.insert("reason", Value::from("Start Nightjar when you sign in"));
        options.insert("autostart", Value::from(on));
        options.insert("commandline", Value::from(vec!["nightjar", super::SIGN_IN_ARG]));
        let _: OwnedObjectPath = background.call("RequestBackground", &("", options)).map_err(failed)?;
        let answer = answers.next().ok_or("the desktop portal closed without an answer")?;
        let (code, _results): (u32, HashMap<String, OwnedValue>) = answer.body().deserialize().map_err(failed)?;
        if code == 0 {
            Ok(())
        } else {
            Err("The desktop did not allow it.".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nightjar-autostart-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_run_value_quotes_a_path_with_spaces_and_marks_the_sign_in() {
        assert_eq!(run_command(Path::new(r"C:\Program Files\Nightjar\nightjar.exe")), r#""C:\Program Files\Nightjar\nightjar.exe" --autostart"#);
    }

    #[test]
    fn the_sign_in_word_leaves_the_command_line() {
        let args = |list: &[&str]| list.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        assert_eq!(split_sign_in(args(&[])), (args(&[]), false));
        assert_eq!(split_sign_in(args(&["--autostart"])), (args(&[]), true));
        assert_eq!(split_sign_in(args(&["--host", "x"])), (args(&["--host", "x"]), false));
    }

    #[test]
    fn an_entry_is_compared_with_the_command_of_this_copy() {
        let ours = r#""C:\N\nightjar.exe" --autostart"#;
        let exists = |p: &str| p != r"C:\Gone\nightjar.exe";
        let of = |saved| compare(saved, ours, run_program, exists);
        assert_eq!(of(None), Saved::Off);
        assert_eq!(of(Some(ours)), Saved::Current);
        assert_eq!(of(Some(r#""C:\N\nightjar.exe""#)), Saved::Stale, "an older version's words");
        assert_eq!(of(Some(r#""C:\Gone\nightjar.exe" --autostart"#)), Saved::Stale, "moved away");
        assert_eq!(of(Some(r#""C:\Other\nightjar.exe" --autostart"#)), Saved::Others);
        assert_eq!(of(Some("")), Saved::Others, "says nothing");
    }

    #[test]
    fn only_a_stale_entry_is_written_again_and_a_failure_leaves_it_on() {
        assert!(!settle(Saved::Off, || panic!("not written")));
        assert!(!settle(Saved::Others, || panic!("not written")));
        assert!(settle(Saved::Current, || panic!("not written")));
        let mut wrote = false;
        assert!(settle(Saved::Stale, || {
            wrote = true;
            Ok(())
        }));
        assert!(wrote);
        assert!(settle(Saved::Stale, || Err("no".into())));
    }

    #[test]
    fn a_run_value_and_an_exec_line_name_their_program() {
        assert_eq!(run_program(r#""C:\Program Files\N\nightjar.exe" --autostart"#).as_deref(), Some(r"C:\Program Files\N\nightjar.exe"));
        assert_eq!(run_program(r"C:\N\nightjar.exe --autostart").as_deref(), Some(r"C:\N\nightjar.exe"));
        assert_eq!(run_program(""), None);
        for path in ["/opt/my apps/nightjar", "/a$b/100%/c", "/a\"b`c\\d"] {
            assert_eq!(exec_program(&format!("{} --autostart", exec_field(Path::new(path)))).as_deref(), Some(path));
        }
        assert_eq!(exec_program("/usr/bin/nightjar --autostart").as_deref(), Some("/usr/bin/nightjar"));
        assert_eq!(exec_program("\"never closed"), None);
    }

    fn xdg_writer(config: &Path, exe: &str) -> Arc<dyn Writer> {
        linux_writer(false, Some(config.to_owned()), None, Some(exe.into()), config, |_| panic!("no portal")).unwrap()
    }

    #[test]
    fn a_stale_xdg_entry_is_brought_up_to_date_and_another_copys_is_left() {
        let config = folder("stale");
        let file = config.join("autostart").join("io.github.ramazankara.nightjar.desktop");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        // Absolute on every test machine, so `exists` answers the same.
        let other = std::env::current_exe().unwrap();
        let other = other.to_string_lossy().replace('\\', "/");
        std::fs::write(&file, desktop_entry(&exec_field(Path::new(&other)))).unwrap();
        let writer = xdg_writer(&config, "/old/copy/of/nightjar");
        assert!(!writer.is_on().unwrap(), "another copy's");
        assert!(std::fs::read_to_string(&file).unwrap().contains(&other), "and left alone");
        std::fs::write(&file, desktop_entry("\"/old/copy/of/nightjar\"")).unwrap();
        assert!(writer.is_on().unwrap());
        assert!(std::fs::read_to_string(&file).unwrap().contains("Exec=\"/old/copy/of/nightjar\" --autostart\n"), "refreshed");
        writer.set(true).unwrap();
        assert!(writer.is_on().unwrap());
        let _ = std::fs::remove_dir_all(&config);
    }

    #[test]
    fn an_answer_that_never_comes_does_not_hold_up_the_caller() {
        let (_keep, never) = std::sync::mpsc::channel::<()>();
        assert_eq!(within(Duration::from_millis(50), move || never.recv().is_ok()), None);
        assert_eq!(within(Duration::from_secs(30), || 7), Some(7));
    }

    #[test]
    fn the_exec_field_escapes_what_the_format_reads() {
        assert_eq!(exec_field(Path::new("/opt/my apps/nightjar")), "\"/opt/my apps/nightjar\"");
        assert_eq!(exec_field(Path::new("/a$b/100%/c")), "\"/a\\\\$b/100%%/c\"");
    }

    #[test]
    fn the_xdg_entry_appears_and_goes_in_the_given_folder_only() {
        let config = folder("xdg");
        let writer = linux_writer(false, Some(config.clone()), None, Some("/usr/bin/nightjar".into()), &config, |_| panic!("no portal")).unwrap();
        assert!(!writer.is_on().unwrap());
        writer.set(true).unwrap();
        assert!(writer.is_on().unwrap());
        let text = std::fs::read_to_string(config.join("autostart").join("io.github.ramazankara.nightjar.desktop")).unwrap();
        assert!(text.starts_with("[Desktop Entry]\n") && text.contains("Exec=\"/usr/bin/nightjar\" --autostart\n"), "{text}");
        writer.set(false).unwrap();
        assert!(!writer.is_on().unwrap());
        writer.set(false).expect("taking back what is gone is fine");
        let _ = std::fs::remove_dir_all(&config);
    }

    #[test]
    fn the_xdg_entry_falls_back_to_the_home_folder_and_needs_one() {
        let home = folder("home");
        let writer = linux_writer(false, Some("relative".into()), Some(home.clone()), Some("/n".into()), &home, |_| panic!("no portal")).unwrap();
        writer.set(true).unwrap();
        assert!(home.join(".config/autostart/io.github.ramazankara.nightjar.desktop").exists());
        let _ = std::fs::remove_dir_all(&home);
        assert!(linux_writer(false, None, None, Some("/n".into()), &home, |_| Ok(())).is_none());
    }

    #[test]
    fn a_flatpak_goes_through_the_portal_and_remembers_its_answer() {
        let config = folder("portal");
        std::fs::create_dir_all(&config).unwrap();
        let writer = linux_writer(true, None, None, None, &config, |_| Ok(())).unwrap();
        assert!(!writer.is_on().unwrap());
        writer.set(true).unwrap();
        assert!(writer.is_on().unwrap());
        writer.set(false).unwrap();
        assert!(!writer.is_on().unwrap());
        let refused = linux_writer(true, None, None, None, &config, |_| Err("no".into())).unwrap();
        assert_eq!(refused.set(true), Err("no".to_string()));
        assert!(!refused.is_on().unwrap(), "a refusal changes nothing");
        let _ = std::fs::remove_dir_all(&config);
    }
}
