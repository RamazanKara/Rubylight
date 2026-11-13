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
fn straight_to_the_display_the_launcher_opens_at_the_displays_mode() {
    assert_eq!(start_size(true, Some((3840, 2160))), (3840, 2160));
    assert_eq!(start_size(false, Some((3840, 2160))), (1280, 800), "a desktop window keeps its size");
    assert_eq!(start_size(true, None), (1280, 800), "no mode read: SDL picks");
    assert_eq!(start_size(true, Some((0, 0))), (1280, 800));
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
    let patch = include_str!("../../../vendor/sdl3-src/patches/renderer-sdr-white.patch");
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
