// Included by the crates' build scripts. Every binary Rubylight ships carries
// a Windows VERSIONINFO resource: Explorer and Task Manager show its
// description, and SignPath refuses to sign a release binary unless its
// ProductName is "Rubylight" and its ProductVersion is the release version
// (see .signpath/ and docs/code-signing.md).

/// The release version: the workspace version in the repository's
/// Cargo.toml. The display driver is outside the workspace and has its own
/// crate version, so the version comes from here rather than from Cargo. The repository is the nearest folder above
/// the crate that holds this file.
fn release_version() -> String {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repository = manifest
        .ancestors()
        .find(|folder| folder.join("rust/assets/version_info.rs").is_file())
        .expect("the repository holding rust/assets/version_info.rs");
    let cargo = repository.join("Cargo.toml");
    println!("cargo:rerun-if-changed={}", cargo.display());
    let text = std::fs::read_to_string(&cargo).unwrap();
    text.lines()
        .find_map(|line| line.strip_prefix("version = \"")?.strip_suffix('"'))
        .expect("the workspace version in Cargo.toml")
        .to_owned()
}

/// A VERSIONINFO resource script for one binary. `dll` sets the file type.
fn version_resource(description: &str, dll: bool) -> String {
    let version = release_version();
    // 2.4.0-rc.1 is 2,4,0,0 in the numeric fields; the strings keep it whole.
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u16>().unwrap_or(0));
    let [major, minor, patch] = [(); 3].map(|_| parts.next().unwrap_or(0));
    let numbers = format!("{major},{minor},{patch},0");
    let file_type = if dll { 2 } else { 1 };
    [
        "1 VERSIONINFO".to_owned(),
        format!("FILEVERSION {numbers}"),
        format!("PRODUCTVERSION {numbers}"),
        "FILEOS 0x40004".to_owned(),
        format!("FILETYPE {file_type}"),
        "BEGIN".to_owned(),
        "BLOCK \"StringFileInfo\"".to_owned(),
        "BEGIN".to_owned(),
        "BLOCK \"040904B0\"".to_owned(),
        "BEGIN".to_owned(),
        format!("VALUE \"FileDescription\", \"{description}\""),
        format!("VALUE \"FileVersion\", \"{version}\""),
        "VALUE \"LegalCopyright\", \"Ramazan Kara and contributors, GPL-3.0\"".to_owned(),
        "VALUE \"ProductName\", \"Rubylight\"".to_owned(),
        format!("VALUE \"ProductVersion\", \"{version}\""),
        "END".to_owned(),
        "END".to_owned(),
        "BLOCK \"VarFileInfo\"".to_owned(),
        "BEGIN".to_owned(),
        "VALUE \"Translation\", 0x409, 1200".to_owned(),
        "END".to_owned(),
        "END".to_owned(),
    ]
    .join("\n")
        + "\n"
}

/// Writes `name`.rc to OUT_DIR, `extra` (an icon line, say) followed by the
/// version resource, and returns its path for embed_resource.
fn write_resource(name: &str, extra: &str, description: &str, dll: bool) -> std::path::PathBuf {
    let rc = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join(format!("{name}.rc"));
    std::fs::write(&rc, format!("{extra}{}", version_resource(description, dll))).unwrap();
    rc
}
