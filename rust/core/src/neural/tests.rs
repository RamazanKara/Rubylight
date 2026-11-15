use super::*;
use serde_json::json;

fn hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

#[test]
fn only_the_two_packages_and_the_checksums_may_be_fetched() {
    assert_eq!(asset("anr-fg-V.B16-full.zip"), Some(Asset::Full("V.B16")));
    assert_eq!(asset("anr-fg-V.B16-addon.zip"), Some(Asset::Addon("V.B16")));
    assert_eq!(asset(SUMS), Some(Asset::Sums));
    for name in [
        "anr-fg-V.B16-kit-windows.zip",
        "anr-fg-V.B16-kit-linux.tar.gz",
        "anr-fg-V.B16-full-optiscaler-source.zip",
        "anr-fg--full.zip",
        "anr-fg-V/B16-full.zip",
        "AMDNR-v0.3.5.8R2.zip",
        "AMDNR-lmxxf-0.3.5.8R2.zip",
        "sha256sums.txt",
        "install.ps1",
    ] {
        assert!(!allowed_asset(name), "{name}");
    }
    // The pin names only files the allowlist takes.
    for file in [PINNED.full, PINNED.addon, PINNED.sums] {
        assert!(allowed_asset(file.asset), "{}", file.asset);
        assert_eq!(file.sha256.len(), 64);
    }
}

fn listed(tag: &str, files: &[(&str, &str, u64)]) -> Value {
    json!({
        "tag_name": tag,
        "draft": false,
        "assets": files.iter().map(|(name, sha, size)| json!({
            "name": name,
            "size": size,
            "digest": format!("sha256:{sha}"),
            "browser_download_url": format!("{GITHUB}/{REPOSITORY}/releases/download/{tag}/{name}"),
        })).collect::<Vec<_>>(),
    })
}

fn pinned_list() -> Value {
    listed(
        PINNED.tag,
        &[
            (PINNED.full.asset, PINNED.full.sha256, 10_782_205),
            (PINNED.addon.asset, PINNED.addon.sha256, 3_156_952),
            (PINNED.sums.asset, PINNED.sums.sha256, 500),
            ("anr-fg-V.B16-kit-windows.zip", &"0".repeat(64), 1),
        ],
    )
}

#[test]
fn only_the_pinned_release_with_its_pinned_digests_is_taken() {
    let release = pinned(&pinned_list(), GITHUB, &PINNED).unwrap();
    assert_eq!(release.tag, "FG-V.B16");
    assert_eq!(release.full.asset, PINNED.full.asset);
    assert_eq!(release.full.digest, PINNED.full.sha256);
    assert_eq!(release.addon.size, 3_156_952);
    assert_eq!(
        release.sums.url,
        format!("{GITHUB}/apex-compute/ANR/releases/download/FG-V.B16/SHA256SUMS.txt")
    );
    // Another tag, a draft, a changed digest, a missing file, an odd address.
    let mut other = pinned_list();
    other["tag_name"] = json!("FG-V.B17");
    assert!(pinned(&other, GITHUB, &PINNED).is_err());
    let mut draft = pinned_list();
    draft["draft"] = json!(true);
    assert!(pinned(&draft, GITHUB, &PINNED).is_err());
    let mut changed = pinned_list();
    changed["assets"][1]["digest"] = json!(format!("sha256:{}", "1".repeat(64)));
    let error = pinned(&changed, GITHUB, &PINNED).unwrap_err();
    assert!(error.contains("another SHA-256"), "{error}");
    let mut sums = pinned_list();
    sums["assets"][2]["digest"] = json!(format!("sha256:{}", "2".repeat(64)));
    assert!(pinned(&sums, GITHUB, &PINNED).is_err());
    let mut gone = pinned_list();
    gone["assets"].as_array_mut().unwrap().remove(0);
    assert!(pinned(&gone, GITHUB, &PINNED).is_err());
    let mut moved = pinned_list();
    moved["assets"][0]["browser_download_url"] = json!("https://example.com/x.zip");
    assert!(pinned(&moved, GITHUB, &PINNED).is_err());
    let mut huge = pinned_list();
    huge["assets"][0]["size"] = json!(MAX_ARCHIVE + 1);
    assert!(pinned(&huge, GITHUB, &PINNED).is_err());
}

#[test]
fn every_archive_must_be_in_the_releases_checksums_with_its_digest() {
    let release = pinned(&pinned_list(), GITHUB, &PINNED).unwrap();
    let text = format!(
        "{}  {}\n{}  {}\n{}  anr-fg-V.B16-kit-windows.zip\n",
        PINNED.full.sha256,
        PINNED.full.asset,
        PINNED.addon.sha256.to_uppercase(),
        PINNED.addon.asset,
        "8".repeat(64)
    );
    let list = sums(&text).unwrap();
    assert_eq!(
        check_archives(&[&release.full, &release.addon], &list),
        Ok(())
    );
    let without = sums(&format!("{}  {}\n", PINNED.full.sha256, PINNED.full.asset)).unwrap();
    assert!(check_archives(&[&release.full, &release.addon], &without).is_err());
    let wrong = sums(&format!("{}  {}\n", "9".repeat(64), PINNED.full.asset)).unwrap();
    assert!(check_archives(&[&release.full], &wrong).is_err());
    assert!(sums("not a checksum").is_err());
    assert!(sums(&format!("{0}  a\n{1}  a\n", "1".repeat(64), "2".repeat(64))).is_err());
}

#[test]
fn the_archives_give_only_optiscaler_the_provider_and_the_licence() {
    for path in [
        "game/dxgi.dll",
        "game/OptiScaler.ini",
        "game/OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll",
        "LICENSES/OptiScaler-GPL-3.0.txt",
    ] {
        assert!(Archive::Full.takes(path), "{path}");
    }
    for path in [
        "install.ps1",
        "install.cmd",
        "README.txt",
        "nr_fg_package.json",
    ] {
        assert!(!Archive::Full.takes(path), "{path}");
        assert!(!Archive::Addon.takes(path), "{path}");
    }
    assert!(Archive::Addon.takes("Apex Neural Rendering FG/dlssg/nr_fg.ini"));
    assert!(!Archive::Addon.takes("game/dxgi.dll"));

    let mut files: BTreeMap<String, ()> = required().into_iter().map(|p| (p, ())).collect();
    assert_eq!(check_whole(&files), Ok(()));
    files.remove("addon/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll");
    assert!(check_whole(&files).unwrap_err().contains("addon/"));
}

#[test]
fn each_file_goes_where_its_mode_places_it() {
    let full = |path: &str| target(Mode::Full, path, "winmm.dll", "");
    assert_eq!(full("full/game/dxgi.dll").as_deref(), Some("winmm.dll"));
    assert_eq!(
        full("full/game/OptiScaler.ini").as_deref(),
        Some("OptiScaler.ini")
    );
    assert_eq!(
        full("full/game/OptiScaler/custom/Apex Neural Rendering FG/dlssg/x.nrx").as_deref(),
        Some("OptiScaler/custom/Apex Neural Rendering FG/dlssg/x.nrx")
    );
    assert_eq!(
        full("full/LICENSES/OptiScaler-GPL-3.0.txt").as_deref(),
        Some("OptiScaler/LICENSES/OptiScaler-GPL-3.0.txt")
    );
    assert_eq!(full("addon/Apex Neural Rendering FG/dlssg/x.dll"), None);
    let addon = |dir: &str| {
        target(
            Mode::Addon,
            "addon/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll",
            "",
            dir,
        )
    };
    assert_eq!(
        addon("").as_deref(),
        Some("custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll")
    );
    assert_eq!(
        addon("OptiScaler").as_deref(),
        Some("OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll")
    );
    assert_eq!(target(Mode::Addon, "full/game/dxgi.dll", "", ""), None);
    assert!(settings("OptiScaler.ini"));
    assert!(settings(
        "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nr_fg.ini"
    ));
    assert!(!settings(
        "OptiScaler/custom/Apex Neural Rendering FG/dlssg/path_table.json"
    ));
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
}

#[test]
fn optiscaler_never_takes_a_name_another_program_holds() {
    let with = |vulkan: bool, taken: &'static [(&'static str, Occupant)]| {
        proxy(vulkan, move |name| {
            taken
                .iter()
                .find(|(n, _)| *n == name)
                .map_or(Occupant::Free, |(_, o)| *o)
        })
    };
    assert_eq!(with(false, &[]), Ok("dxgi.dll"));
    // ReShade or a game's own DLL as dxgi.dll: the next name.
    assert_eq!(
        with(false, &[("dxgi.dll", Occupant::Foreign)]),
        Ok("winmm.dll")
    );
    assert_eq!(
        with(
            false,
            &[
                ("dxgi.dll", Occupant::Foreign),
                ("winmm.dll", Occupant::Foreign)
            ]
        ),
        Ok("version.dll")
    );
    // A game that only uses Vulkan never loads dxgi.dll.
    assert_eq!(with(true, &[]), Ok("winmm.dll"));
    assert_eq!(
        with(true, &[("winmm.dll", Occupant::Foreign)]),
        Ok("version.dll")
    );
    let vulkan = with(
        true,
        &[
            ("winmm.dll", Occupant::Foreign),
            ("version.dll", Occupant::Foreign),
        ],
    )
    .unwrap_err();
    assert!(
        vulkan.contains("every name") && !vulkan.contains("dxgi"),
        "{vulkan}"
    );
    // An earlier install keeps its name.
    assert_eq!(
        with(false, &[("version.dll", Occupant::Ours)]),
        Ok("version.dll")
    );
    assert!(
        with(
            false,
            &[
                ("dxgi.dll", Occupant::Foreign),
                ("winmm.dll", Occupant::Foreign),
                ("version.dll", Occupant::Foreign)
            ]
        )
        .unwrap_err()
        .contains("every name")
    );
}

#[test]
fn a_vulkan_game_and_an_optiscaler_with_providers_are_told_by_their_bytes() {
    let wide = |text: &str| text.bytes().flat_map(|b| [b, 0]).collect::<Vec<u8>>();
    assert!(vulkan_only(b"..VULKAN-1.dll..kernel32.dll"));
    assert!(vulkan_only(&wide("vulkan-1.dll")));
    assert!(!vulkan_only(b"vulkan-1.dll d3d12.dll"));
    assert!(!vulkan_only(
        &[b"vulkan-1.dll ".as_slice(), &wide("DXGI.dll")].concat()
    ));
    assert!(!vulkan_only(b"d3d11.dll"));
    assert!(custom_providers(
        b"..LoadLibraries Custom DLSSG {} initialized.."
    ));
    assert!(!custom_providers(b"FGNvngxReplacement Nukems Arturs"));
    assert!(dlss_frame_generation("nvngx_dlssg.dll") && dlss_frame_generation("SL.DLSS_G.DLL"));
    assert!(!dlss_frame_generation("nvngx_dlss.dll") && !dlss_frame_generation("sl.dlss.dll"));
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
        MANIFEST,
        LEGACY_MANIFEST,
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

fn placed(text: &str) -> Placed {
    Placed {
        sha256: hash(text),
        size: text.len() as u64,
    }
}

fn manifest() -> Manifest {
    Manifest {
        by: PLACED_BY.into(),
        release: "FG-V.B16".into(),
        proxy: "dxgi.dll".into(),
        mode: Mode::Full,
        switches: vec![Switch::FrameGeneration],
        files: [
            ("dxgi.dll", placed("dll")),
            ("OptiScaler.ini", placed("ini")),
            (
                "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nr_fg.ini",
                placed("fg ini"),
            ),
            (
                "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll",
                placed("fg"),
            ),
        ]
        .into_iter()
        .map(|(path, placed)| (path.to_string(), placed))
        .collect(),
        previous: BTreeMap::new(),
        folders: vec!["OptiScaler".into()],
        ini: None,
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
    outside
        .files
        .insert("../../Windows/System32/x.dll".into(), placed("x"));
    assert!(Manifest::parse(&serde_json::to_string(&outside).unwrap()).is_err());
    let mut folder = good.clone();
    folder.folders.push("C:/Windows".into());
    assert!(Manifest::parse(&serde_json::to_string(&folder).unwrap()).is_err());
    let mut ini = good.clone();
    ini.ini = Some(IniRecord {
        path: "../OptiScaler.ini".into(),
        before: BTreeMap::new(),
        section_added: false,
        break_added: false,
    });
    assert!(Manifest::parse(&serde_json::to_string(&ini).unwrap()).is_err());
    // An update's earlier forms only of files it names.
    let mut updating = good.clone();
    updating
        .previous
        .insert("dxgi.dll".into(), placed("old dll"));
    assert_eq!(
        Manifest::parse(&serde_json::to_string(&updating).unwrap()),
        Ok(updating.clone())
    );
    updating
        .previous
        .insert("../../Windows/System32/x.dll".into(), placed("x"));
    assert!(Manifest::parse(&serde_json::to_string(&updating).unwrap()).is_err());
    let mut name = good;
    name.proxy = "kernel32.dll".into();
    assert!(Manifest::parse(&serde_json::to_string(&name).unwrap()).is_err());
    assert!(Manifest::parse("{").is_err());
}

/// Rubylight 3.0's AMDNR manifest, as it wrote it, still reads: its proxy
/// may be any of the seven names, and it has no mode, switches or ini.
#[test]
fn a_rubylight_3_0_amdnr_manifest_still_reads() {
    let text = format!(
        r#"{{
  "by": "Rubylight",
  "release": "Alpha0.3.5.8R2",
  "proxy": "winhttp.dll",
  "files": {{
    "winhttp.dll": {{"sha256": "{dll}", "size": 3}},
    "OptiScaler.ini": {{"sha256": "{ini}", "size": 3}},
    "LmxxfNrRuntime.pak": {{"sha256": "{pak}", "size": 3}}
  }},
  "folders": ["OptiScaler"]
}}"#,
        dll = hash("dll"),
        ini = hash("ini"),
        pak = hash("pak")
    );
    let legacy = Manifest::parse(&text).unwrap();
    assert_eq!(legacy.proxy, "winhttp.dll");
    assert_eq!(legacy.mode, Mode::Full);
    assert!(legacy.switches.is_empty() && legacy.ini.is_none());
    let removal = removal(&legacy, |path| match path {
        "OptiScaler.ini" => Now::Changed,
        "winhttp.dll" => Now::Changed,
        _ => Now::Same,
    });
    assert_eq!(removal.delete, ["LmxxfNrRuntime.pak", "OptiScaler.ini"]);
    assert_eq!(removal.keep, ["winhttp.dll"]);
}

#[test]
fn removal_deletes_what_is_still_as_placed_and_keeps_what_changed() {
    let removal = removal(&manifest(), |path| match path {
        // Replaced by someone: no longer provably the mod.
        "dxgi.dll" => Now::Changed,
        // The menu saved its settings, and the user edited the provider's.
        "OptiScaler.ini" => Now::Changed,
        "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nr_fg.ini" => Now::Changed,
        "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll" => Now::Same,
        _ => Now::Missing,
    });
    assert_eq!(
        removal.delete,
        [
            "OptiScaler.ini",
            "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nr_fg.ini",
            "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll"
        ]
    );
    assert_eq!(removal.keep, ["dxgi.dll"]);
    assert!(
        logs("OptiScaler.log")
            && logs("lmxxf_backend.log")
            && logs("OptiScaler.previous-1.Game.exe.log")
    );
    assert!(!logs("Game.log") && !logs("OptiScaler.ini"));
    assert!(provider_logs("nr_fg-20261114-101500.log") && !provider_logs("nr_fg.ini"));
}

/// An ini as `lines` joined with `ending`, a BOM in front when `bom`.
fn ini(lines: &[&str], ending: &str, bom: bool, trailing: bool) -> Vec<u8> {
    let mut text = lines.join(ending);
    if trailing {
        text.push_str(ending);
    }
    let mut bytes = if bom {
        b"\xef\xbb\xbf".to_vec()
    } else {
        Vec::new()
    };
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

fn values(bytes: &[u8]) -> Vec<Option<String>> {
    let ini = Ini::read(bytes).unwrap();
    INI_VALUES
        .iter()
        .map(|(key, _)| ini.get(INI_SECTION, key).unwrap())
        .collect()
}

#[test]
fn the_addon_sets_three_values_and_puts_back_exactly_what_was_there() {
    let stock = [
        "; OptiScaler",
        "[FrameGen]",
        "; Selected FG Input",
        "FGInput = auto",
        "",
        "FGNvngxReplacement=auto",
        "",
        "DisableHudless=auto",
        "",
        // As the release's OptiScaler.ini has them: [DLSSG] without the key,
        // [NvngxFG] with its own DisableHudless.
        "[DLSSG]",
        "Enabled=auto",
        "",
        "[NvngxFG]",
        "DisableHudless = auto",
        "",
        "[Libraries]",
        "OptiDllPath=auto",
    ];
    let partial = [
        "[FrameGen]",
        "Enabled=true",
        "",
        "[Libraries]",
        "OptiDllPath=auto",
    ];
    let none = ["[Upscalers]", "Dx12Upscaler=fsr31"];
    for lines in [&stock[..], &partial[..], &none[..], &[][..]] {
        for ending in ["\r\n", "\n"] {
            for bom in [false, true] {
                for trailing in [false, true] {
                    let before = ini(lines, ending, bom, trailing);
                    let (after, record) = ini_apply(&before, "OptiScaler.ini", None).unwrap();
                    assert_eq!(
                        values(&after),
                        [
                            Some("nvngxfg".into()),
                            Some(PROVIDER.into()),
                            Some("false".into())
                        ],
                        "{lines:?} {ending:?}"
                    );
                    // Its line endings and byte order mark stay.
                    assert_eq!(after.starts_with(b"\xef\xbb\xbf"), bom);
                    let text = String::from_utf8_lossy(&after);
                    if ending == "\r\n" && !lines.is_empty() {
                        assert!(!text.replace("\r\n", "").contains('\n'), "{text:?}");
                    }
                    // The other section's DisableHudless is not touched.
                    if lines == stock {
                        assert!(text.contains("DisableHudless = auto"), "{text:?}");
                    }
                    let restored = ini_restore(&after, &record).unwrap();
                    assert_eq!(
                        String::from_utf8_lossy(&restored),
                        String::from_utf8_lossy(&before),
                        "{lines:?} {ending:?} bom {bom} trailing {trailing}"
                    );
                    // A second apply keeps the first record.
                    let (again, kept) = ini_apply(&after, "OptiScaler.ini", Some(&record)).unwrap();
                    assert_eq!(again, after);
                    assert_eq!(kept, record);
                }
            }
        }
    }
    // OptiScaler's menu changed a value since: it is put back all the same.
    let before = ini(&stock, "\r\n", false, true);
    let (after, record) = ini_apply(&before, "OptiScaler.ini", None).unwrap();
    let edited = String::from_utf8(after)
        .unwrap()
        .replace("FGInput=nvngxfg", "FGInput=fsrfg");
    assert_eq!(ini_restore(edited.as_bytes(), &record).unwrap(), before);
    // What it can't change and put back exactly, it refuses.
    assert!(ini_apply(b"\xff\xfe[\x00", "OptiScaler.ini", None).is_err());
    let twice = ini(&["[FrameGen]", "FGInput=a", "FGInput=b"], "\n", false, true);
    assert!(
        ini_apply(&twice, "OptiScaler.ini", None)
            .unwrap_err()
            .contains("twice")
    );
    let sections = ini(&["[FrameGen]", "[framegen]"], "\n", false, true);
    assert!(ini_apply(&sections, "OptiScaler.ini", None).is_err());
}

#[test]
fn optiscalers_folder_comes_from_its_ini_and_stays_in_the_game_folder() {
    let with = |value: &str| {
        ini(
            &["[Libraries]", &format!("OptiDllPath={value}")],
            "\r\n",
            false,
            true,
        )
    };
    assert_eq!(opti_dir(&with("auto"), true).as_deref(), Ok("OptiScaler"));
    assert_eq!(opti_dir(&with("auto"), false).as_deref(), Ok(""));
    assert_eq!(opti_dir(b"[FrameGen]\n", true).as_deref(), Ok("OptiScaler"));
    assert_eq!(
        opti_dir(&with(".\\Mods\\Opti\\"), false).as_deref(),
        Ok("Mods/Opti")
    );
    assert_eq!(
        opti_dir(&with("\".\\OptiScaler\""), false).as_deref(),
        Ok("OptiScaler")
    );
    assert_eq!(opti_dir(&with("."), true).as_deref(), Ok(""));
    for outside in [
        "C:\\Mods",
        "..\\Mods",
        "\\\\server\\share",
        "/x",
        "a/../../b",
    ] {
        assert!(opti_dir(&with(outside), true).is_err(), "{outside}");
    }
}

fn streams(vendor: u32, device: u32) -> Streams {
    Streams {
        name: "GPU".into(),
        luid: 7,
        vendor,
        device,
    }
}

fn gpu(arch: &str, luid: Option<u64>) -> Gpu {
    Gpu {
        name: "GPU".into(),
        arch: arch.into(),
        compute_units: 12,
        luid,
    }
}

#[test]
fn the_switch_is_offered_on_the_gpus_the_provider_runs_on() {
    let none: [&str; 0] = [];
    // NVIDIA: Turing (GTX 16, RTX 20) and every newer device ID.
    assert!(capable(&streams(NVIDIA, 0x1e04), Err("no HIP"), &none).is_ok());
    assert!(capable(&streams(NVIDIA, 0x2684), Err("no HIP"), &none).is_ok());
    assert!(capable(&streams(NVIDIA, 0x3100), Err("no HIP"), &none).is_ok());
    let pascal = capable(&streams(NVIDIA, 0x1b80), Err("no HIP"), &none).unwrap_err();
    assert!(pascal.contains("older"), "{pascal}");
    // AMD by the compute runtime's architecture.
    for arch in ["gfx1010", "gfx1030", "gfx1103", "gfx1151", "gfx1201"] {
        assert!(
            capable(&streams(AMD, 0xffff), Ok(&[gpu(arch, Some(7))]), &none).is_ok(),
            "{arch}"
        );
    }
    let vega = capable(&streams(AMD, 0x15d8), Ok(&[gpu("gfx90c", Some(7))]), &none).unwrap_err();
    assert!(vega.contains("older"), "{vega}");
    // By name when the runtime gives no LUID, and only when the name is unique.
    assert!(capable(&streams(AMD, 0xffff), Ok(&[gpu("gfx1100", None)]), &none).is_ok());
    // The runtime can't say: the PCI device ID (this laptop's 780M is 0x1900).
    for device in [0x1900, 0x15bf, 0x744c, 0x7550, 0x731f, 0x150e] {
        assert!(
            capable(&streams(AMD, device), Err("no HIP"), &none).is_ok(),
            "{device:#x}"
        );
    }
    let unknown = capable(&streams(AMD, 0x15d8), Err("no HIP"), &none).unwrap_err();
    assert!(
        unknown.contains("can't identify") && unknown.contains("no HIP"),
        "{unknown}"
    );
    let twins = [gpu("gfx1100", None), gpu("gfx1100", None)];
    assert!(
        capable(&streams(AMD, 0x15d8), Ok(&twins), &none)
            .unwrap_err()
            .contains("can't identify")
    );
    // Neither AMD nor NVIDIA.
    assert!(capable(&streams(0x8086, 0x56a0), Err("no HIP"), &none).is_err());
    // The GPU comes before the Visual C++ runtime OptiScaler links: a PC
    // whose GPU can't run it is not sent to install the runtime.
    let older = capable(
        &streams(NVIDIA, 0x1b80),
        Err("no HIP"),
        &["VCRUNTIME140_1.dll"],
    )
    .unwrap_err();
    assert!(
        older.contains("which is older") && !older.contains("Visual C++"),
        "{older}"
    );
    let missing = capable(
        &streams(NVIDIA, 0x2684),
        Err("no HIP"),
        &["VCRUNTIME140_1.dll"],
    )
    .unwrap_err();
    assert!(
        missing.contains("Visual C++") && missing.contains("VCRUNTIME140_1.dll"),
        "{missing}"
    );
}

/// What reaches a console or a client never names the mod: a path through
/// its folder shows as its file's name.
#[test]
fn shown_text_names_no_mod() {
    let dll = format!("OptiScaler/custom/{PROVIDER}/dlssg/nvngx_dlssg.dll");
    assert_eq!(shown(&dll), "frame generation's nvngx_dlssg.dll");
    assert_eq!(
        shown(&format!(
            "{dll} changed since it was placed; nothing was removed."
        )),
        "frame generation's nvngx_dlssg.dll changed since it was placed; nothing was removed."
    );
    assert_eq!(
        shown(&format!(
            "custom/{PROVIDER} is already in the game's folder"
        )),
        "frame generation's folder is already in the game's folder"
    );
    assert_eq!(
        shown(&format!(
            r"cannot write C:\Games\X\OptiScaler\custom\{PROVIDER}\dlssg\nr_fg.ini: denied"
        )),
        "cannot write frame generation's nr_fg.ini: denied"
    );
    // A stop character of more than one byte before the path.
    for stop in ['\u{3000}', '\u{a0}'] {
        assert_eq!(
            shown(&format!("missing{stop}D:\\G\\OptiScaler\\custom\\{PROVIDER}\\dlssg\\x.dll")),
            format!("missing{stop}frame generation's x.dll")
        );
    }
    assert_eq!(shown("the game uses BattlEye"), "the game uses BattlEye");
}

#[test]
fn switches_have_stable_ids() {
    assert_eq!(Switch::FrameGeneration.id(), "frame-generation");
    assert_eq!(
        Switch::from_id("frame-generation"),
        Some(Switch::FrameGeneration)
    );
    assert_eq!(Switch::from_id("upscaling"), None);
    assert_eq!(
        serde_json::to_value(Switch::FrameGeneration).unwrap(),
        json!("frame-generation")
    );
}
