include!("../assets/version_info.rs");

// Links NVIDIA's NGX library from the RTX Video SDK and gives the adapter DLL
// the release's version information. Only Rubylight's separate, unsigned
// NVIDIA build (`rust/build.ps1 -Nvidia`) builds this crate.
fn main() {
    if !std::env::var("TARGET").unwrap().ends_with("windows-msvc") {
        panic!("NVIDIA's vendor library requires --target x86_64-pc-windows-msvc");
    }
    println!("cargo:rerun-if-env-changed=NV_RTX_VIDEO_SDK");
    let sdk = std::path::PathBuf::from(
        std::env::var("NV_RTX_VIDEO_SDK").expect("set NV_RTX_VIDEO_SDK to RTX Video SDK 1.1.0"),
    );
    for header in ["nvsdk_ngx_defs_truehdr.h", "nvsdk_ngx_defs_vsr.h"] {
        assert!(
            sdk.join("include").join(header).exists(),
            "SDK header {header} is missing"
        );
    }
    println!("cargo:rerun-if-changed=src/lib.rs");
    check_declarations(&sdk.join("include"));
    println!(
        "cargo:rustc-link-search=native={}",
        sdk.join("lib/Windows/x64").display()
    );
    // NVIDIA's library comes for the static C runtime (_s) and its DLLs (_d). The static one
    // spares the adapter the Visual C++ Redistributable, which a PC need not have;
    // rust/build.ps1 builds the adapter with crt-static.
    let static_crt = std::env::var("CARGO_CFG_TARGET_FEATURE")
        .is_ok_and(|features| features.split(',').any(|feature| feature == "crt-static"));
    let ngx = if static_crt {
        "nvsdk_ngx_s"
    } else {
        "nvsdk_ngx_d"
    };
    println!("cargo:rustc-link-lib=static={ngx}");
    println!("cargo:rustc-link-lib=advapi32");
    println!("cargo:rustc-link-lib=user32");
    println!("cargo:rerun-if-changed=../assets/version_info.rs");
    let rc = write_resource(
        "truehdr",
        Some("butterpollo_truehdr.dll"),
        "",
        "Rubylight NVIDIA image enhancement adapter",
        true,
    );
    embed_resource::compile_for_cdylib(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}

/// Checks that each NGX function `src/lib.rs` declares takes as many
/// parameters as the SDK declares for applications: the headers without the
/// NGX_SNIPPET_BUILD branches, which declare the feature DLLs' own forms. A
/// wrong count shifts every later argument: called with the snippet form,
/// NVSDK_NGX_D3D11_Init read the version as its feature-info pointer and
/// crashed (issue #103).
fn check_declarations(include: &std::path::Path) {
    let source = std::fs::read_to_string("src/lib.rs").expect("src/lib.rs");
    let block = source
        .split("unsafe extern \"C\" {")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("src/lib.rs declares NGX's functions in an extern block");
    let block: String = block
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .map(|line| format!("{line}\n"))
        .collect();
    let mut headers = String::new();
    for header in ["nvsdk_ngx.h", "nvsdk_ngx_params.h"] {
        let text = std::fs::read_to_string(include.join(header))
            .unwrap_or_else(|e| panic!("SDK header {header}: {e}"));
        headers += &application_lines(&text);
    }
    let mut checked = 0;
    for item in block.split("fn ").skip(1) {
        let (name, rest) = item.split_once('(').expect("a declaration's parameters");
        let name = name.trim();
        let ours = parameter_count(rest);
        // A declaration: `NVSDK_CONV`, any spacing, the name and its list.
        let pattern = format!("{name}(");
        let theirs: Vec<usize> = headers
            .match_indices(&pattern)
            .filter(|(at, _)| headers[..*at].trim_end().ends_with("NVSDK_CONV"))
            .map(|(at, _)| parameter_count(&headers[at + pattern.len()..]))
            .collect();
        assert!(!theirs.is_empty(), "the SDK's headers declare no {name}");
        assert!(
            theirs.iter().all(|&count| count == ours),
            "{name} takes {theirs:?} parameters in the SDK's headers but {ours} in src/lib.rs"
        );
        checked += 1;
    }
    assert!(checked > 0, "src/lib.rs declares no NGX function");
}

/// The header's lines outside its NGX_SNIPPET_BUILD branches.
fn application_lines(header: &str) -> String {
    // Per open #if: whether NGX_SNIPPET_BUILD decides it, and whether the
    // current branch is the one taken when it is defined.
    let mut open: Vec<(bool, bool)> = Vec::new();
    let mut kept = String::new();
    for line in header.lines() {
        let directive = line.trim_start();
        if directive.starts_with("#if") {
            let snippet = directive.contains("NGX_SNIPPET_BUILD");
            let negated = directive.starts_with("#ifndef") || directive.contains('!');
            open.push((snippet, snippet && !negated));
        } else if directive.starts_with("#el") {
            if let Some((true, branch)) = open.last_mut() {
                *branch = !*branch;
            }
        } else if directive.starts_with("#endif") {
            open.pop();
        } else if !open.iter().any(|&(_, branch)| branch) {
            kept += line;
            kept.push('\n');
        }
    }
    kept
}

/// The number of parameters before the `)` that closes a list starting
/// right after its `(`; `void` alone counts none.
fn parameter_count(list: &str) -> usize {
    let mut depth = 1;
    let mut parameters = vec![String::new()];
    for c in list.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            ',' if depth == 1 => {
                parameters.push(String::new());
                continue;
            }
            _ => {}
        }
        parameters.last_mut().unwrap().push(c);
    }
    parameters
        .iter()
        .map(|parameter| parameter.trim())
        .filter(|parameter| !parameter.is_empty() && *parameter != "void")
        .count()
}
