//! Builds Nightjar's patched moonlight-common-c receive paths for the differential tests.
//!
//! The sources are copied to OUT_DIR and the patches from `moonlight-core/patches` are applied
//! there in the app's order, so the submodule stays untouched. Without the sources (submodules
//! not checked out) or off Unix, the tests are compiled out.

use std::fs;
use std::path::{Path, PathBuf};

/// The order `scripts/test-native.sh` and `app/build.gradle` apply them in.
const PATCHES: &[&str] = &[
    "bughunt-stream.patch",
    "bughunt-input.patch",
    "controller-haptics.patch",
    "host-termination.patch",
    "disconnect-intercept.patch",
    "control-payload.patch",
    "control-teardown.patch",
    "host-timing.patch",
    "network-grace.patch",
    "cursor-shape.patch",
    "network-grace-input.patch",
    "pyrowave-link-hints.patch",
    "video-recv-buffer.patch",
    "clock-sync.patch",
];

const WRAPPERS: &[&str] = &[
    "android_rtp_video.c",
    "android_depacketizer.c",
    "pyrowave_protocol.h",
];

fn main() {
    println!("cargo::rustc-check-cfg=cfg(difftest_c)");
    println!("cargo::rerun-if-changed=c");
    println!("cargo::rerun-if-changed=build.rs");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let jni = manifest.join("../../app/src/main/jni");
    let core = jni.join("moonlight-core");
    let common = core.join("moonlight-common-c");
    let nanors = common.join("nanors");
    for path in [
        &core.join("patches"),
        &core.join("android_rtp_video.c"),
        &core.join("android_depacketizer.c"),
    ] {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    println!("cargo::rerun-if-changed={}", common.join("src").display());

    let unix = std::env::var("CARGO_CFG_UNIX").is_ok();
    if !unix || !common.join("src/Limelight.h").exists() || !nanors.join("rs.c").exists() {
        println!(
            "cargo::warning=differential tests disabled: needs Unix and the moonlight-common-c and nanors submodules"
        );
        return;
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let out_core = out.join("jni/moonlight-core");
    let out_src = out_core.join("moonlight-common-c/src");
    let _ = fs::remove_dir_all(out.join("jni"));
    fs::create_dir_all(&out_src).unwrap();
    fs::create_dir_all(out.join("jni/pyrowave-renderer")).unwrap();
    // Gradle and scripts/test-native.sh patch the submodule's src in place, so take the pinned
    // commit's files from git; the working tree is only the fallback without git.
    let pinned = copy_pinned_sources(&common, &out_src);
    if !pinned {
        for entry in fs::read_dir(common.join("src")).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), out_src.join(entry.file_name())).unwrap();
        }
    }
    for name in WRAPPERS {
        fs::copy(core.join(name), out_core.join(name)).unwrap();
    }
    fs::copy(
        jni.join("pyrowave-renderer/frame.h"),
        out.join("jni/pyrowave-renderer/frame.h"),
    )
    .unwrap();
    for name in PATCHES {
        let patch = fs::read_to_string(core.join("patches").join(name)).unwrap();
        apply_patch(&out_core.join("moonlight-common-c"), &patch, !pinned)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
    }

    // Limelight-internal.h includes enet; nothing here uses it.
    let stub = out.join("stub/enet");
    fs::create_dir_all(&stub).unwrap();
    fs::write(
        stub.join("enet.h"),
        "#pragma once\n#include <stdint.h>\ntypedef uint32_t enet_uint32;\n\
         typedef struct _ENetHost ENetHost;\ntypedef struct _ENetPeer ENetPeer;\n\
         typedef struct _ENetEvent ENetEvent;\n",
    )
    .unwrap();

    let mut build = cc::Build::new();
    build
        .std("gnu11")
        .opt_level(2)
        .define("HAS_SOCKLEN_T", "1")
        .define("HAVE_CLOCK_GETTIME", "1")
        .include(&out_core)
        .include(&out_src)
        .include(out.join("stub"))
        .include(&nanors)
        .include(nanors.join("deps/obl"))
        .include(manifest.join("c"))
        .warnings(false)
        .file(out_core.join("android_rtp_video.c"))
        .file(manifest.join("c/shim_video.c"))
        .file(manifest.join("c/shim_audio.c"))
        .file(out_src.join("RtpAudioQueue.c"))
        .file(out_src.join("ByteBuffer.c"))
        .file(out_src.join("PlatformCrypto.c"))
        .file(nanors.join("rs.c"))
        .file(nanors.join("deps/obl/oblas_common.c"))
        .file(nanors.join("deps/obl/oblas_lite.c"));
    build.compile("moonlight_receive");
    println!("cargo::rustc-link-lib=crypto");
    println!("cargo::rustc-cfg=difftest_c");
}

/// Writes `src/*` as committed at the submodule's HEAD to `out`; false when git cannot.
fn copy_pinned_sources(common: &Path, out: &Path) -> bool {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(common)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
    };
    let Some(list) = git(&["ls-tree", "--name-only", "HEAD", "src/"]) else {
        return false;
    };
    let names: Vec<String> = String::from_utf8_lossy(&list)
        .lines()
        .map(str::to_owned)
        .collect();
    if names.is_empty() {
        return false;
    }
    for name in names {
        let Some(bytes) = git(&["show", &format!("HEAD:{name}")]) else {
            return false;
        };
        let file = Path::new(&name).file_name().unwrap();
        fs::write(out.join(file), bytes).unwrap();
    }
    true
}

/// Applies a unified diff (`git diff` output) to files below `root`. Hunks must match exactly,
/// ignoring line endings; they may sit at a different line than the header says. With
/// `patched` (sources copied from a working tree that may already carry the patches), hunks
/// whose result is present are skipped.
fn apply_patch(root: &Path, patch: &str, patched: bool) -> Result<(), String> {
    let lines: Vec<&str> = patch.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let Some(path) = lines[i].strip_prefix("+++ b/") else {
            i += 1;
            continue;
        };
        let file = root.join(path.trim_end());
        let text = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let mut content: Vec<String> = text
            .lines()
            .map(|l| l.trim_end_matches('\r').to_string())
            .collect();
        i += 1;
        let mut shift: isize = 0;
        while i < lines.len() && lines[i].starts_with("@@") {
            let (start, old_count, new_count) = hunk_header(lines[i]).ok_or("bad hunk header")?;
            i += 1;
            let (mut old, mut new) = (Vec::new(), Vec::new());
            while i < lines.len() && (old.len() < old_count || new.len() < new_count) {
                let line = lines[i].trim_end_matches('\r');
                match line.chars().next() {
                    Some(' ') => {
                        old.push(&line[1..]);
                        new.push(&line[1..]);
                    }
                    Some('-') => old.push(&line[1..]),
                    Some('+') => new.push(&line[1..]),
                    None => {
                        old.push("");
                        new.push("");
                    }
                    _ => {}
                }
                i += 1;
            }
            if i < lines.len() && lines[i].starts_with('\\') {
                i += 1;
            }
            let expected = (start - 1 + shift).max(0) as usize;
            let find = |block: &[&str]| {
                (0..content.len().max(expected) + 1)
                    .flat_map(|d| [expected.checked_add(d), expected.checked_sub(d)])
                    .flatten()
                    .find(|&at| {
                        at + block.len() <= content.len()
                            && content[at..at + block.len()]
                                .iter()
                                .zip(block)
                                .all(|(a, b)| a == b)
                    })
            };
            // A working tree the app build already patched carries each hunk's result.
            if patched && find(&new).is_some() {
                continue;
            }
            let at = find(&old)
                .ok_or_else(|| format!("hunk at line {start} of {path} does not apply"))?;
            content.splice(at..at + old.len(), new.iter().map(|s| s.to_string()));
            shift += new.len() as isize - old.len() as isize + at as isize - expected as isize;
        }
        let mut text = content.join("\n");
        text.push('\n');
        fs::write(&file, text).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `@@ -start,count +start,count @@` with counts defaulting to 1.
fn hunk_header(line: &str) -> Option<(isize, usize, usize)> {
    let mut parts = line.split_whitespace().skip(1);
    let range = |part: &str| -> Option<(isize, usize)> {
        let mut numbers = part[1..].split(',');
        let start = numbers.next()?.parse().ok()?;
        let count = numbers.next().map_or(Some(1), |n| n.parse().ok())?;
        Some((start, count))
    };
    let (start, old) = range(parts.next()?)?;
    let (_, new) = range(parts.next()?)?;
    Some((start, old, new))
}
