//! "Start Nightjar when I sign in": the HKCU Run value on Windows, an XDG autostart entry on Linux,
//! and the Background portal in a Flatpak, whose sandbox has neither the host's autostart folder nor
//! a command the host could run directly. [`Writer`] keeps tests off the real key and home folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The app id, as the Flatpak and the `.desktop` file name it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const APP_ID: &str = "io.github.ramazankara.nightjar";

/// Where this system keeps the sign-in start.
pub trait Writer: Send + Sync {
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
    format!("\"{}\"", exe.display())
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
        exec: exec_field(&exe?),
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
        self.path.try_exists().map_err(|e| format!("{}: {e}", self.path.display()))
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

    impl super::Writer for RunKey {
        fn is_on(&self) -> Result<bool, String> {
            let mut size = 0u32;
            // SAFETY: constant strings, and the out size lives across the call.
            let answer = unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN, VALUE, RRF_RT_REG_SZ, None, None, Some(&mut size)) };
            if answer == ERROR_FILE_NOT_FOUND {
                return Ok(false);
            }
            checked(answer).map(|()| true)
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
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    const DESTINATION: &str = "org.freedesktop.portal.Desktop";

    /// `org.freedesktop.portal.Background`: asks for (or takes back) the start at sign-in. The
    /// desktop may ask the user first, so this waits for its answer; call it off the UI thread.
    pub fn request_background(on: bool) -> Result<(), String> {
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
        options.insert("commandline", Value::from(vec!["nightjar"]));
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
    fn the_run_value_quotes_a_path_with_spaces() {
        assert_eq!(run_command(Path::new(r"C:\Program Files\Nightjar\nightjar.exe")), r#""C:\Program Files\Nightjar\nightjar.exe""#);
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
        assert!(text.starts_with("[Desktop Entry]\n") && text.contains("Exec=\"/usr/bin/nightjar\"\n"), "{text}");
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
