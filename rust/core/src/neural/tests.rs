use super::*;
use serde_json::json;
use std::io::Write;

#[test]
fn only_the_mods_own_package_may_be_fetched() {
    for allowed in [
        "AMDNR-v0.3.5.8.zip",
        "AMDNR-v0.3.5.8R4.zip",
        "AMDNR-v1.0.zip",
        "AMDNR-v0.4.zip",
    ] {
        assert!(allowed_asset(allowed), "{allowed}");
    }
    // Everything else the releases carry: the launcher (all rights reserved),
    // the danielblnc runtimes (licensed to the mod only), the capture host,
    // separate components, the source archive, checksums, and NVIDIA's files.
    for refused in [
        "AMDNR-Launcher.exe",
        "v0.8.0-Runtime.zip",
        "v0.6.0-Runtime.zip",
        "Runtime.zip",
        "Daniel-0.8.0-Manual-AMDNR-0.3.5.8.zip",
        "AMDNR-Anywhere-0.3.5.8R2.zip",
        "AMDNR-lmxxf-0.3.5.8R2.zip",
        "AMDNR-Mochi-0.3.5.8R2-component.zip",
        "AMDNR-v0.3.5.8R4-source.zip",
        "SHA256SUMS.txt",
        "nvngx_dlssnr.dll",
        "nvngx_dlss.dll",
        "AMDNR-v.zip",
        "AMDNR-vR1.zip",
        "AMDNR-v0.3.5.8.zip.exe",
        "AMDNR-v0.3.5.8.exe",
        "amdnr-v0.3.5.8.zip",
        "AMDNR-v0.3.5.8R.zip",
        "AMDNR-v0..3.zip",
        "AMDNR-v0.3/../x.zip",
        "AMDNR-v0.3.5.8 .zip",
        "AMDNR-v12345.zip",
    ] {
        assert!(!allowed_asset(refused), "{refused}");
    }
}

fn asset(tag: &str, name: &str, size: u64) -> Value {
    json!({
        "name": name,
        "size": size,
        "digest": format!("sha256:{}", "AB".repeat(32)),
        "browser_download_url": format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{name}"),
    })
}

fn release(tag: &str, published: &str, assets: Vec<Value>) -> Value {
    json!({"tag_name": tag, "published_at": published, "draft": false, "prerelease": false, "assets": assets})
}

#[test]
fn releases_offer_only_the_package_newest_first() {
    let mut draft = release(
        "Alpha0.9",
        "2026-12-01T00:00:00Z",
        vec![asset("Alpha0.9", "AMDNR-v0.9.zip", 10)],
    );
    draft["draft"] = json!(true);
    let mut pre = release(
        "Alpha0.8",
        "2026-11-30T00:00:00Z",
        vec![asset("Alpha0.8", "AMDNR-v0.8.zip", 10)],
    );
    pre["prerelease"] = json!(true);
    let mut foreign_url = asset("Alpha0.7", "AMDNR-v0.7.zip", 10);
    foreign_url["browser_download_url"] = json!("https://example.com/AMDNR-v0.7.zip");
    let mut no_digest = asset("Alpha0.6", "AMDNR-v0.6.zip", 10);
    no_digest.as_object_mut().unwrap().remove("digest");
    let list = json!([
        draft,
        pre,
        release("Alpha0.7", "2026-11-29T00:00:00Z", vec![foreign_url]),
        release("Alpha0.6", "2026-11-28T00:00:00Z", vec![no_digest]),
        release(
            "Alpha0.5",
            "2026-11-27T00:00:00Z",
            vec![asset("Alpha0.5", "AMDNR-v0.5.zip", MAX_ARCHIVE + 1)]
        ),
        // Only refused assets: nothing to fetch.
        release(
            "Alpha0.4",
            "2026-11-26T00:00:00Z",
            vec![
                asset("Alpha0.4", "AMDNR-Launcher.exe", 10),
                asset("Alpha0.4", "v0.8.0-Runtime.zip", 10)
            ]
        ),
        release(
            "../evil",
            "2026-11-25T00:00:00Z",
            vec![asset("../evil", "AMDNR-v0.4.zip", 10)]
        ),
        // Two packages: ambiguous.
        release(
            "Alpha0.3",
            "2026-11-24T00:00:00Z",
            vec![
                asset("Alpha0.3", "AMDNR-v0.3.zip", 10),
                asset("Alpha0.3", "AMDNR-v0.3R1.zip", 10)
            ]
        ),
        // Listed out of order: published dates decide.
        release(
            "Alpha0.3.5.7",
            "2026-10-22T18:09:28Z",
            vec![
                asset("Alpha0.3.5.7", "AMDNR-Launcher.exe", 10),
                asset("Alpha0.3.5.7", "AMDNR-v0.3.5.7.zip", 20)
            ]
        ),
        release(
            "Alpha0.3.5.8",
            "2026-10-23T16:57:26Z",
            vec![
                asset("Alpha0.3.5.8", "v0.8.0-Runtime.zip", 10),
                asset("Alpha0.3.5.8", "AMDNR-v0.3.5.8.zip", 30),
                asset("Alpha0.3.5.8", "AMDNR-Anywhere-0.3.5.8.zip", 10),
            ]
        ),
    ]);
    let found = releases(&list);
    let tags: Vec<_> = found.iter().map(|r| r.tag.as_str()).collect();
    assert_eq!(tags, ["Alpha0.3.5.8", "Alpha0.3.5.7"]);
    assert_eq!(
        found[0],
        Release {
            tag: "Alpha0.3.5.8".into(),
            asset: "AMDNR-v0.3.5.8.zip".into(),
            url: format!(
                "https://github.com/{REPOSITORY}/releases/download/Alpha0.3.5.8/AMDNR-v0.3.5.8.zip"
            ),
            digest: "ab".repeat(32),
            size: 30,
        }
    );
    assert!(found.iter().all(|r| allowed_asset(&r.asset)));
    assert!(releases(&json!({"message": "rate limited"})).is_empty());
}

/// The file lists of two real packages: 0.3.5.8 has the whole mod, 0.3.5.8R4
/// ships the neural runtime as a separate component.
#[test]
fn a_package_without_the_runtime_is_not_the_whole_mod() {
    let names = |list: &[&str]| list.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    let whole = names(&[
        "Licenses/lmxxf_LICENSE.txt",
        "OptiScaler/libxess.dll",
        "LmxxfNrRuntime.dll",
        "LmxxfNrRuntime.pak",
        "OptiScaler.dll",
        "OptiScaler.ini",
        "SHA256SUMS.txt",
    ]);
    assert!(complete(&whole));
    let split = names(&[
        "Licenses/lmxxf_LICENSE.txt",
        "OptiScaler/libxess.dll",
        "OptiScaler.dll",
        "OptiScaler.ini",
    ]);
    assert!(!complete(&split));
    let unchecked = names(&["LmxxfNrRuntime.dll", "LmxxfNrRuntime.pak", "OptiScaler.dll"]);
    assert!(!complete(&unchecked), "no checksum list");
}

fn archive(names: &[&str], comment: &str) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for name in names {
        writer.start_file(*name, options).unwrap();
        writer.write_all(name.as_bytes()).unwrap();
    }
    writer.set_comment(comment);
    writer.finish().unwrap().into_inner()
}

fn listed(bytes: &[u8], tail: usize) -> Result<Vec<String>, String> {
    let tail = &bytes[bytes.len().saturating_sub(tail)..];
    let (offset, length) = directory_span(tail, bytes.len() as u64)?;
    directory_names(&bytes[offset as usize..(offset + length) as usize])
}

#[test]
fn a_zip_lists_its_files_from_its_last_bytes() {
    let names = [
        "OptiScaler.dll",
        "OptiScaler/D3D12_OptiScaler/D3D12Core.dll",
        "Licenses/Привет.txt",
    ];
    let bytes = archive(&names, "");
    assert_eq!(listed(&bytes, 64 * 1024).unwrap(), names);
    // A comment, and a tail that ends just past the record.
    let commented = archive(&names, "PK\x05\x06 looks like a record");
    assert_eq!(listed(&commented, 22 + 26).unwrap(), names);
    // Not a zip, or cut before its end record.
    assert!(listed(b"not an archive at all, just text", 64).is_err());
    assert!(listed(&bytes[..bytes.len() - 5], 64 * 1024).is_err());
    // A directory that points past the end.
    let mut wrong = bytes.clone();
    let end = wrong.len() - 22;
    wrong[end + 16..end + 20].copy_from_slice(&u32::MAX.to_le_bytes()[..]);
    wrong[end + 12..end + 16].copy_from_slice(&5u32.to_le_bytes());
    assert!(listed(&wrong, 64 * 1024).is_err());
    let mut damaged = bytes.clone();
    let (offset, _) = directory_span(&bytes, bytes.len() as u64).unwrap();
    damaged[offset as usize] = b'X';
    assert!(listed(&damaged, 64 * 1024).is_err());
}

#[test]
fn a_zip64_end_record_gives_the_directory() {
    // A directory of 100 bytes at 1000, then the zip64 record, its locator and the end record.
    let size = 1000 + 100 + 56 + 20 + 22;
    let mut tail = Vec::new();
    let record_at = 1100u64;
    tail.extend_from_slice(b"PK\x06\x06");
    tail.extend_from_slice(&44u64.to_le_bytes());
    tail.extend_from_slice(&[0; 2 + 2 + 4 + 4]);
    tail.extend_from_slice(&3u64.to_le_bytes());
    tail.extend_from_slice(&3u64.to_le_bytes());
    tail.extend_from_slice(&100u64.to_le_bytes());
    tail.extend_from_slice(&1000u64.to_le_bytes());
    assert_eq!(tail.len(), 56);
    tail.extend_from_slice(b"PK\x06\x07");
    tail.extend_from_slice(&0u32.to_le_bytes());
    tail.extend_from_slice(&record_at.to_le_bytes());
    tail.extend_from_slice(&1u32.to_le_bytes());
    tail.extend_from_slice(b"PK\x05\x06");
    tail.extend_from_slice(&[0; 4]);
    tail.extend_from_slice(&[0xff; 4]);
    tail.extend_from_slice(&[0xff; 8]);
    tail.extend_from_slice(&0u16.to_le_bytes());
    assert_eq!(directory_span(&tail, size), Ok((1000, 100)));
    // The record before the tail cannot be read.
    assert!(directory_span(&tail[56..], size).is_err());
}

fn hash(text: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(text.as_bytes()))
}

#[test]
fn every_file_must_match_the_release_checksums() {
    let text = format!(
        "\u{feff}{}  OptiScaler.dll\r\n{}  LmxxfNrRuntime.dll\n{} *LmxxfNrRuntime.pak\n{}  OptiScaler/libxess.dll\n{}  v0.8.0-Runtime/dlssnr_amd_pass1.dll\n\n",
        hash("dll"),
        hash("runtime"),
        hash("pak"),
        hash("xess").to_ascii_uppercase(),
        hash("daniel"),
    );
    let sums = sums(&text).unwrap();
    assert_eq!(sums["OptiScaler/libxess.dll"], hash("xess"));
    let files: BTreeMap<String, String> = [
        ("OptiScaler.dll", hash("dll")),
        ("LmxxfNrRuntime.dll", hash("runtime")),
        ("LmxxfNrRuntime.pak", hash("pak")),
        ("OptiScaler/libxess.dll", hash("xess")),
        (SUMS, hash("sums")),
    ]
    .into_iter()
    .map(|(path, hash)| (path.to_string(), hash))
    .collect();
    // The runtime zip's file in the list has no file here: that is fine.
    assert_eq!(check_package(&files, &sums), Ok(()));

    let mut changed = files.clone();
    changed.insert("LmxxfNrRuntime.pak".into(), hash("other pak"));
    assert_eq!(
        check_package(&changed, &sums),
        Err("LmxxfNrRuntime.pak does not match the release's SHA256SUMS.txt".into())
    );
    let mut extra = files.clone();
    extra.insert("payload.dll".into(), hash("payload"));
    assert_eq!(
        check_package(&extra, &sums),
        Err("payload.dll is not in the release's SHA256SUMS.txt".into())
    );
    let mut missing = files.clone();
    missing.remove("LmxxfNrRuntime.dll");
    assert_eq!(
        check_package(&missing, &sums),
        Err("the release has no LmxxfNrRuntime.dll".into())
    );

    assert!(super::sums("not a checksum line").is_err());
    let twice = format!("{}  a.dll\n{}  a.dll\n", hash("a"), hash("b"));
    assert!(super::sums(&twice).is_err());
}

#[test]
fn archive_paths_stay_inside_the_game_folder() {
    assert_eq!(
        entry_path("OptiScaler/D3D12_OptiScaler/D3D12Core.dll").as_deref(),
        Some("OptiScaler/D3D12_OptiScaler/D3D12Core.dll")
    );
    assert_eq!(
        entry_path("Licenses\\LICENSE.txt").as_deref(),
        Some("Licenses/LICENSE.txt")
    );
    assert_eq!(entry_path("OptiScaler/").as_deref(), Some("OptiScaler"));
    for bad in [
        "",
        "/etc/x",
        "../x.dll",
        "a/../../x.dll",
        "C:/Windows/x.dll",
        "C:x.dll",
        "a/./b",
        "dxgi.dll:stream",
        "con.txt",
        "a/NUL",
        "COM1.dll",
        "x.dll.",
        "x ",
        "a//b",
        "a\u{0}b",
    ] {
        assert_eq!(entry_path(bad), None, "{bad:?}");
    }
    assert!(
        document("README.md")
            && document("LICENSE")
            && document("Licenses/XeSS_LICENSE.txt")
            && document("Docs/a.png")
    );
    assert!(
        !document("OptiScaler.ini")
            && !document("OptiScaler/x.md")
            && !document("LmxxfNrRuntime.pak")
    );
}

#[test]
fn the_mod_never_takes_a_name_another_program_holds() {
    let with = |taken: &'static [(&'static str, Occupant)]| {
        proxy(move |name| {
            taken
                .iter()
                .find(|(n, _)| *n == name)
                .map_or(Occupant::Free, |(_, o)| *o)
        })
    };
    assert_eq!(with(&[]), Ok("dxgi.dll"));
    // ReShade or a game's own DLL as dxgi.dll: the next name.
    assert_eq!(with(&[("dxgi.dll", Occupant::Foreign)]), Ok("d3d12.dll"));
    assert_eq!(
        with(&[
            ("dxgi.dll", Occupant::Foreign),
            ("d3d12.dll", Occupant::Foreign),
            ("winmm.dll", Occupant::Foreign)
        ]),
        Ok("version.dll")
    );
    // An earlier install keeps its name.
    assert_eq!(with(&[("winmm.dll", Occupant::Ours)]), Ok("winmm.dll"));
    let error = with(&[("version.dll", Occupant::OptiScaler)]).unwrap_err();
    assert!(
        error.contains("another copy of OptiScaler") && error.contains("version.dll"),
        "{error}"
    );
    let all: &'static [(&'static str, Occupant)] = &[
        ("dxgi.dll", Occupant::Foreign),
        ("d3d12.dll", Occupant::Foreign),
        ("winmm.dll", Occupant::Foreign),
        ("version.dll", Occupant::Foreign),
        ("dbghelp.dll", Occupant::Foreign),
        ("winhttp.dll", Occupant::Foreign),
        ("wininet.dll", Occupant::Foreign),
    ];
    assert!(with(all).unwrap_err().contains("every name"));
}

#[test]
fn other_upscaler_mods_are_recognised_by_their_files() {
    assert_eq!(other_mod("OptiScaler.ini"), Some("OptiScaler"));
    assert_eq!(other_mod("OptiScaler"), Some("OptiScaler"));
    assert_eq!(other_mod("nvngx.ini"), Some("OptiScaler"));
    assert_eq!(
        other_mod("fakenvapi.dll"),
        Some("fakenvapi (an OptiScaler add-on)")
    );
    assert_eq!(other_mod("dlss-enabler-upscaler.dll"), Some("DLSS Enabler"));
    assert_eq!(
        other_mod("dlssg_to_fsr3_amd_is_better.dll"),
        Some("dlssg-to-fsr3")
    );
    assert_eq!(other_mod("SpecialK64.dll"), Some("Special K"));
    for name in [
        "ReShade.ini",
        "dxgi.dll",
        "nvngx_dlss.dll",
        "Game.exe",
        "OptiScaler.log",
    ] {
        assert_eq!(other_mod(name), None, "{name}");
    }
}

#[test]
fn anti_cheat_is_recognised_by_its_files_and_folders() {
    for (name, which) in [
        ("EasyAntiCheat", "Easy Anti-Cheat"),
        ("EasyAntiCheat_EOS", "Easy Anti-Cheat"),
        ("EasyAntiCheat_x64.dll", "Easy Anti-Cheat"),
        ("start_protected_game.exe", "Easy Anti-Cheat"),
        ("BattlEye", "BattlEye"),
        ("BEService_x64.exe", "BattlEye"),
        ("BEClient_x64.dll", "BattlEye"),
        ("RainbowSix_BE.exe", "BattlEye"),
        ("vgk.sys", "Riot Vanguard"),
        ("randgrid.sys", "Ricochet"),
        ("mhyprot3.sys", "mhyprot"),
        ("GameGuard", "nProtect GameGuard"),
        ("npggNT.des", "nProtect GameGuard"),
        ("XIGNCODE", "XIGNCODE3"),
        ("x3.xem", "XIGNCODE3"),
        ("PnkBstrA.exe", "PunkBuster"),
        ("EQU8", "EQU8"),
        ("faceit.sys", "FACEIT"),
        ("AntiCheatExpert", "Anti-Cheat Expert"),
        ("SGuard64.exe", "Anti-Cheat Expert"),
        ("EAAntiCheat.GameServiceLauncher.exe", "EA Javelin"),
        ("denuvo-anti-cheat.sys", "Denuvo Anti-Cheat"),
        ("NeacSafe64.sys", "NetEase Anti-Cheat"),
        ("HShield", "AhnLab HackShield"),
    ] {
        assert_eq!(anti_cheat(name), Some(which), "{name}");
    }
    for name in [
        "Binaries",
        "Win64",
        "SilentHill2-Win64-Shipping.exe",
        "Engine",
        "be.exe",
        "Tribe.exe",
        "steam_api64.dll",
        "dxgi.dll",
        "anticheat_readme_not.txt",
        "Faceless.pak",
    ] {
        assert_eq!(anti_cheat(name), None, "{name}");
    }
}

fn manifest() -> Manifest {
    let placed = |text: &str| Placed {
        sha256: hash(text),
        size: text.len() as u64,
    };
    Manifest {
        by: PLACED_BY.into(),
        release: "Alpha0.3.5.8".into(),
        proxy: "dxgi.dll".into(),
        files: [
            ("dxgi.dll", placed("dll")),
            ("OptiScaler.ini", placed("ini")),
            ("LmxxfNrRuntime.pak", placed("pak")),
            ("OptiScaler/libxess.dll", placed("xess")),
        ]
        .into_iter()
        .map(|(path, placed)| (path.to_string(), placed))
        .collect(),
        folders: vec!["OptiScaler".into()],
    }
}

#[test]
fn a_manifest_is_trusted_only_as_the_host_wrote_it() {
    let good = manifest();
    let text = serde_json::to_string(&good).unwrap();
    assert_eq!(Manifest::parse(&text), Ok(good.clone()));
    let mut other = good.clone();
    other.by = "Someone".into();
    assert!(Manifest::parse(&serde_json::to_string(&other).unwrap()).is_err());
    let mut outside = good.clone();
    outside.files.insert(
        "../../Windows/System32/x.dll".into(),
        Placed {
            sha256: hash("x"),
            size: 1,
        },
    );
    assert!(Manifest::parse(&serde_json::to_string(&outside).unwrap()).is_err());
    let mut folder = good.clone();
    folder.folders.push("C:/Windows".into());
    assert!(Manifest::parse(&serde_json::to_string(&folder).unwrap()).is_err());
    let mut name = good;
    name.proxy = "kernel32.dll".into();
    assert!(Manifest::parse(&serde_json::to_string(&name).unwrap()).is_err());
    assert!(Manifest::parse("{").is_err());
}

#[test]
fn removal_deletes_what_is_still_as_placed_and_keeps_what_changed() {
    let removal = removal(&manifest(), |path| match path {
        // Replaced by someone: no longer provably the mod.
        "dxgi.dll" => Now::Changed,
        // The menu saved its settings.
        "OptiScaler.ini" => Now::Changed,
        "LmxxfNrRuntime.pak" => Now::Same,
        _ => Now::Missing,
    });
    assert_eq!(removal.delete, ["LmxxfNrRuntime.pak", "OptiScaler.ini"]);
    assert_eq!(removal.keep, ["dxgi.dll"]);
    assert!(
        logs("OptiScaler.log")
            && logs("lmxxf_backend.log")
            && logs("OptiScaler.previous-1.Game.exe.log")
    );
    assert!(
        !logs("Game.log") && !logs("AMDNR-report-Game.exe-2026.zip") && !logs("OptiScaler.ini")
    );
}

fn gpu(name: &str, arch: &str, compute_units: u32) -> Gpu {
    Gpu {
        name: name.into(),
        arch: arch.into(),
        compute_units,
    }
}

#[test]
fn the_switch_is_offered_by_what_the_compute_runtime_reports() {
    for supported in [
        gpu("AMD Radeon RX 9070 XT", "gfx1201", 64),
        gpu("AMD Radeon RX 7900 XT", "gfx1100", 84),
        gpu("AMD Radeon RX 7600", "gfx1102", 32),
        gpu("AMD Radeon 780M Graphics", "gfx1103", 12),
        gpu("AMD Radeon(TM) Graphics", "gfx1103", 12),
        gpu("AMD Radeon 890M Graphics", "gfx1150", 16),
        gpu("AMD Radeon 8060S Graphics", "gfx1151", 40),
    ] {
        assert_eq!(
            capable(std::slice::from_ref(&supported), Some("26.10.1")),
            Ok(&supported)
        );
    }
    let small = capable(&[gpu("AMD Radeon 760M Graphics", "gfx1103", 8)], None).unwrap_err();
    assert!(
        small.contains("12 or more compute units")
            && small.contains("760M")
            && small.contains("has 8"),
        "{small}"
    );
    let rdna2 = capable(&[gpu("AMD Radeon RX 6800 XT", "gfx1030", 72)], None).unwrap_err();
    assert!(
        rdna2.contains("RDNA 3 and RDNA 4") && rdna2.contains("gfx1030"),
        "{rdna2}"
    );
    assert!(capable(&[gpu("Radeon Vega 8", "gfx90c", 8)], None).is_err());
    // An RDNA 2 integrated GPU beside an RX 7900 XT: the card qualifies.
    let both = [
        gpu("AMD Radeon(TM) Graphics", "gfx1036", 2),
        gpu("AMD Radeon RX 7900 XT", "gfx1100", 84),
    ];
    assert_eq!(capable(&both, None).unwrap().name, "AMD Radeon RX 7900 XT");
    assert!(capable(&[], None).unwrap_err().contains("finds no GPU"));
    let old = capable(&both, Some("25.10.2")).unwrap_err();
    assert!(
        old.contains("26.9.1 or newer") && old.contains("25.10.2"),
        "{old}"
    );
    assert!(capable(&both, Some("26.9.1")).is_ok());
    assert!(capable(&both, Some("unknown")).is_ok());
    assert_eq!(driver_version("26.10.1"), Some((26, 10, 1)));
    assert_eq!(driver_version("26.9"), Some((26, 9, 0)));
}
