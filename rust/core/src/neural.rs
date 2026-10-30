//! Neural Rendering for an app: a game mod that the host fetches from its
//! authors' GitHub releases on this PC, places beside the game's executable,
//! keeps on the release Rubylight pins ([`PINNED`]) and removes again. The mod is AMDNR by 3zwr1
//! (<https://github.com/3zwr1/AMD-NR---OptiScaler>, GPL-3.0), an OptiScaler
//! build that runs a neural rendering model on Radeon GPUs through a proxy
//! DLL. Rubylight never hosts, bundles or changes its files.
//!
//! This file decides: which release asset may be fetched, whether its
//! archive holds the whole mod, whether the files match its checksums, which
//! DLL name the mod loads under, what refuses a game folder (anti-cheat,
//! another copy of the mod) and what removal deletes. The host's `neural`
//! module downloads and moves the files.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The mod's repository. Downloads come from its releases only.
pub const REPOSITORY: &str = "3zwr1/AMD-NR---OptiScaler";

/// The mod's release Rubylight installs, pinned as Nucleus Co-op's is: no
/// other release is ever taken on its own. To move to another, change it
/// here and only here: the tag, and each archive's name and SHA-256 from
/// the release's SHA256SUMS.txt (GitHub's digest must say the same), then
/// run the host's `neural::tests::real_release_into_a_fake_game`.
pub const PINNED: Pin<'static> = Pin {
    tag: "Alpha0.3.5.8R2",
    package: PinnedFile {
        asset: "AMDNR-v0.3.5.8R2.zip",
        sha256: "a11143327ede14ff4d7fe50ea0ce9d148320aeec4cd163f11c8c9973c944a97d",
    },
    runtime: Some(PinnedFile {
        asset: "AMDNR-lmxxf-0.3.5.8R2.zip",
        sha256: "58ccdf75c1caace59826433c77a23a00c2c77dc9842868917bfc3be9a10f9607",
    }),
};

/// A release as Rubylight pins it ([`PINNED`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin<'a> {
    pub tag: &'a str,
    pub package: PinnedFile<'a>,
    /// The release's runtime component, when the package lacks the runtime.
    pub runtime: Option<PinnedFile<'a>>,
}

/// An archive of a pinned release: its name and SHA-256, lower-case hex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinnedFile<'a> {
    pub asset: &'a str,
    pub sha256: &'a str,
}
/// The largest archive the host downloads (the releases so far are 0.2 to 1.0 GB).
pub const MAX_ARCHIVE: u64 = 2 << 30;
/// The largest checksum list the host reads.
pub const MAX_SUMS: u64 = 1 << 20;
/// The record of what the host placed in a game folder.
pub const MANIFEST: &str = "rubylight-neural.json";
/// A checksum list: the release's own (of its archives) and, in some
/// packages, the archive's (of its files). It is checked, not placed.
pub const SUMS: &str = "SHA256SUMS.txt";
/// The mod's DLL, placed under one of [`PROXIES`].
pub const PROXY_SOURCE: &str = "OptiScaler.dll";
/// The neural runtime and its model: in the package, or in the release's
/// separate runtime component.
pub const RUNTIME: [&str; 2] = ["LmxxfNrRuntime.dll", "LmxxfNrRuntime.pak"];
/// What makes the whole mod: the mod, the neural runtime and its model,
/// which only work as files of one release.
pub const REQUIRED: [&str; 3] = [PROXY_SOURCE, RUNTIME[0], RUNTIME[1]];
/// The names the mod loads under, in the order its guide tries them.
pub const PROXIES: [&str; 7] = [
    "dxgi.dll",
    "d3d12.dll",
    "winmm.dll",
    "version.dll",
    "dbghelp.dll",
    "winhttp.dll",
    "wininet.dll",
];
/// The mod's settings, which its in-game menu rewrites.
pub const SETTINGS: &str = "OptiScaler.ini";
/// Logs the mod writes beside the game; removal deletes them with the mod.
const LOGS: [&str; 5] = [
    "OptiScaler.log",
    "amd_bridge.log",
    "amd_presr.log",
    "lmxxf_backend.log",
    "amdnr_crash.log",
];
/// What the manifest's `by` says; removal trusts no other manifest.
pub const PLACED_BY: &str = "Rubylight";

/// What a release asset the host may fetch is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asset<'a> {
    /// `AMDNR-v<version>.zip`: the mod's own package (GPL-3.0).
    Package(&'a str),
    /// `AMDNR-lmxxf-<version>.zip`: the neural runtime as the mod's authors
    /// distribute it beside the package since 0.3.5.8R1 (lmxxf by Kien, MIT).
    Runtime(&'a str),
    /// `SHA256SUMS.txt`: the release's checksums of its archives.
    Sums,
}

/// What a release asset is, or `None` for one that is never fetched. Only
/// the mod's own GPL package, the MIT runtime component its authors ship
/// with it and the release's checksum list may be. The releases also carry
/// other people's work under other terms: the danielblnc runtime zips
/// (`*-Runtime.zip`, `Daniel-*`) are all rights reserved and licensed to the
/// mod alone, the launcher is all rights reserved, and the Anywhere host,
/// the Mochi component and the HelixSR and d4r models are not the mod's to
/// pass on. None of it is fetched, and nothing from NVIDIA ever is.
pub fn asset(name: &str) -> Option<Asset<'_>> {
    if name == SUMS {
        return Some(Asset::Sums);
    }
    let version = |prefix: &str| {
        name.strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(".zip"))
            .filter(|version| valid_version(version))
    };
    version("AMDNR-v")
        .map(Asset::Package)
        .or_else(|| version("AMDNR-lmxxf-").map(Asset::Runtime))
}

/// Whether a release asset may be fetched at all; see [`asset`].
pub fn allowed_asset(name: &str) -> bool {
    asset(name).is_some()
}

/// `0.3.5.8` or `0.3.5.8R2`.
fn valid_version(version: &str) -> bool {
    let digits =
        |part: &str| (1..=4).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit());
    let (numbers, revision) = match version.split_once('R') {
        Some((numbers, revision)) => (numbers, Some(revision)),
        None => (version, None),
    };
    numbers.split('.').count() <= 5 && numbers.split('.').all(digits) && revision.is_none_or(digits)
}

/// One file of a release, as GitHub lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Download {
    pub asset: String,
    pub url: String,
    /// GitHub's SHA-256 of the file, lower-case hex.
    pub digest: String,
    pub size: u64,
}

/// What one release offers the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub tag: String,
    pub package: Download,
    /// The runtime component of the package's own version, when the
    /// release has one. A component of any other version is never paired.
    pub runtime: Option<Download>,
    /// The release's `SHA256SUMS.txt`.
    pub sums: Option<Download>,
}

/// Where release files are downloaded from.
pub const GITHUB: &str = "https://github.com";

/// The release `pin` names as GitHub lists it
/// (`/repos/{REPOSITORY}/releases/tags/{tag}`), its files at `origin`
/// ([`GITHUB`], or a test's own server): only with GitHub's digest of each
/// pinned archive the same as the pin's. A runtime component the pin does
/// not name is not taken.
pub fn pinned(listed: &Value, origin: &str, pin: &Pin) -> Result<Release, String> {
    let release = release(listed, origin)
        .filter(|release| release.tag == pin.tag)
        .ok_or_else(|| {
            format!(
                "GitHub does not list the mod's release {} with its package; nothing was downloaded",
                pin.tag
            )
        })?;
    let differs = |asset: &str| {
        format!(
            "GitHub lists {asset} of release {} with another SHA-256 than Rubylight pins; nothing was downloaded",
            pin.tag
        )
    };
    let same = |download: &Download, file: &PinnedFile| {
        download.asset == file.asset && download.digest == file.sha256
    };
    if !same(&release.package, &pin.package) {
        return Err(differs(pin.package.asset));
    }
    let runtime = match &pin.runtime {
        None => None,
        Some(file) => Some(
            release
                .runtime
                .clone()
                .filter(|runtime| same(runtime, file))
                .ok_or_else(|| differs(file.asset))?,
        ),
    };
    Ok(Release { runtime, ..release })
}

/// A release as GitHub lists it, when it offers the package. A draft, a
/// pre-release and a release whose package has no GitHub digest, an
/// unexpected address or size is not; a runtime component or checksum list
/// that has one of those faults is left out.
pub fn release(release: &Value, origin: &str) -> Option<Release> {
    if release["draft"] == true || release["prerelease"] == true {
        return None;
    }
    let tag = release["tag_name"].as_str()?;
    if tag.is_empty()
        || tag.len() > 64
        || !tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
        || tag.starts_with('.')
    {
        return None;
    }
    let assets = release["assets"].as_array()?;
    let download = |name: &str, limit: u64| {
        let asset = assets.iter().find(|asset| asset["name"] == name)?;
        let url = format!("{origin}/{REPOSITORY}/releases/download/{tag}/{name}");
        if asset["browser_download_url"] != url.as_str() {
            return None;
        }
        let digest = asset["digest"]
            .as_str()?
            .strip_prefix("sha256:")
            .filter(|d| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()))?;
        let size = asset["size"]
            .as_u64()
            .filter(|size| *size > 0 && *size <= limit)?;
        Some(Download {
            asset: name.into(),
            url,
            digest: digest.to_ascii_lowercase(),
            size,
        })
    };
    let mut packages =
        assets
            .iter()
            .filter_map(|asset| match self::asset(asset["name"].as_str()?)? {
                Asset::Package(version) => Some((asset["name"].as_str()?, version)),
                _ => None,
            });
    let (name, version) = packages.next()?;
    if packages.next().is_some() {
        return None;
    }
    Some(Release {
        tag: tag.into(),
        package: download(name, MAX_ARCHIVE)?,
        runtime: download(&format!("AMDNR-lmxxf-{version}.zip"), MAX_ARCHIVE),
        sums: download(SUMS, MAX_SUMS),
    })
}

/// Where a release's neural runtime comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runtime {
    Package,
    Component,
}

/// Whether a release holds the whole mod, from its archives' file lists:
/// the mod's DLL in the package, the neural runtime in the package or else
/// in the release's own runtime component, and a checksum list for each
/// archive used (the release's or the archive's own). Without them the
/// package alone would install a mod without its model.
pub fn complete(
    release: &Release,
    package: &[String],
    component: Option<&[String]>,
) -> Option<Runtime> {
    let has = |names: &[String], file: &str| names.iter().any(|name| name == file);
    let checked = |names: &[String]| release.sums.is_some() || has(names, SUMS);
    if !has(package, PROXY_SOURCE) || !checked(package) {
        return None;
    }
    if RUNTIME.iter().all(|file| has(package, file)) {
        return Some(Runtime::Package);
    }
    release.runtime.as_ref()?;
    let component = component?;
    (RUNTIME.iter().all(|file| has(component, file)) && checked(component))
        .then_some(Runtime::Component)
}

/// Each archive the host downloads must be in the release's checksum list
/// with the digest GitHub reports for it.
pub fn check_archives(
    archives: &[&Download],
    sums: &BTreeMap<String, String>,
) -> Result<(), String> {
    for archive in archives {
        match sums.get(&archive.asset) {
            None => return Err(format!("{} is not in the release's {SUMS}", archive.asset)),
            Some(listed) if *listed != archive.digest => {
                return Err(format!(
                    "{} does not match the release's {SUMS}",
                    archive.asset
                ));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// What the host takes from a runtime component: the runtime and its
/// licences. The component also carries notes for the mod's launcher that
/// clash with the package's own, and its authors say not to unpack it into
/// the game folder whole.
pub fn from_component(path: &str) -> bool {
    RUNTIME.contains(&path) || path.to_ascii_lowercase().starts_with("licenses/")
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?).into())
}
fn u32_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?).into())
}
fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// Where the central directory of a zip archive of `size` bytes lies, as
/// (offset, length), from the archive's last bytes `tail`. Lets the host read
/// a release's file list with two small range requests before it downloads.
pub fn directory_span(tail: &[u8], size: u64) -> Result<(u64, u64), String> {
    let start = size
        .checked_sub(tail.len() as u64)
        .ok_or("the archive is shorter than its end")?;
    // The end record is 22 bytes and a comment; the last signature whose
    // comment ends the archive is the record.
    let end = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&at| {
            tail[at..].starts_with(b"PK\x05\x06")
                && u16_at(tail, at + 20)
                    .is_some_and(|comment| at + 22 + comment as usize == tail.len())
        })
        .ok_or("the archive has no end record")?;
    let field =
        |at: usize, value: Option<u64>| value.ok_or(format!("the end record is cut at {at}"));
    let (mut offset, mut length) = (
        field(16, u32_at(tail, end + 16))?,
        field(12, u32_at(tail, end + 12))?,
    );
    if offset == 0xffff_ffff
        || length == 0xffff_ffff
        || field(10, u16_at(tail, end + 10))? == 0xffff
    {
        // Zip64: a locator right before the end record points at the larger record.
        let locator = end
            .checked_sub(20)
            .ok_or("the archive's zip64 locator is missing")?;
        if !tail[locator..].starts_with(b"PK\x06\x07") {
            return Err("the archive's zip64 locator is missing".into());
        }
        let record = field(8, u64_at(tail, locator + 8))?
            .checked_sub(start)
            .ok_or("the archive's zip64 end record is outside its last bytes")?
            as usize;
        if !tail
            .get(record..)
            .is_some_and(|r| r.starts_with(b"PK\x06\x06"))
        {
            return Err("the archive's zip64 end record is missing".into());
        }
        length = field(40, u64_at(tail, record + 40))?;
        offset = field(48, u64_at(tail, record + 48))?;
    }
    if offset.checked_add(length).is_none_or(|end| end > size) {
        return Err("the archive's directory lies outside it".into());
    }
    Ok((offset, length))
}

/// The entry names of a zip central directory.
pub fn directory_names(directory: &[u8]) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut at = 0;
    while at < directory.len() {
        if !directory[at..].starts_with(b"PK\x01\x02") {
            return Err(format!("the archive's directory is damaged at byte {at}"));
        }
        let cut = || format!("the archive's directory is cut at byte {at}");
        let name = u16_at(directory, at + 28).ok_or_else(cut)? as usize;
        let extra = u16_at(directory, at + 30).ok_or_else(cut)? as usize;
        let comment = u16_at(directory, at + 32).ok_or_else(cut)? as usize;
        let bytes = directory.get(at + 46..at + 46 + name).ok_or_else(cut)?;
        names.push(String::from_utf8_lossy(bytes).into_owned());
        at += 46 + name + extra + comment;
    }
    Ok(names)
}

/// `SHA256SUMS.txt`: `<sha256>  <path>` lines, as path (with `/`) to lower-case hash.
pub fn sums(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut sums = BTreeMap::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let (hash, path) = line
            .split_once(' ')
            .filter(|(hash, _)| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| format!("{SUMS} has a line that is no checksum: {line:?}"))?;
        let path = path.trim_start_matches([' ', '*']).replace('\\', "/");
        let path = path.trim_start_matches("./");
        let hash = hash.to_ascii_lowercase();
        if sums
            .insert(path.to_owned(), hash.clone())
            .is_some_and(|old| old != hash)
        {
            return Err(format!(
                "{SUMS} lists {path} twice with different checksums"
            ));
        }
    }
    Ok(sums)
}

/// Check an unpacked archive against its own checksum list: every file is
/// in it with the same SHA-256. `files` maps each path (with `/`) to its
/// SHA-256. Paths in the list without a file are other downloads' files
/// (the list also covers the separate runtime zips).
pub fn check_files(
    files: &BTreeMap<String, String>,
    sums: &BTreeMap<String, String>,
) -> Result<(), String> {
    for (path, hash) in files {
        if path == SUMS {
            continue;
        }
        match sums.get(path) {
            None => return Err(format!("{path} is not in the release's {SUMS}")),
            Some(listed) if listed != hash => {
                return Err(format!("{path} does not match the release's {SUMS}"));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// The whole mod is among the unpacked files.
pub fn check_whole<V>(files: &BTreeMap<String, V>) -> Result<(), String> {
    match REQUIRED
        .iter()
        .find(|required| !files.contains_key(**required))
    {
        Some(missing) => Err(format!("the release has no {missing}")),
        None => Ok(()),
    }
}

/// An archive entry's path inside the game folder (with `/`), or `None` for
/// one that would leave it or that Windows cannot name.
pub fn entry_path(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    let name = name.strip_suffix('/').unwrap_or(&name);
    if name.is_empty() || name.starts_with('/') {
        return None;
    }
    const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
    name.split('/')
        .all(|part| {
            let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            let device = RESERVED.contains(&stem.as_str())
                || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.len() == 4
                    && stem.as_bytes()[3].is_ascii_digit());
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with(['.', ' '])
                && !device
                && !part
                    .chars()
                    .any(|c| c.is_control() || "<>:\"|?*".contains(c))
        })
        .then(|| name.to_owned())
}

/// Documents of the package. One the game folder already has (a game's own
/// README or LICENSE) is left out rather than refusing the game.
pub fn document(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower == "license"
        || lower.starts_with("licenses/")
        || lower.starts_with("docs/")
        || (!lower.contains('/') && (lower.ends_with(".md") || lower.ends_with(".txt")))
}

/// What has one of the [`PROXIES`] names in the game folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Occupant {
    Free,
    /// The mod as the host placed it.
    Ours,
    /// Another program's file: a game DLL or another mod's loader.
    Foreign,
    /// Another copy of the mod, or of OptiScaler.
    OptiScaler,
}

/// The name the mod loads under: the one the host used before, else the
/// first free one. A name another program holds is never taken, and another
/// copy of OptiScaler refuses the game: two would fight over the same calls.
pub fn proxy(occupant: impl Fn(&str) -> Occupant) -> Result<&'static str, String> {
    let occupants: Vec<_> = PROXIES.iter().map(|name| (*name, occupant(name))).collect();
    if let Some((name, _)) = occupants.iter().find(|(_, o)| *o == Occupant::OptiScaler) {
        return Err(format!(
            "another copy of OptiScaler is installed in the game's folder as {name}: remove it first"
        ));
    }
    occupants
        .iter()
        .find(|(_, o)| *o == Occupant::Ours)
        .or_else(|| occupants.iter().find(|(_, o)| *o == Occupant::Free))
        .map(|(name, _)| *name)
        .ok_or_else(|| {
            format!(
                "other programs' files hold every name the mod can load under ({})",
                PROXIES.join(", ")
            )
        })
}

/// Another mod in the game's folder that replaces the same calls (another
/// OptiScaler, or an upscaler or frame-generation replacement), by one of its
/// file names. ReShade and other loaders that only share a DLL name are not
/// in the list: the mod takes another name beside them, as its own launcher does.
pub fn other_mod(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let starts = |prefix: &str| lower.starts_with(prefix);
    Some(match lower.as_str() {
        "optiscaler.dll" | "optiscaler.ini" | "optiscaler.asi" | "optiscaler" | "nvngx.ini" => {
            "OptiScaler"
        }
        "fakenvapi.dll" | "fakenvapi.ini" => "fakenvapi (an OptiScaler add-on)",
        "specialk64.dll" | "specialk32.dll" => "Special K",
        _ if starts("dlss-enabler") => "DLSS Enabler",
        _ if starts("dlssg_to_fsr3") => "dlssg-to-fsr3",
        _ if starts("uniscaler") => "Uniscaler",
        _ => return None,
    })
}

/// The anti-cheat a file or folder name in a game's install belongs to. The
/// mod injects a DLL into the game, which kernel and online anti-cheat treat
/// as cheating; such games are refused.
pub fn anti_cheat(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let starts = |prefix: &str| lower.starts_with(prefix);
    Some(match lower.as_str() {
        _ if starts("easyanticheat")
            || starts("eac_launcher")
            || lower == "start_protected_game.exe" =>
        {
            "Easy Anti-Cheat"
        }
        "battleye" => "BattlEye",
        _ if starts("beservice") || starts("beclient") || lower.ends_with("_be.exe") => "BattlEye",
        "vgc.exe" | "vgk.sys" | "riot vanguard" => "Riot Vanguard",
        _ if lower.contains("randgrid") => "Ricochet",
        _ if starts("mhyprot") => "mhyprot",
        "gameguard" | "nprotect" | "gameguard.des" => "nProtect GameGuard",
        _ if starts("npgg") && lower.ends_with(".des") => "nProtect GameGuard",
        "x3.xem" => "XIGNCODE3",
        _ if starts("xigncode") || starts("xhunter") => "XIGNCODE3",
        "pbsvc.exe" | "punkbuster" => "PunkBuster",
        _ if starts("pnkbstr") => "PunkBuster",
        _ if starts("equ8") => "EQU8",
        _ if starts("faceit") => "FACEIT",
        "anticheatexpert" => "Anti-Cheat Expert",
        _ if starts("sguard") || starts("ace-base") => "Anti-Cheat Expert",
        _ if starts("eaanticheat") => "EA Javelin",
        _ if lower.contains("denuvo") && lower.contains("anti") => "Denuvo Anti-Cheat",
        _ if starts("neacsafe") || starts("neaccontroller") => "NetEase Anti-Cheat",
        "hshield" | "ehsvc.dll" => "AhnLab HackShield",
        _ => return None,
    })
}

/// A file the host placed in the game folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placed {
    pub sha256: String,
    pub size: u64,
}

/// `rubylight-neural.json`: exactly what the host placed in one game folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// [`PLACED_BY`].
    pub by: String,
    /// The release the files came from.
    pub release: String,
    /// The name the mod loads under, one of [`PROXIES`].
    pub proxy: String,
    /// Each placed file by its path in the folder (with `/`).
    pub files: BTreeMap<String, Placed>,
    /// While an update runs: each file it replaces as the release before
    /// placed it. Such a file is the host's in either form, so an update cut
    /// short still leaves only files a removal takes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub previous: BTreeMap<String, Placed>,
    /// Folders the host created, parents first.
    #[serde(default)]
    pub folders: Vec<String>,
}

impl Manifest {
    /// A manifest the host wrote, with paths that stay inside the folder.
    pub fn parse(text: &str) -> Result<Self, String> {
        let manifest: Self =
            serde_json::from_str(text).map_err(|e| format!("{MANIFEST} is unreadable: {e}"))?;
        if manifest.by != PLACED_BY {
            return Err(format!("{MANIFEST} was not written by Rubylight"));
        }
        let inside = |path: &String| entry_path(path).as_ref() == Some(path);
        if !PROXIES.contains(&manifest.proxy.as_str())
            || !manifest.files.keys().all(inside)
            || !manifest
                .previous
                .keys()
                .all(|path| manifest.files.contains_key(path))
            || !manifest.folders.iter().all(inside)
        {
            return Err(format!("{MANIFEST} names a file outside the game folder"));
        }
        Ok(manifest)
    }
}

/// A placed file as it is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Now {
    Missing,
    /// As placed.
    Same,
    /// Changed since: someone else's now, unless it is the mod's settings.
    Changed,
}

/// What removal deletes and keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removal {
    pub delete: Vec<String>,
    /// Placed files that changed since; they may be someone else's now.
    pub keep: Vec<String>,
}

/// Removal of a manifest's files: those still as placed, and the mod's
/// settings however its menu changed them. Logs and folders follow in
/// [`logs`] and the manifest's `folders`.
pub fn removal(manifest: &Manifest, now: impl Fn(&str) -> Now) -> Removal {
    let mut removal = Removal::default();
    for path in manifest.files.keys() {
        match now(path) {
            Now::Missing => {}
            Now::Same => removal.delete.push(path.clone()),
            Now::Changed if path == SETTINGS => removal.delete.push(path.clone()),
            Now::Changed => removal.keep.push(path.clone()),
        }
    }
    removal
}

/// Whether `name` in the game folder is one of the mod's logs, which go with it.
pub fn logs(name: &str) -> bool {
    LOGS.iter().any(|log| log.eq_ignore_ascii_case(name))
        || (name.starts_with("OptiScaler.previous") && name.ends_with(".log"))
}

/// A GPU as the driver's compute runtime reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gpu {
    pub name: String,
    /// `gfx1100` and so on.
    pub arch: String,
    pub compute_units: u32,
    /// The adapter's LUID (low part, then high part), when the runtime says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub luid: Option<u64>,
}

/// The fewest compute units the mod's neural runtime runs on (integrated graphics).
pub const MIN_COMPUTE_UNITS: u32 = 12;
/// The oldest AMD Software the mod's runtime needs.
pub const MIN_DRIVER: (u32, u32, u32) = (26, 9, 1);

/// The GPU the mod can run on, or why none can: RDNA 3 and RDNA 4 graphics
/// with at least [`MIN_COMPUTE_UNITS`] (RX 7000 and RX 9000 cards, Strix
/// Halo, Radeon 780M, 880M and 890M class integrated graphics). RDNA 2 needs
/// a runtime the host does not fetch. `driver` is AMD Software's version,
/// None when the driver doesn't report one, which refuses: the runtime
/// needs [`MIN_DRIVER`]. Only the GPU the host streams with counts
/// (`streams`): matched by its LUID, else by its name when no other GPU the
/// runtime finds has that name.
pub fn capable<'a>(
    gpus: &'a [Gpu],
    driver: Option<&str>,
    streams: &Streams,
) -> Result<&'a Gpu, String> {
    let (major, minor, patch) = MIN_DRIVER;
    let needs = format!(
        "Neural Rendering needs AMD Software: Adrenalin Edition {major}.{minor}.{patch} or newer"
    );
    match driver.map(|text| (text, driver_version(text))) {
        None => return Err(format!("{needs}, and this PC's version can't be read")),
        Some((text, None)) => {
            return Err(format!(
                "{needs}, and this PC's version ({text}) can't be read"
            ));
        }
        Some((text, Some(version))) if version < MIN_DRIVER => {
            return Err(format!("{needs}; this PC has {text}"));
        }
        Some(_) => {}
    }
    if gpus.is_empty() {
        return Err("the AMD driver's compute runtime finds no GPU".into());
    }
    let gpu = match gpus.iter().find(|gpu| gpu.luid == Some(streams.luid)) {
        Some(gpu) => gpu,
        None => {
            let mut named = gpus.iter().filter(|gpu| gpu.name == streams.name);
            match (named.next(), named.next()) {
                (Some(gpu), None) => gpu,
                _ => {
                    return Err(format!(
                        "the AMD driver's compute runtime doesn't find the GPU Rubylight streams with ({})",
                        streams.name
                    ));
                }
            }
        }
    };
    let generation = |gpu: &Gpu| {
        let arch = gpu.arch.to_ascii_lowercase();
        let number = arch.strip_prefix("gfx").unwrap_or("");
        match number.len() {
            4 if number.starts_with("12") => 4,
            4 if number.starts_with("11") => 3,
            _ => 0,
        }
    };
    if generation(gpu) < 3 {
        return Err(format!(
            "Neural Rendering runs on RDNA 3 and RDNA 4 Radeon graphics; this PC's {} ({}) is older",
            gpu.name, gpu.arch
        ));
    }
    if gpu.compute_units < MIN_COMPUTE_UNITS {
        return Err(format!(
            "Neural Rendering needs a GPU with {MIN_COMPUTE_UNITS} or more compute units; this PC's {} has {}",
            gpu.name, gpu.compute_units
        ));
    }
    Ok(gpu)
}

/// The GPU the host captures and encodes with, as DXGI names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Streams {
    pub name: String,
    /// Its LUID: the low part, then the high part.
    pub luid: u64,
}

/// AMD Software's version (`26.10.1`) as numbers.
pub fn driver_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text
        .trim()
        .split(['.', ' ', '-'])
        .map(|part| part.parse::<u32>().ok());
    Some((
        parts.next()??,
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
    ))
}

#[cfg(test)]
mod tests;
