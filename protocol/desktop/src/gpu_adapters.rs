//! The GPUs Direct3D 11 offers on Windows, and which of them Nightjar runs on. SDL's Direct3D 11
//! renderer draws on the first adapter, and D3D11VA decodes on the renderer's device. On a PC with
//! two GPUs (a laptop's integrated and dedicated ones) Windows orders them by the app's graphics
//! preference, which it reads when the app starts: the Graphics card setting writes that
//! preference for Nightjar and starts Nightjar again when the running one draws on the other GPU.
//! Automatic leaves the choice to Windows unless YUV 4:4:4 is asked for and only the
//! high-performance GPU decodes it (an integrated Radeon beside an RTX, Rubylight issue 84).

use std::ffi::{c_void, OsString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::OnceLock;

use windows::core::{w, Interface, GUID, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HMODULE, LUID, WIN32_ERROR};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11VideoDevice, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_AYUV, DXGI_FORMAT_Y410};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIDevice, IDXGIFactory1, IDXGIFactory6, DXGI_ADAPTER_DESC1, DXGI_ADAPTER_FLAG_SOFTWARE,
    DXGI_GPU_PREFERENCE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE, DXGI_GPU_PREFERENCE_MINIMUM_POWER,
};
use windows::Win32::System::Registry::{RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ};

use crate::config::GraphicsCard;
use crate::strings::trf;

/// DXVA's HEVC Range Extension profiles FFmpeg decodes HEVC 4:4:4 with (`ff_DXVA2_ModeHEVC_VLD_Main_444`
/// and `_Main10_444` in its dxva2.c; FFmpeg 8.1's avcodec carries both).
const HEVC_MAIN_444: GUID = GUID::from_values(0x4008_018f, 0xf537, 0x4b36, [0x98, 0xcf, 0x61, 0xaf, 0x8a, 0x2c, 0x1a, 0x33]);
const HEVC_MAIN10_444: GUID = GUID::from_values(0x0dab_effa, 0x4458, 0x4602, [0xbc, 0x03, 0x07, 0x95, 0x65, 0x9d, 0x61, 0x7c]);

/// The profile and the surface format a decoder of HEVC 4:4:4 (`ten_bit`: 10-bit) decodes into.
fn hevc444_profile(ten_bit: bool) -> (GUID, DXGI_FORMAT) {
    if ten_bit { (HEVC_MAIN10_444, DXGI_FORMAT_Y410) } else { (HEVC_MAIN_444, DXGI_FORMAT_AYUV) }
}

fn name_of(description: &[u16]) -> String {
    let end = description.iter().position(|&c| c == 0).unwrap_or(description.len());
    String::from_utf16_lossy(&description[..end]).trim().to_owned()
}

/// The renderer's GPU: its name and LUID. `device` is the renderer's `ID3D11Device`.
fn renderer_adapter(device: *mut c_void) -> Option<(String, LUID)> {
    // SAFETY: A live device pointer from the renderer, only borrowed here.
    let device = unsafe { ID3D11Device::from_raw_borrowed(&device) }?;
    // SAFETY: Plain DXGI queries on the borrowed device.
    unsafe {
        let desc = device.cast::<IDXGIDevice>().ok()?.GetAdapter().ok()?.GetDesc().ok()?;
        Some((name_of(&desc.Description), desc.AdapterLuid))
    }
}

/// The name of the GPU the renderer draws and D3D11VA decodes on, for the log.
pub fn renderer_gpu(device: *mut c_void) -> Option<String> {
    renderer_adapter(device).map(|(name, _)| name)
}

/// Whether `adapter`'s video decoder has `profile` and decodes it into `format`.
unsafe fn decodes(adapter: &IDXGIAdapter1, (profile, format): (GUID, DXGI_FORMAT)) -> bool {
    let mut device = None;
    if D3D11CreateDevice(adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_VIDEO_SUPPORT, None,
        D3D11_SDK_VERSION, Some(&mut device), None, None).is_err() {
        return false;
    }
    let Some(video) = device.and_then(|device| device.cast::<ID3D11VideoDevice>().ok()) else { return false };
    (0..video.GetVideoDecoderProfileCount()).any(|index| {
        video.GetVideoDecoderProfile(index).is_ok_and(|found| found == profile)
            && video.CheckVideoDecoderFormat(&profile, format).is_ok_and(|ok| ok.as_bool())
    })
}

/// The GPUs other than the renderer's, by name, that decode HEVC 4:4:4 (`ten_bit`: Main10 4:4:4),
/// with the renderer's: None when the renderer's device is unknown.
pub fn others_decoding_hevc444(device: *mut c_void, ten_bit: bool) -> Option<(String, Vec<String>)> {
    let (renderer, luid) = renderer_adapter(device)?;
    let mut others = Vec::new();
    // SAFETY: Plain DXGI and D3D11 calls; every device made here is its own and dropped here.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        for index in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(index) else { break };
            let Ok(desc) = adapter.GetDesc1() else { continue };
            let same = desc.AdapterLuid.LowPart == luid.LowPart && desc.AdapterLuid.HighPart == luid.HighPart;
            if same || desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            if decodes(&adapter, hevc444_profile(ten_bit)) {
                others.push(name_of(&desc.Description));
            }
        }
    }
    Some((renderer, others))
}

/// What to tell the player when the renderer's GPU (`renderer`) can't decode 4:4:4 and `other`
/// can: with Automatic, Nightjar starts on `other` from its next start ([`choose_at_start`] sees
/// the 4:4:4 wish then); with a GPU chosen, the setting to change.
pub fn hint(renderer: &str, other: &str) -> String {
    if choice() == GraphicsCard::Automatic {
        trf("{} can’t decode it; {} can, and Nightjar uses it from its next start", &[&renderer, &other])
    } else {
        trf("{} can’t decode it; {} can: choose it in Settings › Video › Graphics card", &[&renderer, &other])
    }
}

/// One of the two GPUs Windows chooses between.
#[derive(Debug)]
pub struct Gpu {
    pub name: String,
    luid: (u32, i32),
    /// Whether its video decoder decodes HEVC 4:4:4, once asked.
    hevc444: OnceLock<bool>,
}

impl Gpu {
    fn of(desc: &DXGI_ADAPTER_DESC1) -> Option<Self> {
        (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0).then(|| Self {
            name: name_of(&desc.Description),
            luid: (desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart),
            hevc444: OnceLock::new(),
        })
    }

    /// Whether this GPU decodes 8-bit HEVC 4:4:4 (every one that decodes the 10-bit kind does too).
    pub fn decodes_hevc444(&self) -> bool {
        *self.hevc444.get_or_init(|| {
            // SAFETY: Plain DXGI and D3D11 calls; the devices made are dropped here.
            unsafe {
                let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return false };
                (0..).map_while(|index| factory.EnumAdapters1(index).ok()).any(|adapter| {
                    adapter.GetDesc1().is_ok_and(|desc| (desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart) == self.luid)
                        && decodes(&adapter, hevc444_profile(false))
                })
            }
        })
    }
}

/// The power-saving and the high-performance GPU, when this PC has two.
#[derive(Debug)]
pub struct Gpus {
    pub power_saving: Gpu,
    pub high_performance: Gpu,
}

impl Gpus {
    fn preferred(&self, preference: u32) -> &Gpu {
        if preference == PREFER_HIGH_PERFORMANCE { &self.high_performance } else { &self.power_saving }
    }
}

/// This PC's two GPUs as Windows ranks them whatever Nightjar's preference (None with one GPU).
pub fn gpus() -> Option<&'static Gpus> {
    static GPUS: OnceLock<Option<Gpus>> = OnceLock::new();
    GPUS.get_or_init(|| {
        // SAFETY: Plain DXGI calls.
        let gpus = unsafe {
            let ranked = |factory: &IDXGIFactory6, preference: DXGI_GPU_PREFERENCE| {
                factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(0, preference).ok()?.GetDesc1().ok().as_ref().and_then(Gpu::of)
            };
            CreateDXGIFactory1::<IDXGIFactory6>().ok().and_then(|factory| {
                let power_saving = ranked(&factory, DXGI_GPU_PREFERENCE_MINIMUM_POWER)?;
                let high_performance = ranked(&factory, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE)?;
                (power_saving.luid != high_performance.luid).then_some(Gpus { power_saving, high_performance })
            })
        };
        if let Some(gpus) = &gpus {
            eprintln!("nightjar: GPUs: {} saves power, {} performs", gpus.power_saving.name, gpus.high_performance.name);
        }
        gpus
    }).as_ref()
}

/// The GPU the renderer would take now: the first adapter.
fn first_gpu() -> Option<(u32, i32)> {
    // SAFETY: Plain DXGI calls.
    unsafe {
        let desc = CreateDXGIFactory1::<IDXGIFactory1>().ok()?.EnumAdapters1(0).ok()?.GetDesc1().ok()?;
        Some((desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart))
    }
}

/// Windows' per-app graphics preferences, what Settings › System › Display › Graphics writes: a
/// value named after the program's path, "GpuPreference=N;" among other "key=value;" pairs.
const PREFERENCES: PCWSTR = w!(r"Software\Microsoft\DirectX\UserGpuPreferences");
const PREFER_POWER_SAVING: u32 = 1;
const PREFER_HIGH_PERFORMANCE: u32 = 2;
/// The preference value Nightjar wrote last, so Automatic takes back only its own.
const WRITTEN: &str = "gpu-preference.txt";

/// The preference value `preference` asks for in place of `existing`'s (None: no GpuPreference),
/// the other pairs kept; None when nothing is left.
pub(crate) fn with_preference(existing: Option<&str>, preference: Option<u32>) -> Option<String> {
    let mut pairs: Vec<String> = existing.unwrap_or_default().split(';').map(str::trim)
        .filter(|pair| !pair.is_empty() && !pair.starts_with("GpuPreference="))
        .map(str::to_owned).collect();
    pairs.extend(preference.map(|preference| format!("GpuPreference={preference}")));
    (!pairs.is_empty()).then(|| pairs.iter().map(|pair| format!("{pair};")).collect())
}

/// The preference Nightjar sets: the GPU chosen, and with Automatic the high-performance GPU when
/// YUV 4:4:4 is asked for and only it decodes it; None leaves the choice to Windows.
pub(crate) fn wanted(choice: GraphicsCard, wants444: bool, power_saving_decodes: impl FnOnce() -> bool, performance_decodes: impl FnOnce() -> bool) -> Option<u32> {
    match choice {
        GraphicsCard::PowerSaving => Some(PREFER_POWER_SAVING),
        GraphicsCard::HighPerformance => Some(PREFER_HIGH_PERFORMANCE),
        GraphicsCard::Automatic => (wants444 && !power_saving_decodes() && performance_decodes()).then_some(PREFER_HIGH_PERFORMANCE),
    }
}

/// The new value for a preference that is `current`, given what Nightjar wrote last (`ours`): None
/// when it stays. With `wanted` None only Nightjar's own preference is taken back.
pub(crate) fn next_value(current: Option<&str>, ours: Option<&str>, wanted: Option<u32>) -> Option<Option<String>> {
    let next = match wanted {
        Some(_) => with_preference(current, wanted),
        None if current.is_some() && current == ours => with_preference(current, None),
        None => return None,
    };
    (next.as_deref() != current).then_some(next)
}

fn checked(error: WIN32_ERROR) -> Result<(), String> {
    if error == ERROR_SUCCESS { Ok(()) } else { Err(windows::core::Error::from_hresult(error.to_hresult()).message()) }
}

fn read_preference(program: &HSTRING) -> Result<Option<String>, String> {
    let mut size = 0u32;
    // SAFETY: NUL-terminated strings, and the out size lives across the call.
    let answer = unsafe { RegGetValueW(HKEY_CURRENT_USER, PREFERENCES, program, RRF_RT_REG_SZ, None, None, Some(&mut size)) };
    if answer == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    checked(answer)?;
    let mut text = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: as above, and the buffer holds `size` bytes.
    checked(unsafe { RegGetValueW(HKEY_CURRENT_USER, PREFERENCES, program, RRF_RT_REG_SZ, None, Some(text.as_mut_ptr().cast()), Some(&mut size)) })?;
    Ok(Some(name_of(&text)))
}

fn write_preference(program: &HSTRING, value: Option<&str>) -> Result<(), String> {
    let Some(value) = value else {
        // SAFETY: NUL-terminated strings.
        let answer = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, PREFERENCES, program) };
        return if answer == ERROR_FILE_NOT_FOUND { Ok(()) } else { checked(answer) };
    };
    let text = HSTRING::from(value);
    let bytes = (text.len() + 1) * 2;
    // SAFETY: an HSTRING's buffer ends in a NUL, so `bytes` of it can be read.
    checked(unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, PREFERENCES, program, REG_SZ.0, Some(text.as_ptr().cast()), bytes as u32) })
}

/// Sets `program`'s preference to `wanted` (None: Windows' choice, taking back only Nightjar's
/// own), remembering what it wrote in `dir`. Whether it changed.
fn set_preference(program: &Path, wanted: Option<u32>, dir: &Path) -> Result<bool, String> {
    let program = HSTRING::from(program.as_os_str());
    let current = read_preference(&program)?;
    let ours = std::fs::read_to_string(dir.join(WRITTEN)).ok();
    let Some(next) = next_value(current.as_deref(), ours.as_deref(), wanted) else { return Ok(false) };
    write_preference(&program, next.as_deref())?;
    let _ = match &next {
        Some(value) => std::fs::write(dir.join(WRITTEN), value),
        None => std::fs::remove_file(dir.join(WRITTEN)),
    };
    eprintln!("nightjar: graphics preference {current:?} -> {next:?}");
    Ok(true)
}

/// The Graphics card setting in effect, for [`hint`].
static CHOICE: AtomicU8 = AtomicU8::new(0);
/// Nightjar is to start again once it has closed ([`restart_requested`]): 1 with this start's
/// command line (a start that ended before its window), 2 without (a setting changed).
static RESTART: AtomicU8 = AtomicU8::new(0);
/// What a start that Nightjar made itself carries, so it never starts again for the same reason.
pub const RESTARTED: &str = "--graphics-card-restart";

fn choice() -> GraphicsCard {
    match CHOICE.load(Ordering::Relaxed) { 1 => GraphicsCard::PowerSaving, 2 => GraphicsCard::HighPerformance, _ => GraphicsCard::Automatic }
}

fn remember(choice: GraphicsCard) {
    CHOICE.store(match choice { GraphicsCard::Automatic => 0, GraphicsCard::PowerSaving => 1, GraphicsCard::HighPerformance => 2 }, Ordering::Relaxed);
}

/// This start is one Nightjar made itself ([`split_restarted`]).
static STARTED_AGAIN: AtomicBool = AtomicBool::new(false);

/// Takes [`RESTARTED`] out of the command line, and says whether it was there.
pub fn split_restarted(mut args: Vec<OsString>) -> (Vec<OsString>, bool) {
    let before = args.len();
    args.retain(|arg| arg != RESTARTED);
    let found = args.len() != before;
    STARTED_AGAIN.store(found, Ordering::Relaxed);
    (args, found)
}

/// Sets the preference for `choice` (`wants444`: some layer asks for YUV 4:4:4) and says whether
/// it changed and the GPU the renderer would take now is not the one preferred.
fn choose(choice: GraphicsCard, wants444: bool, dir: &Path) -> Option<(bool, bool, &'static Gpus, Option<u32>)> {
    remember(choice);
    let gpus = gpus()?;
    let program = std::env::current_exe().ok()?;
    let wanted = wanted(choice, wants444, || gpus.power_saving.decodes_hevc444(), || gpus.high_performance.decodes_hevc444());
    let changed = set_preference(&program, wanted, dir).unwrap_or_else(|why| {
        eprintln!("nightjar: the graphics preference cannot be set: {why}");
        false
    });
    let elsewhere = wanted.is_some_and(|preference| Some(gpus.preferred(preference).luid) != first_gpu());
    Some((changed, elsewhere, gpus, wanted))
}

/// Before the renderer is made, with the settings of every layer. True when this start should end
/// and Nightjar start again because the renderer would draw on the other GPU; never for a start
/// Nightjar made itself ([`split_restarted`]), so it starts again at most once.
pub fn choose_at_start(choice: GraphicsCard, wants444: bool, dir: &Path) -> bool {
    let restarted = STARTED_AGAIN.load(Ordering::Relaxed);
    let Some((changed, elsewhere, gpus, wanted)) = choose(choice, wants444, dir) else { return false };
    eprintln!("nightjar: graphics card {choice:?}{}, preferring {}", if wants444 { " with YUV 4:4:4 asked for" } else { "" },
        wanted.map_or("Windows' choice", |preference| gpus.preferred(preference).name.as_str()));
    if elsewhere && restarted {
        eprintln!("nightjar: the renderer is not on the GPU preferred after a restart; Nightjar stays on it");
    }
    let restart = changed && elsewhere && !restarted;
    if restart {
        eprintln!("nightjar: starting again so that Windows draws Nightjar on the GPU preferred");
        RESTART.store(1, Ordering::Relaxed);
    }
    restart
}

/// A Graphics card setting changed with nothing streaming: sets the preference. True when Nightjar
/// should close and start again ([`restart_requested`]) because the renderer is on the other GPU.
pub fn choose_now(choice: GraphicsCard, wants444: bool, dir: &Path) -> bool {
    let restart = choose(choice, wants444, dir).is_some_and(|(_, elsewhere, _, _)| elsewhere);
    if restart {
        eprintln!("nightjar: the Graphics card setting changed; Nightjar closes and starts again on that GPU");
        RESTART.store(2, Ordering::Relaxed);
    }
    restart
}

/// Whether Nightjar is to start again on another GPU once it has closed, and then whether with
/// this start's command line.
pub fn restart_requested() -> Option<bool> {
    match RESTART.load(Ordering::Relaxed) { 1 => Some(true), 2 => Some(false), _ => None }
}

/// Starts Nightjar again with `args` and [`RESTARTED`].
pub fn restart(args: &[OsString]) -> Result<(), String> {
    let program = std::env::current_exe().map_err(|e| e.to_string())?;
    crate::amd_decode::for_child(&mut std::process::Command::new(program))
        .args(args)
        .arg(RESTARTED)
        .spawn()
        .map(drop)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    /// The profiles are the ones FFmpeg's D3D11VA asks for, as its dxva2.c writes them.
    #[test]
    fn hevc_444_profiles_are_ffmpegs() {
        assert_eq!(format!("{:?}", super::HEVC_MAIN_444).to_ascii_lowercase(), "4008018f-f537-4b36-98cf-61af8a2c1a33");
        assert_eq!(format!("{:?}", super::HEVC_MAIN10_444).to_ascii_lowercase(), "0dabeffa-4458-4602-bc03-0795659d617c");
    }

    /// The check runs on this machine's GPUs without failing, and never lists the renderer's own.
    #[test]
    fn other_gpus_are_checked_without_the_renderers() {
        use windows::core::Interface;
        use windows::Win32::Foundation::HMODULE;
        use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
        use windows::Win32::Graphics::Direct3D11::*;
        let mut device = None;
        // SAFETY: A device of the test's own.
        let made = unsafe {
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_VIDEO_SUPPORT, None,
                D3D11_SDK_VERSION, Some(&mut device), None, None)
        };
        let Some(device) = device.filter(|_| made.is_ok()) else {
            eprintln!("no hardware Direct3D 11 device here");
            return;
        };
        let raw = device.as_raw();
        let name = super::renderer_gpu(raw).expect("the device's GPU");
        assert!(!name.is_empty());
        for ten_bit in [false, true] {
            let (renderer, others) = super::others_decoding_hevc444(raw, ten_bit).expect("the renderer's GPU");
            assert_eq!(renderer, name);
            eprintln!("renderer {renderer}; others decoding HEVC 4:4:4 (10-bit {ten_bit}): {others:?}");
        }
    }

    #[test]
    fn the_preference_keeps_windows_other_pairs() {
        use super::with_preference;
        assert_eq!(with_preference(None, Some(2)).as_deref(), Some("GpuPreference=2;"));
        assert_eq!(with_preference(Some("SwapEffectUpgradeEnable=1;GpuPreference=1;"), Some(2)).as_deref(),
            Some("SwapEffectUpgradeEnable=1;GpuPreference=2;"));
        assert_eq!(with_preference(Some("GpuPreference=2;AutoHDREnable=1;"), None).as_deref(), Some("AutoHDREnable=1;"));
        assert_eq!(with_preference(Some("GpuPreference=2;"), None), None, "nothing left: the value goes");
    }

    #[test]
    fn automatic_moves_to_the_performance_gpu_only_for_4_4_4_it_alone_decodes() {
        use super::{wanted, GraphicsCard::*};
        assert_eq!(wanted(Automatic, true, || false, || true), Some(2));
        assert_eq!(wanted(Automatic, true, || true, || true), None, "the power-saving GPU decodes it");
        assert_eq!(wanted(Automatic, true, || false, || false), None, "neither does");
        assert_eq!(wanted(Automatic, false, || panic!("not asked"), || panic!("not asked")), None);
        assert_eq!(wanted(PowerSaving, true, || false, || true), Some(1));
        assert_eq!(wanted(HighPerformance, false, || true, || true), Some(2));
    }

    /// Automatic takes back only the preference Nightjar wrote; one the player set in Windows stays.
    #[test]
    fn automatic_takes_back_only_its_own_preference() {
        use super::next_value;
        assert_eq!(next_value(None, None, Some(2)), Some(Some("GpuPreference=2;".into())));
        assert_eq!(next_value(Some("GpuPreference=2;"), None, Some(2)), None, "already so");
        assert_eq!(next_value(Some("GpuPreference=2;"), Some("GpuPreference=2;"), None), Some(None), "ours goes");
        assert_eq!(next_value(Some("GpuPreference=1;"), Some("GpuPreference=2;"), None), None, "the player's stays");
        assert_eq!(next_value(Some("GpuPreference=1;"), None, None), None);
        assert_eq!(next_value(None, None, None), None);
    }

    /// The preference round-trips through Windows' key under a made-up program path, then goes.
    #[test]
    fn the_preference_is_written_and_taken_back_in_the_registry() {
        let dir = std::env::temp_dir().join(format!("nightjar-gpu-pref-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("not-a-program.exe");
        let key = windows::core::HSTRING::from(program.as_os_str());
        assert!(super::set_preference(&program, Some(2), &dir).unwrap());
        assert_eq!(super::read_preference(&key).unwrap().as_deref(), Some("GpuPreference=2;"));
        assert!(!super::set_preference(&program, Some(2), &dir).unwrap(), "already so");
        assert!(super::set_preference(&program, None, &dir).unwrap(), "Automatic takes its own back");
        assert_eq!(super::read_preference(&key).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restart_is_marked_and_taken_out_of_the_command_line() {
        let (args, restarted) = super::split_restarted(vec!["--host".into(), super::RESTARTED.into()]);
        assert!(restarted);
        assert_eq!(args, vec![std::ffi::OsString::from("--host")]);
    }

    #[test]
    fn the_hint_names_both_gpus() {
        let hint = super::hint("AMD Radeon 780M Graphics", "NVIDIA GeForce RTX 5060 Laptop GPU");
        assert!(hint.contains("AMD Radeon 780M Graphics") && hint.contains("NVIDIA GeForce RTX 5060 Laptop GPU"), "{hint}");
    }
}
