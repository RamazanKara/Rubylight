//! Steam Deck and other Linux handhelds in Game Mode, where gamescope is the compositor: Nightjar
//! starts fullscreen without a border, hides the pointer while a controller is in use, and asks
//! gamescope's swapchain for HDR (scRGB, else HDR10 as PQ) instead of the window's HDR state,
//! which SDL cannot read there. Everything here is pure, so the tests need no gamescope.

use std::ffi::OsStr;
use std::sync::OnceLock;
use crate::config::Renderer;

/// Gamescope runs this session: `XDG_CURRENT_DESKTOP` names it (Game Mode sets
/// `gamescope`; a list is colon-separated) or `GAMESCOPE_WAYLAND_DISPLAY` is set, which gamescope
/// sets for everything it starts, nested or embedded.
pub fn gamescope_in(xdg_current_desktop: Option<&OsStr>, gamescope_wayland_display: Option<&OsStr>) -> bool {
    let desktop = xdg_current_desktop.and_then(OsStr::to_str).unwrap_or("");
    desktop.split(':').any(|name| name.trim().eq_ignore_ascii_case("gamescope"))
        || gamescope_wayland_display.is_some_and(|display| !display.is_empty())
}

/// [`gamescope_in`] for this process's environment, read once.
pub fn gamescope() -> bool {
    static GAMESCOPE: OnceLock<bool> = OnceLock::new();
    *GAMESCOPE.get_or_init(|| {
        gamescope_in(std::env::var_os("XDG_CURRENT_DESKTOP").as_deref(), std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").as_deref())
    })
}

/// The socket gamescope's Vulkan layer (its WSI layer, which offers the HDR swapchains) talks to
/// gamescope over: `GAMESCOPE_WAYLAND_DISPLAY` in `XDG_RUNTIME_DIR`, or the path itself when it
/// is absolute. None when there is no such display to check.
pub fn gamescope_socket(xdg_runtime_dir: Option<&OsStr>, gamescope_wayland_display: Option<&OsStr>) -> Option<std::path::PathBuf> {
    let display = std::path::Path::new(gamescope_wayland_display.filter(|d| !d.is_empty())?);
    if display.is_absolute() {
        return Some(display.to_owned());
    }
    Some(std::path::Path::new(xdg_runtime_dir.filter(|d| !d.is_empty())?).join(display))
}

/// The layer is turned off (`DISABLE_GAMESCOPE_WSI`) when its socket is not there: a Flatpak
/// without `xdg-run/gamescope-0` showed its swapchain then blocks until gamescope ends, where
/// without the layer the output is SDR with its reason.
pub fn gamescope_layer_off(socket: Option<&std::path::Path>, exists: impl Fn(&std::path::Path) -> bool) -> bool {
    socket.is_some_and(|socket| !exists(socket))
}

/// How the launcher's window opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartWindow {
    /// Fullscreen on the desktop's mode (never an exclusive mode change).
    pub fullscreen: bool,
    pub borderless: bool,
    /// The saved place and size from the last launch apply.
    pub restore_place: bool,
    /// Leaving a stream returns the window to windowed.
    pub windowed_after_stream: bool,
}

/// Under gamescope, and straight to the display (KMS), the window fills the screen without a
/// border from the start and stays so after a stream: there is no desktop to return to, and a
/// saved windowed place means nothing (and is not overwritten).
pub fn start_window(fills_screen: bool) -> StartWindow {
    StartWindow { fullscreen: fills_screen, borderless: fills_screen, restore_place: !fills_screen, windowed_after_stream: !fills_screen }
}

/// The window fills the screen with no desktop around it: gamescope, or straight to the display.
pub fn fills_screen() -> bool {
    gamescope() || crate::kms::active()
}

/// The launcher window's size at creation: 1280x800 on a desktop. Straight to the display, the
/// display's current mode (`screen`): SDL's KMSDRM sets the mode closest to the window's windowed
/// size, so a 1280x800 window would take a 4K TV down to 720p for the launcher and the stream.
pub fn start_size(kms: bool, screen: Option<(i32, i32)>) -> (i32, i32) {
    match screen {
        Some((w, h)) if kms && w > 0 && h > 0 => (w, h),
        _ => (1280, 800),
    }
}

/// What an input event is, for the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Controller,
    Mouse,
    /// Keys, touch and the rest leave the pointer as it is.
    Other,
}

/// The pointer outside streams: under gamescope it hides with the first controller input and
/// comes back with the mouse (on a Deck, the trackpads). Elsewhere it is never touched.
#[derive(Debug, Default)]
pub struct Pointer {
    gamescope: bool,
    hidden: bool,
}

impl Pointer {
    pub fn new(gamescope: bool) -> Self {
        Self { gamescope, hidden: false }
    }

    /// Some(visible) when the pointer has to change for `input`.
    pub fn input(&mut self, input: Input) -> Option<bool> {
        match input {
            Input::Controller if self.gamescope && !self.hidden => {
                self.hidden = true;
                Some(false)
            }
            Input::Mouse if self.hidden => {
                self.hidden = false;
                Some(true)
            }
            _ => None,
        }
    }

    /// A stream showed the pointer again on leaving (its cursor's clear): back to shown.
    pub fn shown(&mut self) {
        self.hidden = false;
    }
}

/// The swapchain colour spaces the window's Vulkan surface offers (with gamescope, its WSI layer).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg(any(not(target_os = "macos"), test))]
pub struct Offered {
    /// Extended sRGB linear on RGBA16F.
    pub scrgb: bool,
    /// HDR10 (ST 2084) on RGB10A2.
    pub hdr10: bool,
}

/// What the renderer presents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(target_os = "macos", allow(dead_code, reason = "Linux output negotiation diagnostics"))]
pub enum Output {
    /// Linear light on an fp16 swapchain, SDL's own HDR path.
    Scrgb,
    /// SDL draws linear light and the present encodes it as BT.2020 PQ (the patched SDL's pass).
    Pq,
}

/// Why the output is SDR, for the log and, under gamescope, the stream's note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(target_os = "macos", allow(dead_code, reason = "Linux output negotiation diagnostics"))]
pub enum SdrReason {
    /// The software renderer was asked for: no swapchain to ask.
    NoGpuRenderer,
    /// The compositor reports no HDR for the window, and this is not gamescope.
    DisplayNotHdr,
    /// Neither scRGB nor HDR10 is offered.
    NotOffered,
    /// HDR10 is offered, but the PQ renderer could not be made.
    PqFailed,
    /// scRGB is offered, but the renderer's swapchain did not take it.
    SwapchainRefused,
    /// Only HDR10 is offered on a desktop and the renderer is not made for an HDR stream: PQ output
    /// is for HDR streams.
    PqWithoutHdrStream,
}

impl SdrReason {
    pub fn says(self) -> &'static str {
        match self {
            Self::NoGpuRenderer => "the GPU renderer is not in use",
            Self::DisplayNotHdr => "the compositor reports no HDR for this window",
            Self::NotOffered => "the swapchain offers neither scRGB nor HDR10",
            Self::PqFailed => "HDR10 is offered but the PQ renderer could not be made",
            Self::SwapchainRefused => "scRGB is offered but the swapchain did not take it",
            Self::PqWithoutHdrStream => "only HDR10 is offered, and PQ output is used for HDR streams only",
        }
    }
}

/// The output to try. SDL's window HDR state (`window_hdr`) is what a desktop compositor says;
/// gamescope's Xwayland window never has one, so there the swapchain's colour spaces decide.
/// A hidden Wayland window has none yet either: `plan` passes on a display's HDR when a stream asks for it.
/// For an HDR stream (`hdr_stream`: the renderer is made at its start, the host sends HDR10), PQ
/// first, as moonlight-qt's colour space hint asks for the stream's own: the compositor gets
/// BT.2020 PQ, which it can scan out, instead of fp16 it has to convert, and a frame of only the
/// picture is one pass into a 10-bit image (`stream::present::VideoOnly`). Under gamescope too,
/// unless [`scrgb_first`] masks `hdr_stream`: headless gamescope on an Iris Plus 655 drew a 4K
/// picture in 1.94 ms as PQ and 2.16 ms as scRGB (rust/PERFORMANCE.md, November 12; what
/// gamescope then does with either was not measured). Its DRM backend decides that in
/// `CDRMBackend::Present`: with HDR output on and no AMD plane colour properties in the kernel
/// (`AMD_PLANE_CTM`, `AMD_PLANE_BLEND_TF`), only a single HDR10 PQ layer is scanned out and every
/// scRGB frame is composited; with them, either can be scanned out. Otherwise
/// scRGB, as before: the launcher and SDR streams keep drawing straight into the swapchain, which
/// PQ's present pass rules out. A desktop offering HDR10 alone stays SDR without an HDR stream;
/// gamescope offering HDR10 alone presents PQ whatever is drawn.
#[cfg(any(not(target_os = "macos"), test))]
pub fn choose(gpu: bool, gamescope: bool, window_hdr: bool, hdr_stream: bool, offered: Offered) -> Result<Output, SdrReason> {
    if !gpu {
        return Err(SdrReason::NoGpuRenderer);
    }
    if !window_hdr && !gamescope {
        return Err(SdrReason::DisplayNotHdr);
    }
    if gamescope {
        return if offered.hdr10 && hdr_stream {
            Ok(Output::Pq)
        } else if offered.scrgb {
            Ok(Output::Scrgb)
        } else if offered.hdr10 {
            Ok(Output::Pq)
        } else {
            Err(SdrReason::NotOffered)
        };
    }
    if offered.hdr10 && hdr_stream {
        Ok(Output::Pq)
    } else if offered.scrgb {
        Ok(Output::Scrgb)
    } else if offered.hdr10 {
        Err(SdrReason::PqWithoutHdrStream)
    } else {
        Err(SdrReason::NotOffered)
    }
}

/// The output to try when `failed` could not be made: a desktop's PQ falls back to scRGB where it
/// is offered (KDE's scRGB is known to work), never straight to SDR.
#[cfg(any(not(target_os = "macos"), test))]
pub fn fallback(failed: Output, offered: Offered) -> Option<Output> {
    (failed == Output::Pq && offered.scrgb).then_some(Output::Scrgb)
}

/// The renderer presents HDR another way than the HDR10 PQ an HDR stream would get ([`choose`]):
/// HDR10 offered, the last decision was not PQ, PQ has not failed this run (`pq_failed`: its
/// fallback stays), and gamescope is not kept at scRGB ([`scrgb_first`]). A stream asking for HDR
/// makes it again for PQ once ([`renderer_switch`]).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[cfg(any(not(target_os = "macos"), test))]
pub fn pq_unused(scrgb_first: bool, decision: Option<Result<Output, SdrReason>>, hdr10_offered: bool, pq_failed: bool) -> bool {
    !scrgb_first && hdr10_offered && !pq_failed && decision != Some(Ok(Output::Pq))
}

/// Set to 0, gamescope's HDR streams present as scRGB where it is offered, as before they took
/// HDR10 PQ (A/B runs).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const GAMESCOPE_PQ_ENVIRONMENT: &str = "NIGHTJAR_GAMESCOPE_PQ";

/// Gamescope's HDR streams keep scRGB first: only when [`GAMESCOPE_PQ_ENVIRONMENT`] (`value`) is 0.
/// The renderer's makers then hand [`plan`] no HDR stream, and [`pq_unused`] finds nothing to switch.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn scrgb_first(gamescope: bool, value: Option<&OsStr>) -> bool {
    gamescope && value.is_some_and(|value| value == "0")
}

/// [`scrgb_first`] for this process.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn keeps_scrgb_first() -> bool {
    scrgb_first(gamescope(), std::env::var_os(GAMESCOPE_PQ_ENVIRONMENT).as_deref())
}

/// The compositor's window HDR state: SDL's flag, unless Nightjar set it itself for an earlier
/// renderer of the same window (a second attempt after a zero-copy renderer failed).
#[cfg(any(not(target_os = "macos"), test))]
pub fn window_hdr_reported(flag: bool, set_by_nightjar: bool) -> bool {
    flag && !set_by_nightjar
}

/// What `sdl::hdr_output` does with the first renderer it was handed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(any(not(target_os = "macos"), test))]
pub struct Plan {
    pub decision: Result<Output, SdrReason>,
    /// Set the window's HDR state to [`gamescope_hdr_properties`] (the compositor reports none).
    pub set_window_hdr: bool,
    /// Make the renderer again: SDL reads the window's HDR state, and PQ's present, at creation.
    pub rebuild: bool,
}

/// `display_hdr`: a display of the compositor has HDR on. A Wayland window is told its state only
/// once it is mapped, and mapping it takes this first renderer's frame. So where a saved setting
/// asks for HDR (`hdr_asked`) the scRGB renderer stays, and SDL's own report for the window turns
/// its HDR on when it arrives (`SDL_EVENT_WINDOW_HDR_STATE_CHANGED`) or leaves it off on an SDR
/// display. Without an HDR setting the SDR swapchain stays: fp16 would serve no stream. A desktop's
/// PQ, only for an HDR stream's renderer (`hdr_stream`, [`choose`]), is made again with the PQ
/// present, the window's HDR state still the compositor's to report. Gamescope's PQ, for an HDR
/// stream or where HDR10 is all it offers, is made again too, after Nightjar set the window's state.
#[cfg(any(not(target_os = "macos"), test))]
pub fn plan(gamescope: bool, window_hdr: bool, display_hdr: bool, hdr_asked: bool, hdr_stream: bool, offered: Offered) -> Plan {
    let decision = choose(true, gamescope, window_hdr || (display_hdr && (hdr_asked || hdr_stream)), hdr_stream, offered);
    match decision {
        Ok(output) if !gamescope => Plan { decision, set_window_hdr: false, rebuild: output == Output::Pq },
        Ok(output) => Plan { decision, set_window_hdr: !window_hdr, rebuild: output == Output::Pq || !window_hdr },
        Err(_) => Plan { decision, set_window_hdr: false, rebuild: false },
    }
}

/// The output once the renderer is (re)made: SDR when it could not be made or its swapchain did
/// not take HDR.
#[cfg(any(not(target_os = "macos"), test))]
pub fn settle(output: Output, made: bool, hdr_taken: bool) -> Result<Output, SdrReason> {
    match (made && hdr_taken, output) {
        (true, output) => Ok(output),
        (false, Output::Pq) => Err(SdrReason::PqFailed),
        (false, Output::Scrgb) => Err(SdrReason::SwapchainRefused),
    }
}

/// The log line for the renderer's output decision: the renderer, what the swapchain offers, and
/// what was chosen or why it is SDR. `hdr_stream`: made for an HDR stream ([`choose`]).
/// `pq_failed`: PQ was tried first and could not be made, so the output is its [`fallback`].
#[allow(clippy::too_many_arguments)]
#[cfg(any(not(target_os = "macos"), test))]
pub fn describe(renderer: &str, gamescope: bool, window_hdr: bool, display_hdr: bool, hdr_asked: bool, hdr_stream: bool, offered: Offered, decision: Result<Output, SdrReason>, pq_failed: bool) -> String {
    let yes = |b: bool| if b { "yes" } else { "no" };
    let pending = if !gamescope && !window_hdr { ", pending the window's report" } else { "" };
    let fell_back = if pq_failed { ", as HDR10 PQ could not be made" } else { "" };
    let output = match decision {
        Ok(Output::Scrgb) => format!("HDR as scRGB{pending}{fell_back}"),
        Ok(Output::Pq) => format!("HDR as HDR10 PQ (BT.2020, ST 2084){pending}"),
        Err(reason) => format!("SDR, {}", reason.says()),
    };
    format!(
        "renderer {renderer:?}: swapchain offers scRGB {}, HDR10 {}; gamescope {}, window HDR {}, display HDR {}, HDR asked {}, for an HDR stream {}; output {output}",
        yes(offered.scrgb), yes(offered.hdr10), yes(gamescope), yes(window_hdr), yes(display_hdr), yes(hdr_asked), yes(hdr_stream)
    )
}

/// SDR white and the peak gamescope's HDR output is drawn for, in nits, when SDL has no window
/// HDR state to take them from: BT.2408's reference white, and the 1000-nit peak HDR10 masters
/// and the Steam Deck OLED's panel share. Gamescope maps what is above its display's peak.
#[cfg(any(not(target_os = "macos"), test))]
pub const GAMESCOPE_SDR_WHITE_NITS: f32 = 203.0;
#[cfg(any(not(target_os = "macos"), test))]
pub const GAMESCOPE_PEAK_NITS: f32 = 1000.0;

/// SDL's window HDR properties for those: SDR white in scRGB units (80 nits each) and the
/// headroom over it.
#[cfg(any(not(target_os = "macos"), test))]
pub fn gamescope_hdr_properties() -> (f32, f32) {
    (GAMESCOPE_SDR_WHITE_NITS / 80.0, GAMESCOPE_PEAK_NITS / GAMESCOPE_SDR_WHITE_NITS)
}

/// The window property the patched SDL's renderer takes its SDR white from, in place of the
/// window's own level (`vendor/sdl3-src/patches/renderer-sdr-white.patch`): a float, in scRGB units.
#[cfg(any(not(target_os = "macos"), test))]
pub const RENDERER_SDR_WHITE_PROPERTY: &std::ffi::CStr = c"SDL.window.renderer.sdr_white_level";

/// The SDR white SDL's renderer draws its linear output with, in scRGB units (80 nits each),
/// where Nightjar sets it ([`RENDERER_SDR_WHITE_PROPERTY`]); None leaves the window's own level.
/// A desktop's scRGB: 1.0. Mesa describes an scRGB swapchain to a Wayland compositor with the
/// colour management protocol's default luminances, where 1.0 is the reference white, which the
/// compositor shows as its SDR white. SDL 3.4.20 reports the window's SDR white as the
/// compositor's reference luminance over 80 nits (2.5 at 203 nits) and its renderer multiplies
/// all it draws by that: the launcher and SDR streams came out that many times too bright.
/// PQ's present encodes 1.0 as 80 nits, absolute, so the window's own level is right there, and
/// with its headroom gives an HDR stream the display's peak. Under gamescope Nightjar sets the
/// window's HDR state itself ([`gamescope_hdr_properties`]).
#[cfg(any(not(target_os = "macos"), test))]
pub fn renderer_sdr_white(gamescope: bool, output_pq: bool) -> Option<f32> {
    (!gamescope && !output_pq).then_some(1.0)
}

/// What the log adds to a window's HDR report: the SDR white and headroom the compositor gave SDL
/// for the window (`white` in scRGB units), and the SDR white its renderer draws with (`drawn`).
pub fn describe_levels(white: f32, headroom: f32, drawn: Option<f32>) -> String {
    let drawn = drawn.map(|drawn| format!(", drawn with SDR white {drawn:.2}")).unwrap_or_default();
    format!("SDR white {:.0} nits, headroom {headroom:.2}{drawn}", white * 80.0)
}

/// An HDR stream asked for where the output is SDR: outside gamescope the launcher refuses it as
/// before (the desktop can turn HDR on); under gamescope the stream plays in SDR with a note,
/// so a controller-only session never ends at a dialog it cannot fix.
pub fn hdr_stream(asked: bool, output_hdr: bool, gamescope: bool) -> HdrStream {
    match (asked, output_hdr) {
        (false, _) => HdrStream::Sdr,
        (true, true) => HdrStream::Hdr,
        (true, false) if gamescope => HdrStream::SdrWithNote,
        (true, false) => HdrStream::Refuse,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HdrStream {
    Hdr,
    Sdr,
    SdrWithNote,
    Refuse,
}

/// The window's HDR report can still turn HDR on: the swapchain took HDR and a display has it on.
/// Whether the renderer's swapchain can take HDR. With an SDL GPU device (Linux's "gpu" renderer)
/// its swapchain decides (`swapchain_hdr`). Without one, Windows' Direct3D 11 renderer follows the
/// window, while Linux's OpenGL renderer never presents HDR: Nightjar picks it at start when no
/// saved setting asks for HDR, and an HDR stream's start switches to the GPU renderer
/// ([`renderer_switch`]); another display would not help.
pub fn renderer_takes_hdr(windows: bool, gpu_device: bool, swapchain_hdr: bool) -> bool {
    if gpu_device { swapchain_hdr } else { windows }
}

pub fn hdr_report_can_come(swapchain_hdr: bool, display_hdr: bool) -> bool {
    swapchain_hdr && display_hdr
}

/// What an HDR stream refused on a desktop can do about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HdrRefusal {
    /// No display has HDR on.
    TurnHdrOn,
    /// A display has HDR on, and the renderer cannot present it: Linux's GPU renderer could not be
    /// made (`RendererSwitch`), or its swapchain did not take HDR when it was made for this stream.
    RendererSdr,
    /// The swapchain is ready and a display has HDR on, and this window has no HDR: another display's.
    MoveWindow,
}

pub fn hdr_refusal(swapchain_hdr: bool, display_hdr: bool) -> HdrRefusal {
    match (swapchain_hdr, display_hdr) {
        (_, false) => HdrRefusal::TurnHdrOn,
        (false, true) => HdrRefusal::RendererSdr,
        (true, true) => HdrRefusal::MoveWindow,
    }
}

/// The Linux renderer as a stream's start finds it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RendererNow {
    /// SDL's "gpu" renderer (else OpenGL, or software).
    pub gpu: bool,
    /// Its swapchain takes HDR.
    pub hdr_swapchain: bool,
    /// It was made for an HDR stream with a display's HDR on: making it again would change nothing.
    pub made_for_hdr: bool,
    /// The GPU renderer could not be made this run: a stream does not try again (each try makes a
    /// working renderer again), it is refused saying why.
    pub gpu_failed: bool,
    /// Its HDR is not the HDR10 PQ a desktop HDR stream would get ([`pq_unused`]).
    pub pq_unused: bool,
    /// PQ failed this run and the swapchain offers no scRGB: making it again for HDR would land on
    /// SDR again, so an HDR stream keeps it.
    pub hdr_unreachable: bool,
}

/// Why a stream's start makes Linux's renderer again on the same window, before its checks.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererSwitch {
    /// From OpenGL to the GPU renderer, which the stream needs for this (`Settings::gpu_need`, or
    /// Vulkan video).
    ToGpu(&'static str),
    /// The GPU renderer again, for HDR: it was made for SDR, before a display had HDR on or a
    /// setting asked for HDR, and an SDR swapchain stays SDR; or made for scRGB on a desktop that
    /// offers the HDR10 PQ an HDR stream gets.
    ForHdr,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl RendererSwitch {
    pub fn reason(self) -> &'static str {
        match self {
            Self::ToGpu(why) => why,
            Self::ForHdr => "HDR",
        }
    }
}

/// Whether a stream's start switches Linux's renderer, and why. Nightjar starts on OpenGL when no
/// saved setting needs the GPU renderer (VAAPI pictures reach it as DMA-BUFs, `dmabuf`), so a
/// setting turned on later switches here instead of needing a restart. `need`: what in the stream's
/// settings needs the GPU renderer; `hdr`: it asks for HDR; `display_hdr`: a display has HDR on;
/// `busy`: a stream or a launch may use the renderer's device. Only ever towards the GPU renderer:
/// an SDR stream after it keeps it. HDR alone with no display's HDR on keeps OpenGL: the stream is
/// refused either way, and the zero-copy path stays for the next SDR stream. The Renderer setting
/// at OpenGL (`choice`) never switches: the stream's check refuses it, naming the setting.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn renderer_switch(choice: Renderer, now: RendererNow, need: Option<&'static str>, hdr: bool, display_hdr: bool, busy: bool) -> Option<RendererSwitch> {
    if busy || choice == Renderer::OpenGl || now.gpu_failed {
        return None;
    }
    if now.gpu && now.hdr_unreachable {
        return None;
    }
    if !now.gpu {
        return need.filter(|&why| why != "HDR" || display_hdr).map(RendererSwitch::ToGpu);
    }
    (hdr && display_hdr && (!now.hdr_swapchain || now.pq_unused) && !now.made_for_hdr).then_some(RendererSwitch::ForHdr)
}

/// Whether HDR can reach a display, for the renderer switch: gamescope presents HDR whatever the
/// displays report, as in [`plan`]; elsewhere a display has HDR on.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn hdr_display(gamescope: bool, any_display_hdr: bool) -> bool {
    gamescope || any_display_hdr
}

/// Whether Linux starts on the GPU renderer: the Renderer setting, or at Auto whether a saved
/// setting or Vulkan video needs it (`needed`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn starts_on_gpu(choice: Renderer, needed: bool) -> bool {
    match choice {
        Renderer::Auto => needed,
        Renderer::OpenGl => false,
        Renderer::Gpu => true,
    }
}

/// The renderer a changed Renderer setting makes at once, with nothing streaming: Some(true) for the
/// GPU renderer, Some(false) for OpenGL, None to keep the one there (`gpu`). Auto keeps either: it
/// moves up when a stream needs it ([`renderer_switch`]), never down.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn renderer_for_choice(choice: Renderer, gpu: bool) -> Option<bool> {
    match choice {
        Renderer::Auto => None,
        Renderer::OpenGl => gpu.then_some(false),
        Renderer::Gpu => (!gpu).then_some(true),
    }
}

/// Whether a stream whose `need` (`Settings::gpu_need`) finds no GPU renderer is refused before
/// its checks. HDR (SDR may do on an SDR display) and PyroWave (its probe says more) refuse in
/// their own checks.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn refused_without_gpu(need: &str) -> bool {
    !matches!(need, "HDR" | "PyroWave")
}

#[cfg(test)]
mod tests;
