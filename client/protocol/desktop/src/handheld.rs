//! Steam Deck and other Linux handhelds in Game Mode, where gamescope is the compositor: Nightjar
//! starts fullscreen without a border, hides the pointer while a controller is in use, and asks
//! gamescope's swapchain for HDR (scRGB, else HDR10 as PQ) instead of the window's HDR state,
//! which SDL cannot read there. Everything here is pure, so the tests need no gamescope.

use std::ffi::OsStr;
use std::sync::OnceLock;

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
pub struct Offered {
    /// Extended sRGB linear on RGBA16F.
    pub scrgb: bool,
    /// HDR10 (ST 2084) on RGB10A2.
    pub hdr10: bool,
}

/// What the renderer presents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// Linear light on an fp16 swapchain, SDL's own HDR path.
    Scrgb,
    /// SDL draws linear light and the present encodes it as BT.2020 PQ (the patched SDL's pass).
    Pq,
}

/// Why the output is SDR, for the log and, under gamescope, the stream's note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

impl SdrReason {
    pub fn says(self) -> &'static str {
        match self {
            Self::NoGpuRenderer => "the GPU renderer is not used",
            Self::DisplayNotHdr => "the compositor reports no HDR for this window",
            Self::NotOffered => "the swapchain offers neither scRGB nor HDR10",
            Self::PqFailed => "HDR10 is offered but the PQ renderer could not be made",
            Self::SwapchainRefused => "scRGB is offered but the swapchain did not take it",
        }
    }
}

/// The output to try. SDL's window HDR state (`window_hdr`) is what a desktop compositor says;
/// gamescope's Xwayland window never has one, so there the swapchain's colour spaces decide.
/// scRGB first (SDL's tone mapping and the overlay work in it unchanged), then PQ.
pub fn choose(gpu: bool, gamescope: bool, window_hdr: bool, offered: Offered) -> Result<Output, SdrReason> {
    if !gpu {
        return Err(SdrReason::NoGpuRenderer);
    }
    if !window_hdr && !gamescope {
        return Err(SdrReason::DisplayNotHdr);
    }
    if offered.scrgb {
        Ok(Output::Scrgb)
    } else if offered.hdr10 {
        Ok(Output::Pq)
    } else {
        Err(SdrReason::NotOffered)
    }
}

/// The log line for the renderer's output decision: the renderer, what the swapchain offers, and
/// what was chosen or why it is SDR.
pub fn describe(renderer: &str, gamescope: bool, window_hdr: bool, offered: Offered, decision: Result<Output, SdrReason>) -> String {
    let yes = |b: bool| if b { "yes" } else { "no" };
    let output = match decision {
        Ok(Output::Scrgb) => "HDR as scRGB".to_owned(),
        Ok(Output::Pq) => "HDR as HDR10 PQ (BT.2020, ST 2084)".to_owned(),
        Err(reason) => format!("SDR, {}", reason.says()),
    };
    format!(
        "renderer {renderer:?}: swapchain offers scRGB {}, HDR10 {}; gamescope {}, window HDR {}; output {output}",
        yes(offered.scrgb), yes(offered.hdr10), yes(gamescope), yes(window_hdr)
    )
}

/// SDR white and the peak gamescope's HDR output is drawn for, in nits, when SDL has no window
/// HDR state to take them from: BT.2408's reference white, and the 1000-nit peak HDR10 masters
/// and the Steam Deck OLED's panel share. Gamescope maps what is above its display's peak.
pub const GAMESCOPE_SDR_WHITE_NITS: f32 = 203.0;
pub const GAMESCOPE_PEAK_NITS: f32 = 1000.0;

/// SDL's window HDR properties for those: SDR white in scRGB units (80 nits each) and the
/// headroom over it.
pub fn gamescope_hdr_properties() -> (f32, f32) {
    (GAMESCOPE_SDR_WHITE_NITS / 80.0, GAMESCOPE_PEAK_NITS / GAMESCOPE_SDR_WHITE_NITS)
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
    fn scrgb_comes_first_then_pq_and_sdr_says_why() {
        let both = Offered { scrgb: true, hdr10: true };
        let pq_only = Offered { scrgb: false, hdr10: true };
        assert_eq!(choose(true, true, false, both), Ok(Output::Scrgb));
        assert_eq!(choose(true, true, false, pq_only), Ok(Output::Pq), "gamescope offering only HDR10");
        assert_eq!(choose(true, false, true, pq_only), Ok(Output::Pq), "a desktop compositor offering only HDR10");
        assert_eq!(choose(true, false, true, both), Ok(Output::Scrgb));
        assert_eq!(choose(true, true, false, Offered::default()), Err(SdrReason::NotOffered), "gamescope without --hdr-enabled");
        assert_eq!(choose(true, false, false, both), Err(SdrReason::DisplayNotHdr), "a desktop with HDR off");
        assert_eq!(choose(false, true, true, both), Err(SdrReason::NoGpuRenderer));
    }

    #[test]
    fn the_log_names_the_renderer_the_offer_and_the_decision() {
        let line = describe("gpu", true, false, Offered { scrgb: false, hdr10: true }, Ok(Output::Pq));
        assert_eq!(line, "renderer \"gpu\": swapchain offers scRGB no, HDR10 yes; gamescope yes, window HDR no; output HDR as HDR10 PQ (BT.2020, ST 2084)");
        let sdr = describe("gpu", true, false, Offered::default(), Err(SdrReason::NotOffered));
        assert!(sdr.ends_with("output SDR, the swapchain offers neither scRGB nor HDR10"), "{sdr}");
    }

    #[test]
    fn gamescope_hdr_is_drawn_for_reference_white_and_a_1000_nit_peak() {
        let (white, headroom) = gamescope_hdr_properties();
        assert!((white - 2.5375).abs() < 1e-4, "{white}");
        assert!((headroom * GAMESCOPE_SDR_WHITE_NITS - 1000.0).abs() < 1e-2, "{headroom}");
        assert!(headroom > 1.0, "SDL's HDR is on only above 1");
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
