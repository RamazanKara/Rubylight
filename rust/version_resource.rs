use std::{env, path::PathBuf};

pub fn write(name: &str, description: &str, dll: bool) -> PathBuf {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest.join("../assets/butterpollo.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest.join("../version_resource.rs").display()
    );
    let icon = icon.display().to_string().replace('\\', "/");
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numbers = ["MAJOR", "MINOR", "PATCH"]
        .map(|part| {
            env::var(format!("CARGO_PKG_VERSION_{part}"))
                .unwrap()
                .parse::<u16>()
                .expect("Windows version components must fit in 16 bits")
                .to_string()
        })
        .join(",");
    let file_type = if dll { 2 } else { 1 };
    let rc = PathBuf::from(env::var("OUT_DIR").unwrap()).join(format!("{name}.rc"));
    std::fs::write(
        &rc,
        format!(
            r#"#include <winver.h>
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numbers},0
PRODUCTVERSION {numbers},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0
FILEOS VOS_NT_WINDOWS32
FILETYPE {file_type}
FILESUBTYPE 0
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "RamazanKara\0"
            VALUE "FileDescription", "{description}\0"
            VALUE "FileVersion", "{version}\0"
            VALUE "LegalCopyright", "Copyright (C) 2026 RamazanKara; upstream notices in NOTICE and licenses\0"
            VALUE "ProductName", "Rubylight\0"
            VALUE "ProductVersion", "{version}\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0409, 1200
    END
END
"#
        ),
    )
    .unwrap();
    rc
}
