// Included by the crates' build scripts. Every binary Rubylight ships carries
// a Windows VERSIONINFO resource: Explorer and Task Manager show its
// description, and SignPath refuses to sign a release binary unless its
// ProductName is "Rubylight" and its ProductVersion is the release version
// (see .signpath/ and docs/code-signing.md). The service (rust/service) is
// the exception: it carries its own version, so its file stays the same from
// release to release.

/// The release version: the workspace version in the repository's
/// Cargo.toml. The display driver is outside the workspace and has its own
/// crate version, so the version comes from here rather than from Cargo. The repository is the nearest folder above
/// the crate that holds this file.
#[allow(dead_code)] // The service's build script names its own version.
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

/// A VERSIONINFO resource script for one binary at `version`. `file` is the
/// name Rubylight ships it under (OriginalFilename; InternalName is that name
/// without its extension), None for a resource several binaries share. `dll`
/// sets the file type.
fn version_resource(description: &str, file: Option<&str>, dll: bool, version: &str) -> String {
    // 2.4.0-rc.1 is 2,4,0,0 in the numeric fields; the strings keep it whole.
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u16>().unwrap_or(0));
    let [major, minor, patch] = [(); 3].map(|_| parts.next().unwrap_or(0));
    let numbers = format!("{major},{minor},{patch},0");
    let file_type = if dll { 2 } else { 1 };
    let names = file.map_or_else(Vec::new, |file| {
        let internal = std::path::Path::new(file)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(file);
        vec![
            format!("VALUE \"InternalName\", \"{internal}\""),
            format!("VALUE \"OriginalFilename\", \"{file}\""),
        ]
    });
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
        "VALUE \"CompanyName\", \"Rubylight\"".to_owned(),
        format!("VALUE \"FileDescription\", \"{description}\""),
        format!("VALUE \"FileVersion\", \"{version}\""),
    ]
    .into_iter()
    .chain(names)
    .chain([
        "VALUE \"LegalCopyright\", \"Copyright (c) Ramazan Kara and contributors\"".to_owned(),
        "VALUE \"ProductName\", \"Rubylight\"".to_owned(),
        format!("VALUE \"ProductVersion\", \"{version}\""),
        "END".to_owned(),
        "END".to_owned(),
        "BLOCK \"VarFileInfo\"".to_owned(),
        "BEGIN".to_owned(),
        "VALUE \"Translation\", 0x409, 1200".to_owned(),
        "END".to_owned(),
        "END".to_owned(),
    ])
    .collect::<Vec<_>>()
    .join("\n")
        + "\n"
}

/// Writes `name`.rc to OUT_DIR, `extra` (an icon line, say) followed by the
/// version resource at the release version, and returns its path for
/// embed_resource. `file` as in [`version_resource`].
#[allow(dead_code)] // The service's build script names its own version.
fn write_resource(
    name: &str,
    file: Option<&str>,
    extra: &str,
    description: &str,
    dll: bool,
) -> std::path::PathBuf {
    write_versioned_resource(name, file, extra, description, dll, &release_version())
}

/// [`write_resource`] at `version` instead of the release's.
fn write_versioned_resource(
    name: &str,
    file: Option<&str>,
    extra: &str,
    description: &str,
    dll: bool,
    version: &str,
) -> std::path::PathBuf {
    let rc = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join(format!("{name}.rc"));
    let resource = version_resource(description, file, dll, version);
    std::fs::write(&rc, format!("{extra}{resource}")).unwrap();
    rc
}
