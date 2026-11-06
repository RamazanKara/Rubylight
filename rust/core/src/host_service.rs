//! Which Windows service this PC's Rubylight runs as. A normal installation
//! keeps `ApolloService`, the name Apollo gave it, so upgrades stay in place.
//! One installed beside another host, which may own that name, runs as
//! `RubylightService`. Setup records that in HKLM, and everything that names
//! the service reads the record: setup, the launcher and the scripts
//! (rust/service.ps1, rust/release/elevated.ps1, rust/tests/collect_environment.ps1).
//! The service executable itself names nobody: for an own-process service
//! Windows ignores the name given to its dispatcher and control handler, and
//! rust/service stays byte for byte the same (rust/release/service_inputs.py).

/// The registry key under HKLM that holds the record.
pub const KEY: &str = "SOFTWARE\\Rubylight";
/// The record's value. Absent in a normal installation.
pub const VALUE: &str = "ServiceName";
/// The service of a normal installation.
pub const DEFAULT: &str = "ApolloService";
/// The service of a side-by-side installation.
pub const SIDE_BY_SIDE: &str = "RubylightService";

/// The service a record names: anything setup would not have written counts
/// as no record, so nothing else can be named as Rubylight's.
pub fn from_record(recorded: Option<&str>) -> &'static str {
    match recorded {
        Some(name) if name.trim().eq_ignore_ascii_case(SIDE_BY_SIDE) => SIDE_BY_SIDE,
        _ => DEFAULT,
    }
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
}
