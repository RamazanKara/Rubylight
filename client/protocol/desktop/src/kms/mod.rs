//! Nightjar on Linux without a desktop session: on a console or kiosk box with no Wayland or X11
//! (or when asked to), SDL's KMSDRM video driver opens the display straight through DRM KMS and
//! reads keyboards, mice and controllers from evdev, so the launcher (PCs, games, settings) and
//! a stream started from the command line work as on a desktop, fullscreen. During a stream the
//! decoded VA-API pictures go straight to a display plane (`stream::kms_scanout`), with the
//! overlay and menu on a plane above them, HDR metadata, VRR, content type Game and async flips.
//!
//! This module decides whether that mode is on (`decide`), from the "Display output" setting, the
//! `--kms` / `--no-kms` flags and the session; the decision goes to the log with its reason.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, reason = "DRM KMS is Linux-only; the decisions are tested everywhere"))]

pub mod plan;
#[cfg(target_os = "linux")]
pub mod drm;

use std::ffi::OsString;

use crate::config::DisplayOutput;

/// Forces KMS for this start, whatever the setting says.
pub const KMS_ARG: &str = "--kms";
/// Forces the desktop's window for this start.
pub const NO_KMS_ARG: &str = "--no-kms";

/// Takes `--kms` and `--no-kms` out of the arguments: Some(true) for KMS, Some(false) for the
/// desktop, None for neither; the last one given wins.
pub fn split_flag(args: Vec<OsString>) -> (Vec<OsString>, Option<bool>) {
    let mut flag = None;
    let rest = args
        .into_iter()
        .filter(|arg| {
            let found = if arg == KMS_ARG { Some(true) } else if arg == NO_KMS_ARG { Some(false) } else { None };
            if found.is_some() { flag = found; }
            found.is_none()
        })
        .collect();
    (rest, flag)
}

/// The desktop sessions SDL could show a window in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// `WAYLAND_DISPLAY`, or the default `wayland-0` socket in `XDG_RUNTIME_DIR`.
    pub wayland: bool,
    /// `DISPLAY`.
    pub x11: bool,
}

impl Session {
    /// Read through `var` and `exists` (the environment and the filesystem; tests give their own).
    pub fn read(var: impl Fn(&str) -> Option<OsString>, exists: impl Fn(&std::path::Path) -> bool) -> Self {
        let set = |name: &str| var(name).is_some_and(|value| !value.is_empty());
        let wayland = set("WAYLAND_DISPLAY")
            || var("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()).is_some_and(|dir| exists(&std::path::Path::new(&dir).join("wayland-0")));
        Self { wayland, x11: set("DISPLAY") }
    }

    pub fn any(self) -> bool {
        self.wayland || self.x11
    }
}

/// Where Nightjar shows itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// A window on the desktop (Wayland or X11), as always.
    Desktop,
    /// SDL's KMSDRM video driver, straight on the display.
    Kms,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub backend: Backend,
    /// The log line's reason.
    pub why: String,
    /// The SDL video drivers to ask for, None to leave the environment's `SDL_VIDEO_DRIVER` to
    /// SDL. The desktop pins Wayland and X11: SDL's own order would fall through to KMSDRM.
    pub driver_hint: Option<&'static str>,
}

/// The backend for this start. `SDL_VIDEO_DRIVER` in the environment is SDL's to follow and wins
/// over everything; then the flags, then the setting; Automatic draws through KMS only where no
/// desktop session is there to show a window in.
pub fn decide(flag: Option<bool>, setting: DisplayOutput, session: Session, sdl_video_driver: Option<&str>) -> Decision {
    if let Some(driver) = sdl_video_driver.filter(|d| !d.is_empty()) {
        let kms = driver.split(',').next().is_some_and(|first| first.trim() == "kmsdrm");
        let ignored = match flag {
            Some(asked) if asked != kms => format!("; {} is ignored", if asked { KMS_ARG } else { NO_KMS_ARG }),
            _ => String::new(),
        };
        return Decision { backend: if kms { Backend::Kms } else { Backend::Desktop }, why: format!("SDL_VIDEO_DRIVER is {driver}{ignored}"), driver_hint: None };
    }
    let (kms, why) = match (flag, setting) {
        (Some(true), _) => (true, format!("{KMS_ARG} was given")),
        (Some(false), _) => (false, format!("{NO_KMS_ARG} was given")),
        (None, DisplayOutput::Kms) => (true, "Display output is set to straight to the display".to_owned()),
        (None, DisplayOutput::Desktop) => (false, "Display output is set to the desktop".to_owned()),
        (None, DisplayOutput::Automatic) if session.any() => (false, format!("a {} session is there", if session.wayland { "Wayland" } else { "X11" })),
        (None, DisplayOutput::Automatic) => (true, "no Wayland or X11 session is there".to_owned()),
    };
    Decision { backend: if kms { Backend::Kms } else { Backend::Desktop }, why, driver_hint: Some(if kms { KMS_DRIVER } else { DESKTOP_DRIVERS }) }
}

/// SDL's video driver for straight to the display.
pub const KMS_DRIVER: &str = "kmsdrm";
/// SDL's video drivers for the desktop, in SDL's own order: never KMSDRM behind the desktop's back.
pub const DESKTOP_DRIVERS: &str = "wayland,x11";

/// The renderer KMS mode draws with: OpenGL (EGL on GBM), on which SDL keeps DRM master on its
/// own descriptor, so a stream's pictures can go on display planes; the GPU renderer's Vulkan
/// display surface takes the display for itself, so with it every stream is composited. The
/// Renderer setting at Automatic takes OpenGL with video planes on; a renderer chosen by hand is
/// kept, and then (GPU) the log says the planes are off.
pub fn renderer_for(choice: crate::config::Renderer, video_planes: bool) -> Option<bool> {
    use crate::config::Renderer;
    match choice {
        Renderer::Auto if video_planes => Some(false),
        _ => None,
    }
}

/// What a failed start through the desktop says when no Wayland or X11 session is there: the
/// desktop was asked for (flag or setting), so SDL does not fall back to the display on its own.
pub fn desktop_failure(error: &str) -> String {
    format!(
        "Nightjar found no Wayland or X11 session to show its window in ({error}). Start it in a desktop session, or choose Straight to the display (Settings, Display output, or {KMS_ARG}) to draw on the display from a text console."
    )
}

/// What a failed start in KMS mode says: SDL's error and what it usually means. `connected` is
/// how many connectors of the card have a display, None when the cards could not be read.
pub fn start_failure(error: &str, connected: Option<usize>) -> String {
    if connected == Some(0) {
        return format!(
            "Nightjar could not open the display through KMS ({error}): no connected display was found on the graphics card. Connect a display and turn it on, or choose another card with SDL_KMSDRM_DEVICE_INDEX."
        );
    }
    format!(
        "Nightjar could not open the display through KMS ({error}). It needs read and write access to /dev/dri/card* and /dev/input/event* \
         (the video and input groups, or a seat from logind), and no other program showing on that display. Run it from a text console, or pass {NO_KMS_ARG} in a desktop session."
    )
}

/// The process's own decision, made once before SDL starts (`set`), for the rest of the run.
static BACKEND: std::sync::OnceLock<Backend> = std::sync::OnceLock::new();

pub fn set(backend: Backend) {
    let _ = BACKEND.set(backend);
}

/// `--kms` or `--no-kms` from the command line (`split_flag`), kept for `run`'s decision.
static FLAG: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();

pub fn set_flag(flag: Option<bool>) {
    let _ = FLAG.set(flag);
}

pub fn flag() -> Option<bool> {
    FLAG.get().copied().flatten()
}

/// Nightjar draws through KMS this run.
pub fn active() -> bool {
    BACKEND.get() == Some(&Backend::Kms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(wayland: bool, x11: bool) -> Session {
        Session { wayland, x11 }
    }

    #[test]
    fn the_flags_come_out_of_the_arguments_and_the_last_one_wins() {
        let args = |a: &[&str]| a.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(split_flag(args(&["--host", "pc"])), (args(&["--host", "pc"]), None));
        assert_eq!(split_flag(args(&["--kms", "--host", "pc"])), (args(&["--host", "pc"]), Some(true)));
        assert_eq!(split_flag(args(&["--kms", "--no-kms"])), (Vec::new(), Some(false)));
    }

    #[test]
    fn a_session_is_wayland_display_the_default_socket_or_display() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |name: &str| pairs.iter().find(|(n, _)| *n == name).map(|(_, v)| OsString::from(v));
        let none = |_: &std::path::Path| false;
        assert_eq!(Session::read(env(&[]), none), session(false, false));
        assert_eq!(Session::read(env(&[("WAYLAND_DISPLAY", "wayland-1")]), none), session(true, false));
        assert_eq!(Session::read(env(&[("DISPLAY", ":0")]), none), session(false, true));
        assert_eq!(Session::read(env(&[("DISPLAY", "")]), none), session(false, false), "an empty variable is none");
        let socket = |path: &std::path::Path| path == std::path::Path::new("/run/user/1000/wayland-0");
        assert_eq!(Session::read(env(&[("XDG_RUNTIME_DIR", "/run/user/1000")]), socket), session(true, false), "SDL connects to wayland-0 by default");
        assert_eq!(Session::read(env(&[("XDG_RUNTIME_DIR", "/run/user/1000")]), none), session(false, false), "a console login has the directory without a socket");
    }

    #[test]
    fn automatic_draws_through_kms_only_without_a_session() {
        let d = decide(None, DisplayOutput::Automatic, session(false, false), None);
        assert_eq!((d.backend, d.driver_hint), (Backend::Kms, Some("kmsdrm")));
        assert!(d.why.contains("no Wayland or X11"));
        let d = decide(None, DisplayOutput::Automatic, session(true, true), None);
        assert_eq!((d.backend, d.driver_hint), (Backend::Desktop, Some(DESKTOP_DRIVERS)));
        assert!(d.why.contains("Wayland"));
        assert!(decide(None, DisplayOutput::Automatic, session(false, true), None).why.contains("X11"));
    }

    #[test]
    fn the_setting_and_the_flags_decide_over_the_session() {
        assert_eq!(decide(None, DisplayOutput::Kms, session(true, false), None).backend, Backend::Kms);
        assert_eq!(decide(None, DisplayOutput::Desktop, session(false, false), None).backend, Backend::Desktop);
        assert_eq!(decide(Some(true), DisplayOutput::Desktop, session(true, false), None).backend, Backend::Kms);
        let d = decide(Some(false), DisplayOutput::Kms, session(false, false), None);
        // The desktop pins SDL to Wayland and X11, so SDL cannot fall back to KMSDRM by itself.
        assert_eq!((d.backend, d.driver_hint), (Backend::Desktop, Some("wayland,x11")));
        assert!(d.why.contains("--no-kms"));
        assert_eq!(decide(None, DisplayOutput::Desktop, session(false, false), None).driver_hint, Some(DESKTOP_DRIVERS));
    }

    #[test]
    fn start_failures_name_the_missing_desktop_or_display() {
        let desktop = desktop_failure("No available video device");
        assert!(desktop.contains("no Wayland or X11 session") && desktop.contains("Straight to the display") && desktop.contains(KMS_ARG), "{desktop}");
        let none = start_failure("error getting KMSDRM displays information", Some(0));
        assert!(none.contains("no connected display was found") && !none.contains("read and write access"), "{none}");
        for connected in [Some(1), None] {
            assert!(start_failure("Permission denied", connected).contains("read and write access"));
        }
    }

    #[test]
    fn sdl_video_driver_in_the_environment_wins_and_a_flag_against_it_is_said_to_be_ignored() {
        let d = decide(Some(true), DisplayOutput::Kms, session(false, false), Some("wayland"));
        assert_eq!((d.backend, d.driver_hint), (Backend::Desktop, None));
        assert!(d.why.contains("--kms is ignored"), "{}", d.why);
        let d = decide(None, DisplayOutput::Desktop, session(true, false), Some("kmsdrm,offscreen"));
        assert_eq!((d.backend, d.driver_hint), (Backend::Kms, None), "SDL tries its list in order");
        assert_eq!(decide(None, DisplayOutput::Automatic, session(false, false), Some("")).backend, Backend::Kms, "an empty variable is unset");
    }

    #[test]
    fn automatic_renderer_takes_opengl_for_video_planes_and_a_chosen_renderer_stays() {
        use crate::config::Renderer;
        assert_eq!(renderer_for(Renderer::Auto, true), Some(false));
        assert_eq!(renderer_for(Renderer::Auto, false), None);
        assert_eq!(renderer_for(Renderer::Gpu, true), None);
        assert_eq!(renderer_for(Renderer::OpenGl, true), None);
    }
}
