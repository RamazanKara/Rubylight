//! Which Windows service this PC's Rubylight runs as. A normal installation
//! keeps `ApolloService`, the name Apollo gave it, so upgrades stay in place.
//! One installed beside another host, which may own that name, runs as
//! `RubylightService`. Setup records that in HKLM, and everything that names
//! the service reads the record: setup, the launcher and the scripts
//! (rust/service.ps1, rust/release/elevated.ps1, rust/tests/collect_environment.ps1).
//! The service executable itself names nobody: for an own-process service
//! Windows ignores the name given to its dispatcher and control handler, and
//! rust/service stays byte for byte the same (rust/release/service_inputs.py).

use std::path::{Path, PathBuf};

/// The registry key under HKLM that holds the record.
pub const KEY: &str = "SOFTWARE\\Rubylight";
/// The record's value. Absent in a normal installation.
pub const VALUE: &str = "ServiceName";
/// The service of a normal installation.
pub const DEFAULT: &str = "ApolloService";
/// The service of a side-by-side installation.
pub const SIDE_BY_SIDE: &str = "RubylightService";
/// The value of a service's key ([`service_key`]) that says what it runs.
pub const IMAGE_PATH: &str = "ImagePath";

/// The service a record names: anything setup would not have written counts
/// as no record, so nothing else can be named as Rubylight's.
pub fn from_record(recorded: Option<&str>) -> &'static str {
    match recorded {
        Some(name) if name.trim().eq_ignore_ascii_case(SIDE_BY_SIDE) => SIDE_BY_SIDE,
        _ => DEFAULT,
    }
}
/// The service this PC's Rubylight runs as; `program` says what a service
/// runs. A lost record must not turn a side-by-side installation into one that
/// removes the host it was installed beside, or that asks for that host's
/// service: its own service still says what it is.
pub fn service_named(
    recorded: Option<&str>,
    program: impl Fn(&str) -> Option<PathBuf>,
) -> &'static str {
    match from_record(recorded) {
        DEFAULT if program(SIDE_BY_SIDE).is_some_and(|p| our_service(&p)) => SIDE_BY_SIDE,
        named => named,
    }
}
/// Whether a service runs Rubylight's program. Another host may hold the name
/// `ApolloService`; only this says it is Rubylight's to stop, change or delete.
pub fn our_service(program: &Path) -> bool {
    program
        .to_string_lossy()
        .rsplit(['\\', '/'])
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("butterpollo-service.exe"))
}
/// A service's registry key under HKLM.
pub fn service_key(name: &str) -> String {
    format!("SYSTEM\\CurrentControlSet\\Services\\{name}")
}
/// The program a service's `ImagePath` runs.
pub fn image_program(image: &str) -> PathBuf {
    let image = image.trim();
    let program = match image.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => image.split(" -").next().unwrap_or(image).trim(),
    };
    PathBuf::from(program)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_side_by_side_name_changes_the_service() {
        assert_eq!(from_record(None), DEFAULT);
        assert_eq!(from_record(Some("")), DEFAULT);
        assert_eq!(from_record(Some("SunshineService")), DEFAULT);
        assert_eq!(from_record(Some("RubylightService")), SIDE_BY_SIDE);
        assert_eq!(from_record(Some(" rubylightservice ")), SIDE_BY_SIDE);
    }
    #[test]
    fn the_record_or_rubylights_own_service_names_the_service() {
        let none = |_: &str| None;
        assert_eq!(service_named(None, none), DEFAULT);
        assert_eq!(service_named(Some("RubylightService"), none), SIDE_BY_SIDE);
        // The record lost or malformed: Rubylight's service still says.
        let side = |name: &str| {
            (name == SIDE_BY_SIDE).then(|| PathBuf::from(r"C:\Rubylight\butterpollo-service.exe"))
        };
        assert_eq!(service_named(None, side), SIDE_BY_SIDE);
        assert_eq!(service_named(Some("rubbish"), side), SIDE_BY_SIDE);
        // Someone else's service of that name says nothing.
        let other = |_: &str| Some(PathBuf::from(r"C:\Other\other.exe"));
        assert_eq!(service_named(None, other), DEFAULT);
    }
    #[test]
    fn a_service_is_ours_by_the_program_its_image_path_names() {
        for image in [
            r#""C:\Program Files\Rubylight\butterpollo-service.exe""#,
            r#""C:\Program Files\Rubylight\Butterpollo-Service.EXE" --flag"#,
            r"C:\Rubylight\butterpollo-service.exe -x",
        ] {
            assert!(our_service(&image_program(image)), "{image}");
        }
        for image in [
            r#""C:\Vibepollo\tools\sunshinesvc.exe""#,
            r"C:\butterpollo-service.exe\other.exe",
            "",
        ] {
            assert!(!our_service(&image_program(image)), "{image}");
        }
    }
}
