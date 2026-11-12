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

/// Under gamescope the window fills the screen without a border from the start and stays so
/// after a stream: there is no desktop to return to, and a saved windowed place means nothing.
pub fn start_window(gamescope: bool) -> StartWindow {
    StartWindow { fullscreen: gamescope, borderless: gamescope, restore_place: !gamescope, windowed_after_stream: !gamescope }
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
/// gamescope then does with either was not measured). Otherwise
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
mod tests {
    use super::*;

    fn os(s: &str) -> Option<&OsStr> {
        Some(OsStr::new(s))
    }

    #[test]
    fn gamescope_is_found_by_its_desktop_name_or_wayland_display() {
        assert!(gamescope_in(os("gamescope"), None), "Game Mode");
        assert!(gamescope_in(os("KDE:gamescope"), None), "a list");
        assert!(gamescope_in(os("Gamescope"), None));
        assert!(gamescope_in(None, os("gamescope-0")), "nested gamescope keeps the desktop's name");
        assert!(gamescope_in(os("KDE"), os("gamescope-0")));
        assert!(!gamescope_in(os("KDE"), None), "Desktop Mode");
        assert!(!gamescope_in(os("GNOME"), os("")), "an empty variable is no gamescope");
        assert!(!gamescope_in(os("notgamescope"), None));
        assert!(!gamescope_in(None, None));
    }

    #[test]
    fn gamescopes_layer_goes_off_only_when_its_socket_is_missing() {
        let socket = gamescope_socket(os("/run/user/1000"), os("gamescope-0")).unwrap();
        assert_eq!(socket, std::path::Path::new("/run/user/1000").join("gamescope-0"));
        assert_eq!(gamescope_socket(os("/run/user/1000"), os("/tmp/gs/gamescope-1")).unwrap(), std::path::Path::new("/tmp/gs/gamescope-1"));
        assert_eq!(gamescope_socket(os("/run/user/1000"), None), None, "no gamescope display, nothing to check");
        assert_eq!(gamescope_socket(os("/run/user/1000"), os("")), None);
        assert_eq!(gamescope_socket(None, os("gamescope-0")), None);
        assert!(gamescope_layer_off(Some(&socket), |_| false), "a sandbox without the socket");
        assert!(!gamescope_layer_off(Some(&socket), |_| true));
        assert!(!gamescope_layer_off(None, |_| false));
    }

    #[test]
    fn under_gamescope_the_window_is_borderless_fullscreen_and_stays_so() {
        let deck = start_window(true);
        assert!(deck.fullscreen && deck.borderless);
        assert!(!deck.restore_place, "a saved windowed place is not used");
        assert!(!deck.windowed_after_stream, "a stream's end keeps it fullscreen");
        let desktop = start_window(false);
        assert_eq!(desktop, StartWindow { fullscreen: false, borderless: false, restore_place: true, windowed_after_stream: true });
    }

    #[test]
    fn the_pointer_hides_for_a_controller_only_under_gamescope() {
        let mut pointer = Pointer::new(true);
        assert_eq!(pointer.input(Input::Mouse), None, "already shown");
        assert_eq!(pointer.input(Input::Controller), Some(false));
        assert_eq!(pointer.input(Input::Controller), None, "hidden once");
        assert_eq!(pointer.input(Input::Other), None, "keys leave it hidden");
        assert_eq!(pointer.input(Input::Mouse), Some(true), "the trackpad brings it back");
        assert_eq!(pointer.input(Input::Mouse), None);
        pointer.input(Input::Controller);
        pointer.shown();
        assert_eq!(pointer.input(Input::Mouse), None, "a stream's end showed it");
        assert_eq!(pointer.input(Input::Controller), Some(false));

        let mut desktop = Pointer::new(false);
        assert_eq!(desktop.input(Input::Controller), None);
        assert_eq!(desktop.input(Input::Mouse), None);
    }

    #[test]
    fn gamescope_takes_scrgb_first_a_desktop_hdr_stream_pq_and_sdr_says_why() {
        let both = Offered { scrgb: true, hdr10: true };
        let pq_only = Offered { scrgb: false, hdr10: true };
        let scrgb_only = Offered { scrgb: true, hdr10: false };
        assert_eq!(choose(true, true, false, true, both), Ok(Output::Pq), "gamescope's HDR stream presents as HDR10 PQ too");
        assert_eq!(choose(true, true, false, false, both), Ok(Output::Scrgb), "gamescope's launcher and SDR streams keep scRGB");
        assert_eq!(choose(true, true, false, true, scrgb_only), Ok(Output::Scrgb), "gamescope without HDR10: scRGB");
        assert_eq!(choose(true, true, false, false, pq_only), Ok(Output::Pq), "gamescope offering only HDR10");
        assert_eq!(choose(true, false, true, true, both), Ok(Output::Pq), "a desktop HDR stream presents as HDR10 PQ");
        assert_eq!(choose(true, false, true, true, pq_only), Ok(Output::Pq));
        assert_eq!(choose(true, false, true, true, scrgb_only), Ok(Output::Scrgb), "no HDR10: scRGB");
        assert_eq!(choose(true, false, true, false, both), Ok(Output::Scrgb), "SDR settings keep scRGB on an HDR window");
        assert_eq!(choose(true, false, true, false, pq_only), Err(SdrReason::PqWithoutHdrStream), "PQ is for HDR streams");
        assert_eq!(choose(true, true, false, true, Offered::default()), Err(SdrReason::NotOffered), "gamescope without --hdr-enabled");
        assert_eq!(choose(true, false, false, true, both), Err(SdrReason::DisplayNotHdr), "a desktop with HDR off");
        assert_eq!(choose(false, true, true, true, both), Err(SdrReason::NoGpuRenderer));
    }

    #[test]
    fn a_desktops_pq_that_fails_falls_back_to_scrgb_not_sdr() {
        let both = Offered { scrgb: true, hdr10: true };
        assert_eq!(fallback(Output::Pq, both), Some(Output::Scrgb), "KDE's scRGB");
        assert_eq!(fallback(Output::Pq, Offered { scrgb: false, hdr10: true }), None, "nothing else offered: SDR with the reason");
        assert_eq!(fallback(Output::Scrgb, both), None, "scRGB failing is the last HDR try");
        assert_eq!(settle(Output::Pq, true, false), Err(SdrReason::PqFailed), "the first try");
        assert_eq!(settle(fallback(Output::Pq, both).unwrap(), true, true), Ok(Output::Scrgb), "the second");
    }

    #[test]
    fn a_desktop_renderer_without_pq_is_made_again_for_an_hdr_stream_once() {
        assert!(pq_unused(false, Some(Ok(Output::Scrgb)), true, false), "scRGB made for an SDR setting");
        assert!(pq_unused(false, Some(Err(SdrReason::DisplayNotHdr)), true, false));
        assert!(!pq_unused(false, Some(Ok(Output::Pq)), true, false));
        assert!(!pq_unused(false, Some(Ok(Output::Scrgb)), false, false), "no HDR10 offered");
        assert!(!pq_unused(true, Some(Ok(Output::Scrgb)), true, false), "gamescope kept at scRGB (NIGHTJAR_GAMESCOPE_PQ=0)");
        assert!(scrgb_first(true, Some("0".as_ref())) && !scrgb_first(true, None) && !scrgb_first(true, Some("1".as_ref())));
        assert!(!scrgb_first(false, Some("0".as_ref())), "a desktop's PQ has its own switches");
        let scrgb = RendererNow { gpu: true, hdr_swapchain: true, made_for_hdr: false, gpu_failed: false, pq_unused: true, hdr_unreachable: false };
        assert_eq!(renderer_switch(Renderer::Auto, scrgb, Some("HDR"), true, true, false), Some(RendererSwitch::ForHdr));
        assert_eq!(renderer_switch(Renderer::Auto, scrgb, None, false, true, false), None, "an SDR stream keeps scRGB");
        let tried = RendererNow { made_for_hdr: true, ..scrgb };
        assert_eq!(renderer_switch(Renderer::Auto, tried, Some("HDR"), true, true, false), None, "PQ fell back to scRGB once: no second try");
        let sdr_after_pq = RendererNow { hdr_swapchain: false, pq_unused: false, hdr_unreachable: true, ..scrgb };
        assert_eq!(renderer_switch(Renderer::Auto, sdr_after_pq, Some("HDR"), true, true, false), None,
            "PQ failed and no scRGB: making the renderer again would land on SDR again");
    }

    /// A PQ renderer that could not be made fell back to scRGB; a renderer made again later this
    /// run for another reason (the Renderer setting) is scRGB too, and no HDR stream tries PQ again.
    #[test]
    fn pq_that_failed_once_is_not_tried_again_this_run() {
        assert!(!pq_unused(false, Some(Ok(Output::Scrgb)), true, true), "the fallback stays");
        assert!(!pq_unused(false, Some(Err(SdrReason::PqFailed)), true, true));
        let fallback = RendererNow { gpu: true, hdr_swapchain: true, made_for_hdr: false, gpu_failed: false, pq_unused: pq_unused(false, Some(Ok(Output::Scrgb)), true, true), hdr_unreachable: false };
        assert_eq!(renderer_switch(Renderer::Auto, fallback, Some("HDR"), true, true, false), None, "made again later, scRGB still serves HDR");
        let sdr = RendererNow { hdr_swapchain: false, ..fallback };
        assert_eq!(renderer_switch(Renderer::Auto, sdr, Some("HDR"), true, true, false), Some(RendererSwitch::ForHdr), "an SDR swapchain still needs HDR");
    }

    /// A saved HDR setting no longer makes the launcher and SDR streams present through PQ's pass:
    /// PQ is made only for an HDR stream's renderer.
    #[test]
    fn pq_is_made_only_for_an_hdr_streams_renderer() {
        let both = Offered { scrgb: true, hdr10: true };
        let pq_only = Offered { scrgb: false, hdr10: true };
        assert_eq!(plan(false, true, true, true, false, both), Plan { decision: Ok(Output::Scrgb), set_window_hdr: false, rebuild: false }, "start with a saved HDR setting");
        assert_eq!(plan(false, true, true, true, true, both), Plan { decision: Ok(Output::Pq), set_window_hdr: false, rebuild: true }, "the HDR stream's switch");
        assert_eq!(plan(false, true, true, true, false, pq_only).decision, Err(SdrReason::PqWithoutHdrStream), "HDR10 alone waits for the HDR stream");
        assert_eq!(plan(false, true, true, true, true, pq_only).decision, Ok(Output::Pq));
        assert_eq!(plan(true, false, false, false, true, pq_only).decision, Ok(Output::Pq), "gamescope as before");
    }

    #[test]
    fn a_second_renderer_does_not_take_nightjars_own_hdr_state_for_the_compositors() {
        assert!(window_hdr_reported(true, false), "a desktop compositor's HDR");
        assert!(!window_hdr_reported(true, true), "set by the first renderer under gamescope");
        assert!(!window_hdr_reported(false, false));
        let both = Offered { scrgb: true, hdr10: true };
        // The second attempt under gamescope plans as the first did: set the state and rebuild.
        let second = plan(true, window_hdr_reported(true, true), false, true, false, both);
        assert_eq!(second, Plan { decision: Ok(Output::Scrgb), set_window_hdr: true, rebuild: true });
    }

    #[test]
    fn the_renderer_is_made_again_for_pq_or_a_window_hdr_state_nightjar_sets() {
        let both = Offered { scrgb: true, hdr10: true };
        let pq_only = Offered { scrgb: false, hdr10: true };
        assert_eq!(plan(true, false, false, true, false, both), Plan { decision: Ok(Output::Scrgb), set_window_hdr: true, rebuild: true }, "gamescope scRGB");
        assert_eq!(plan(true, false, false, true, false, pq_only), Plan { decision: Ok(Output::Pq), set_window_hdr: true, rebuild: true }, "gamescope PQ");
        assert_eq!(plan(true, false, false, true, true, both), Plan { decision: Ok(Output::Pq), set_window_hdr: true, rebuild: true }, "gamescope's HDR stream: PQ, the window's state set first");
        assert_eq!(plan(false, true, false, false, false, both), Plan { decision: Ok(Output::Scrgb), set_window_hdr: false, rebuild: false }, "a desktop's HDR keeps the renderer");
        assert_eq!(plan(false, true, false, true, true, both), Plan { decision: Ok(Output::Pq), set_window_hdr: false, rebuild: true }, "a desktop HDR stream's PQ");
        assert_eq!(plan(false, true, false, true, false, both), Plan { decision: Ok(Output::Scrgb), set_window_hdr: false, rebuild: false }, "an HDR setting alone keeps scRGB");
        assert_eq!(plan(true, false, false, false, false, Offered::default()), Plan { decision: Err(SdrReason::NotOffered), set_window_hdr: false, rebuild: false });
        assert_eq!(plan(false, true, false, false, false, pq_only).decision, Err(SdrReason::PqWithoutHdrStream));
    }

    // nightjar#80: KDE on Bazzite offers both colour spaces, but a hidden Wayland window has no HDR
    // state yet when the renderer is made, so HDR was refused until the end of the program.
    #[test]
    fn a_wayland_display_with_hdr_keeps_the_scrgb_renderer_before_the_window_is_told() {
        let both = Offered { scrgb: true, hdr10: true };
        let pq_only = Offered { scrgb: false, hdr10: true };
        let scrgb_only = Offered { scrgb: true, hdr10: false };
        assert_eq!(plan(false, false, true, true, false, scrgb_only), Plan { decision: Ok(Output::Scrgb), set_window_hdr: false, rebuild: false }, "SDL's own report sets the state");
        assert_eq!(plan(false, false, true, true, false, both), Plan { decision: Ok(Output::Scrgb), set_window_hdr: false, rebuild: false }, "scRGB at start, as before PQ");
        assert_eq!(plan(false, false, true, true, true, both), Plan { decision: Ok(Output::Pq), set_window_hdr: false, rebuild: true }, "PQ follows SDL's report as scRGB does");
        assert_eq!(plan(false, false, false, true, false, both).decision, Err(SdrReason::DisplayNotHdr), "no display with HDR stays on the SDR swapchain");
        assert_eq!(plan(false, false, true, true, true, pq_only).decision, Ok(Output::Pq));
        assert_eq!(plan(true, false, true, true, false, both), Plan { decision: Ok(Output::Scrgb), set_window_hdr: true, rebuild: true }, "gamescope as before");
    }

    #[test]
    fn sdr_settings_keep_the_sdr_swapchain_while_another_display_has_hdr() {
        let both = Offered { scrgb: true, hdr10: true };
        assert_eq!(plan(false, false, true, false, false, both).decision, Err(SdrReason::DisplayNotHdr), "no added fp16 for SDR streams");
        assert_eq!(plan(false, true, true, false, false, both).decision, Ok(Output::Scrgb), "a window the compositor calls HDR, as before");
        assert_eq!(plan(true, false, true, false, false, both).decision, Ok(Output::Scrgb), "gamescope, as before");
    }

    #[test]
    fn a_refused_hdr_stream_says_what_helps() {
        assert!(hdr_report_can_come(true, true), "worth a short wait");
        assert!(!hdr_report_can_come(false, true), "an SDR swapchain gets no report that helps");
        assert!(!hdr_report_can_come(true, false));
        assert_eq!(hdr_refusal(true, true), HdrRefusal::MoveWindow, "the window is on an SDR display while another has HDR");
        assert_eq!(hdr_refusal(false, true), HdrRefusal::RendererSdr, "the renderer cannot present HDR");
        assert_eq!(hdr_refusal(true, false), HdrRefusal::TurnHdrOn);
        assert_eq!(hdr_refusal(false, false), HdrRefusal::TurnHdrOn);
    }

    /// Nightjar issue 80: HDR switched on after a start without it left KDE on the OpenGL
    /// renderer, and the dialog said to move the window to the HDR display it was already on.
    #[test]
    fn the_opengl_renderer_never_takes_hdr_and_no_other_display_helps() {
        let opengl = renderer_takes_hdr(false, false, false);
        assert!(!opengl);
        assert!(!hdr_report_can_come(opengl, true), "no window report can help OpenGL");
        assert_eq!(hdr_refusal(opengl, true), HdrRefusal::RendererSdr);
        assert!(renderer_takes_hdr(true, false, false), "Direct3D 11 follows the window");
        assert!(renderer_takes_hdr(false, true, true));
        assert!(!renderer_takes_hdr(false, true, false));
    }

    /// Nightjar issue 80, without the restart: HDR turned on after a start on OpenGL switches to
    /// the GPU renderer at the stream's start.
    #[test]
    fn a_stream_that_needs_the_gpu_renderer_switches_to_it_at_its_start() {
        let opengl = RendererNow { gpu: false, hdr_swapchain: false, made_for_hdr: false, gpu_failed: false, pq_unused: false, hdr_unreachable: false };
        assert_eq!(renderer_switch(Renderer::Auto, opengl, Some("HDR"), true, true, false), Some(RendererSwitch::ToGpu("HDR")));
        assert_eq!(renderer_switch(Renderer::Auto, opengl, Some("upscaling"), false, false, false), Some(RendererSwitch::ToGpu("upscaling")), "SDR needs it too");
        assert_eq!(renderer_switch(Renderer::Auto, opengl, Some("PyroWave"), true, false, false), Some(RendererSwitch::ToGpu("PyroWave")));
        assert_eq!(renderer_switch(Renderer::Auto, opengl, Some("HDR"), true, false, false), None, "no display has HDR on: refused either way, OpenGL stays");
        assert_eq!(renderer_switch(Renderer::Auto, opengl, None, false, true, false), None, "an SDR stream keeps OpenGL's zero-copy path");
        assert_eq!(renderer_switch(Renderer::Auto, opengl, Some("HDR"), true, true, true), None, "never under a stream or a launch");
    }

    /// A Steam Deck under gamescope: no display reports HDR, gamescope presents it anyway, so HDR
    /// turned on later switches as `plan` would have started.
    #[test]
    fn hdr_under_gamescope_switches_though_no_display_reports_it() {
        assert!(hdr_display(true, false));
        assert!(hdr_display(false, true));
        assert!(!hdr_display(false, false));
        let opengl = RendererNow { gpu: false, hdr_swapchain: false, made_for_hdr: false, gpu_failed: false, pq_unused: false, hdr_unreachable: false };
        assert_eq!(renderer_switch(Renderer::Auto, opengl, Some("HDR"), true, hdr_display(true, false), false), Some(RendererSwitch::ToGpu("HDR")));
        let sdr = RendererNow { gpu: true, ..opengl };
        assert_eq!(renderer_switch(Renderer::Auto, sdr, Some("HDR"), true, hdr_display(true, false), false), Some(RendererSwitch::ForHdr));
    }

    #[test]
    fn the_gpu_renderer_is_made_again_for_hdr_once_and_never_left() {
        let sdr = RendererNow { gpu: true, hdr_swapchain: false, made_for_hdr: false, gpu_failed: false, pq_unused: false, hdr_unreachable: false };
        assert_eq!(renderer_switch(Renderer::Auto, sdr, Some("HDR"), true, true, false), Some(RendererSwitch::ForHdr), "made for SDR at start");
        assert_eq!(RendererSwitch::ForHdr.reason(), "HDR");
        let tried = RendererNow { made_for_hdr: true, ..sdr };
        assert_eq!(renderer_switch(Renderer::Auto, tried, Some("HDR"), true, true, false), None, "made for HDR already: the refusal says why");
        let hdr = RendererNow { gpu: true, hdr_swapchain: true, made_for_hdr: false, gpu_failed: false, pq_unused: false, hdr_unreachable: false };
        assert_eq!(renderer_switch(Renderer::Auto, hdr, Some("HDR"), true, true, false), None);
        assert_eq!(renderer_switch(Renderer::Auto, sdr, Some("HDR"), true, false, false), None, "no display has HDR on");
        assert_eq!(renderer_switch(Renderer::Auto, sdr, None, false, true, false), None, "an SDR stream never goes back to OpenGL");
    }

    #[test]
    fn the_renderer_setting_decides_the_start_the_switch_and_a_change() {
        let opengl = RendererNow { gpu: false, hdr_swapchain: false, made_for_hdr: false, gpu_failed: false, pq_unused: false, hdr_unreachable: false };
        let gpu = RendererNow { gpu: true, ..opengl };
        assert!(!starts_on_gpu(Renderer::Auto, false), "OpenGL's zero-copy path for SDR");
        assert!(starts_on_gpu(Renderer::Auto, true));
        assert!(!starts_on_gpu(Renderer::OpenGl, true), "the setting wins over what needs the GPU");
        assert!(starts_on_gpu(Renderer::Gpu, false));
        assert_eq!(renderer_switch(Renderer::OpenGl, opengl, Some("HDR"), true, true, false), None, "OpenGL stays: the stream is refused");
        assert_eq!(renderer_switch(Renderer::OpenGl, gpu, Some("HDR"), true, true, false), None);
        assert_eq!(renderer_switch(Renderer::Gpu, opengl, Some("upscaling"), false, false, false), Some(RendererSwitch::ToGpu("upscaling")));
        let failed = RendererNow { gpu_failed: true, ..opengl };
        for choice in [Renderer::Auto, Renderer::Gpu] {
            assert_eq!(renderer_switch(choice, failed, Some("upscaling"), false, false, false), None, "a GPU renderer that failed this run is not tried for each stream");
            assert_eq!(renderer_switch(choice, RendererNow { gpu: true, ..failed }, Some("HDR"), true, true, false), None, "nor made again for HDR");
        }
        assert_eq!(renderer_switch(Renderer::Gpu, gpu, Some("HDR"), true, true, false), Some(RendererSwitch::ForHdr));
        for need in ["upscaling", "4:4:4", "10-bit SDR", "packed full chroma"] {
            assert!(refused_without_gpu(need), "{need} on OpenGL is refused, not drawn without it");
        }
        assert!(!refused_without_gpu("HDR") && !refused_without_gpu("PyroWave"), "they refuse in their own checks");
        assert_eq!(renderer_for_choice(Renderer::Gpu, false), Some(true));
        assert_eq!(renderer_for_choice(Renderer::Gpu, true), None);
        assert_eq!(renderer_for_choice(Renderer::OpenGl, true), Some(false), "both ways when the user picks it");
        assert_eq!(renderer_for_choice(Renderer::OpenGl, false), None);
        assert_eq!(renderer_for_choice(Renderer::Auto, true), None, "Auto never moves down");
        assert_eq!(renderer_for_choice(Renderer::Auto, false), None, "and up only for a stream");
    }

    #[test]
    fn a_renderer_that_fails_or_refuses_hdr_falls_back_to_sdr_and_says_which() {
        assert_eq!(settle(Output::Pq, true, true), Ok(Output::Pq));
        assert_eq!(settle(Output::Scrgb, true, true), Ok(Output::Scrgb));
        assert_eq!(settle(Output::Pq, false, false), Err(SdrReason::PqFailed));
        assert_eq!(settle(Output::Pq, true, false), Err(SdrReason::PqFailed));
        assert_eq!(settle(Output::Scrgb, true, false), Err(SdrReason::SwapchainRefused));
        assert_eq!(settle(Output::Scrgb, false, false), Err(SdrReason::SwapchainRefused));
    }

    #[test]
    fn the_log_names_the_renderer_the_offer_and_the_decision() {
        let both = Offered { scrgb: true, hdr10: true };
        let line = describe("gpu", true, false, false, true, false, Offered { scrgb: false, hdr10: true }, Ok(Output::Pq), false);
        assert_eq!(line, "renderer \"gpu\": swapchain offers scRGB no, HDR10 yes; gamescope yes, window HDR no, display HDR no, HDR asked yes, for an HDR stream no; output HDR as HDR10 PQ (BT.2020, ST 2084)");
        let sdr = describe("gpu", true, false, false, false, false, Offered::default(), Err(SdrReason::NotOffered), false);
        assert!(sdr.ends_with("output SDR, the swapchain offers neither scRGB nor HDR10"), "{sdr}");
        let pending = describe("gpu", false, false, true, false, false, both, Ok(Output::Scrgb), false);
        assert!(pending.ends_with("display HDR yes, HDR asked no, for an HDR stream no; output HDR as scRGB, pending the window's report"), "{pending}");
        let reported = describe("gpu", false, true, true, false, false, both, Ok(Output::Scrgb), false);
        assert!(reported.ends_with("output HDR as scRGB"), "{reported}");
        let setting = describe("gpu", false, true, true, true, false, both, Ok(Output::Scrgb), false);
        assert!(setting.ends_with("HDR asked yes, for an HDR stream no; output HDR as scRGB"), "a saved HDR setting at start: {setting}");
        let desktop_pq = describe("gpu", false, true, true, true, true, both, Ok(Output::Pq), false);
        assert!(desktop_pq.ends_with("gamescope no, window HDR yes, display HDR yes, HDR asked yes, for an HDR stream yes; output HDR as HDR10 PQ (BT.2020, ST 2084)"), "{desktop_pq}");
        let pq_pending = describe("gpu", false, false, true, true, true, both, Ok(Output::Pq), false);
        assert!(pq_pending.ends_with("output HDR as HDR10 PQ (BT.2020, ST 2084), pending the window's report"), "{pq_pending}");
        let fell_back = describe("gpu", false, true, true, true, true, both, Ok(Output::Scrgb), true);
        assert!(fell_back.ends_with("output HDR as scRGB, as HDR10 PQ could not be made"), "{fell_back}");
    }

    #[test]
    fn gamescope_hdr_is_drawn_for_reference_white_and_a_1000_nit_peak() {
        let (white, headroom) = gamescope_hdr_properties();
        assert!((white - 2.5375).abs() < 1e-4, "{white}");
        assert!((headroom * GAMESCOPE_SDR_WHITE_NITS - 1000.0).abs() < 1e-2, "{headroom}");
        assert!(headroom > 1.0, "SDL's HDR is on only above 1");
    }

    #[test]
    fn a_desktops_scrgb_is_drawn_with_sdr_white_at_one_whatever_the_compositor_reports() {
        // SDL 3.4.20 reports reference luminance / 80 for a Wayland window (2.5 at 203 nits): on
        // scRGB, where 1.0 already is the compositor's SDR white, the launcher came out that much too bright.
        assert_eq!(renderer_sdr_white(false, false), Some(1.0), "the launcher and SDR streams on a desktop's scRGB");
        assert_eq!(renderer_sdr_white(false, true), None, "PQ's present keeps the window's level: an HDR stream's peak comes from it");
        assert_eq!(renderer_sdr_white(true, false), None, "gamescope's scRGB keeps the HDR state Nightjar sets on the window");
        assert_eq!(renderer_sdr_white(true, true), None);
        // The property SDL's renderer reads is the one the patch adds.
        let patch = include_str!("../../vendor/sdl3-src/patches/renderer-sdr-white.patch");
        let name = RENDERER_SDR_WHITE_PROPERTY.to_str().unwrap();
        assert!(patch.contains(&format!("SDL_GetFloatProperty(window_props, \"{name}\", 0.0f)")), "{name} is not what the patch reads");
        assert!(patch.contains("+++ b/src/render/SDL_render.c"));
    }

    #[test]
    fn a_window_hdr_report_says_the_compositors_levels_and_what_is_drawn() {
        assert_eq!(describe_levels(2.5375, 4.926, Some(1.0)), "SDR white 203 nits, headroom 4.93, drawn with SDR white 1.00");
        assert_eq!(describe_levels(2.5375, 4.926, Some(2.5375)), "SDR white 203 nits, headroom 4.93, drawn with SDR white 2.54");
        assert_eq!(describe_levels(1.0, 1.0, None), "SDR white 80 nits, headroom 1.00");
    }

    #[test]
    fn an_hdr_stream_without_hdr_output_plays_sdr_with_a_note_only_under_gamescope() {
        assert_eq!(hdr_stream(true, true, true), HdrStream::Hdr);
        assert_eq!(hdr_stream(true, false, true), HdrStream::SdrWithNote);
        assert_eq!(hdr_stream(true, false, false), HdrStream::Refuse, "the desktop's dialog stays");
        assert_eq!(hdr_stream(false, false, true), HdrStream::Sdr);
        assert_eq!(hdr_stream(false, true, false), HdrStream::Sdr);
    }
}
