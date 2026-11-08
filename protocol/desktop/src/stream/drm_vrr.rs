//! The display check on Linux, which has no scan line to read: whether the display the stream's
//! window is on runs VRR, read from DRM KMS. The CRTC that scans out the window's output has a
//! `VRR_ENABLED` property the compositor sets (and its connector a `vrr_capable` one). At 0 the
//! display refreshes at its own rate whatever is presented, so a VRR stream that presents at once
//! repeats and skips pictures: the verdict is "no VRR", and the stream takes the pacing it would
//! have without VRR, as on Windows (`present::after_display_check`). At 1 it keeps its pacing.
//!
//! The reads are unprivileged (no DRM master, as `drm_info` does them) and run off the render
//! thread. KDE's "Automatic" adaptive sync turns VRR on only while a fullscreen window is on the
//! output, so a read waits for the window to be fullscreen and focused for `SETTLE` (`Settle`).

use std::time::{Duration, Instant};

use super::scanline::{RaceCapability, Reason, Verdict};

/// How long the window presents fullscreen and focused before `VRR_ENABLED` is read: KDE sets it
/// on the next frame once a fullscreen window is active on the output.
pub const SETTLE: Duration = Duration::from_secs(1);

/// How far the display's refresh and the CRTC's mode may differ and still be the same output.
const REFRESH_TOLERANCE_HZ: f64 = 0.5;
/// How far its size may differ: SDL's size times the pixel density is rounded.
const SIZE_TOLERANCE_PX: u32 = 2;

/// One output a CRTC scans out, as DRM has it.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    /// The card node, `card0`.
    pub card: String,
    /// The connector's kernel name, `DP-1`.
    pub name: String,
    /// The CRTC's mode.
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f64,
    /// The connector's `vrr_capable` and the CRTC's `VRR_ENABLED`, None where it has none.
    pub vrr_capable: Option<u64>,
    pub vrr_enabled: Option<u64>,
}

/// The display SDL has the window on: its name and current mode in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub name: Option<String>,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f64,
}

impl Display {
    /// The mode is known: SDL gave one.
    fn known(&self) -> bool { self.width > 0 && self.height > 0 }

    fn shows(&self, output: &Output) -> bool {
        let near = |a: u32, b: u32| a.abs_diff(b) <= SIZE_TOLERANCE_PX;
        // A rotated output's CRTC mode is the panel's, the desktop's is turned.
        let size = (near(output.width, self.width) && near(output.height, self.height))
            || (near(output.height, self.width) && near(output.width, self.height));
        // SDL gives no refresh for some outputs (Wayland virtual ones, RandR modes without timings).
        size && (self.refresh_hz <= 0.0 || (output.refresh_hz - self.refresh_hz).abs() <= REFRESH_TOLERANCE_HZ)
    }

    /// SDL's name is the connector's (X11 with the modesetting driver, some Wayland compositors)
    /// or a description that ends in it in brackets (`Dell Inc. DELL U2724D (DP-1)`).
    fn named(&self, output: &Output) -> bool {
        self.name.as_deref().is_some_and(|name| name == output.name || name.contains(&format!("({})", output.name)))
    }

    fn describe(&self) -> String {
        format!("{}{}x{} at {:.2} Hz", self.name.as_deref().map_or(String::new(), |name| format!("\"{name}\" ")), self.width, self.height, self.refresh_hz)
    }
}

/// The output that shows `display`: among those of its name (`Display::named`) if any, the one
/// of its mode. A name need not match: the amdgpu X driver says `DisplayPort-0`, and SDL's name
/// can be the monitor's alone. Two GPUs can each have a `DP-1`. The mode is required whenever SDL
/// gave one, even of the only output KMS shows: the window's display may be on a card that is not
/// there (one that failed to read, NVIDIA without modesetting), and a laptop panel's VRR off must
/// not turn a VRR monitor to V-Sync. Without a mode only the name decides. Anything still
/// ambiguous is said, not guessed.
pub fn select<'a>(outputs: &'a [Output], display: &Display) -> Result<&'a Output, String> {
    if outputs.is_empty() { return Err("no connector is driven by a CRTC".into()); }
    let mut candidates: Vec<&Output> = outputs.iter().filter(|output| display.named(output)).collect();
    if candidates.is_empty() && display.known() { candidates = outputs.iter().collect(); }
    if display.known() { candidates.retain(|output| display.shows(output)); }
    match candidates.as_slice() {
        [output] => Ok(output),
        found => {
            let all = outputs.iter().map(|o| format!("{} {} {}x{} at {:.2} Hz", o.card, o.name, o.width, o.height, o.refresh_hz)).collect::<Vec<_>>().join(", ");
            Err(format!("{} of {} active outputs ({all}) could show display {}", found.len(), outputs.len(), display.describe()))
        }
    }
}

/// What a read found.
#[derive(Clone, Debug, PartialEq)]
pub enum Reading {
    /// `enabled`: `VRR_ENABLED` is 1 (`runs` says whether the display does).
    Vrr { enabled: bool, output: String, capable: Option<u64> },
    /// The outputs could not be read, or the window's is not known, or its CRTC says nothing.
    Unknown(String),
}

impl Reading {
    /// The reading of the window's output from the outputs read (`outputs`).
    pub fn of(outputs: Result<Vec<Output>, String>, display: &Display) -> Self {
        let outputs = match outputs {
            Ok(outputs) => outputs,
            Err(why) => return Self::Unknown(why),
        };
        let output = match select(&outputs, display) {
            Ok(output) => output,
            Err(why) => return Self::Unknown(why),
        };
        match output.vrr_enabled {
            Some(value @ (0 | 1)) => Self::Vrr { enabled: value == 1, output: output.name.clone(), capable: output.vrr_capable },
            Some(value) => Self::Unknown(format!("VRR_ENABLED reads {value} on {}", output.name)),
            None => Self::Unknown(format!("the CRTC of {} on {} has no VRR_ENABLED property", output.name, output.card)),
        }
    }

    /// The display refreshes on present: `VRR_ENABLED` 1 on a connector not said to be incapable,
    /// which amdgpu scans at a fixed rate whatever the CRTC says.
    pub fn runs(&self) -> bool { matches!(self, Self::Vrr { enabled: true, capable, .. } if *capable != Some(0)) }

    /// The log line after the topic.
    pub fn line(&self) -> String {
        match self {
            Self::Vrr { enabled, output, capable } => {
                let capable = capable.map_or("unknown".to_string(), |value| value.to_string());
                let said = if self.runs() { "VRR on" } else { "no VRR" };
                format!("{said} (DRM: VRR_ENABLED {} on {output}, vrr_capable {capable})", u8::from(*enabled))
            }
            Self::Unknown(why) => format!("VRR not known, the pacing stays (DRM: {why})"),
        }
    }
}

/// The verdict from the last reading, None while none came. VRR off is the "no VRR" a scan line
/// check gives, which turns a VRR stream to its pacing without VRR; VRR on says the display
/// refreshes on present, which a stream turned to V-Sync returns from; not known is "no scanline",
/// which leaves the pacing as it is.
pub fn verdict(tearing: bool, reading: Option<&Reading>) -> Verdict {
    let mut capability = RaceCapability { tearing, vrr: false, flip_inferred: false, scanline: false };
    if !tearing { return Verdict::Off(Reason::NoTearing, capability); }
    match reading {
        None => Verdict::Checking,
        Some(reading @ Reading::Vrr { .. }) if reading.runs() => {
            capability.vrr = true;
            Verdict::Off(Reason::VrrNoScanline, capability)
        }
        Some(Reading::Vrr { .. }) => Verdict::Off(Reason::NoVrr, capability),
        Some(Reading::Unknown(_)) => Verdict::Off(Reason::NoScanline, capability),
    }
}

/// What the probe does after a present (`Settle::present`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    /// Read `VRR_ENABLED` now.
    Read,
    /// The window is on another display: what was read is about the old one.
    Moved,
}

/// When to read: once the window has been fullscreen and focused for `SETTLE`, again each time it
/// becomes so after it was not (KDE turns VRR on and off with it), and after `restart` (a recheck,
/// or the window moved to another display).
#[derive(Debug, Default)]
pub struct Settle {
    display: Option<u32>,
    since: Option<Instant>,
    asked: bool,
}

impl Settle {
    /// One present. `ready`: the window is fullscreen and has the focus. `display`: SDL's id of
    /// the window's display, 0 when not known.
    pub fn present(&mut self, ready: bool, display: u32, now: Instant) -> Step {
        if display != 0 && self.display.replace(display).is_some_and(|was| was != display) {
            self.restart();
            return Step::Moved;
        }
        if !ready {
            (self.since, self.asked) = (None, false);
            return Step::Wait;
        }
        let since = *self.since.get_or_insert(now);
        if self.asked || now.duration_since(since) < SETTLE { return Step::Wait; }
        self.asked = true;
        Step::Read
    }

    /// Reads again once the window has settled for `SETTLE` from now.
    pub fn restart(&mut self) { (self.since, self.asked) = (None, false); }
}

/// Every output a CRTC scans out, on every card (`/dev/dri/card*`). A card that cannot be read is
/// left out while another can; with none, the error says why.
#[cfg(target_os = "linux")]
pub fn outputs() -> Result<Vec<Output>, String> {
    let mut cards: Vec<String> = std::fs::read_dir("/dev/dri")
        .map_err(|error| format!("/dev/dri cannot be listed: {error}"))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.strip_prefix("card").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
        .collect();
    cards.sort();
    if cards.is_empty() { return Err("no /dev/dri/card* node".into()); }
    let (mut found, mut failed, mut read) = (Vec::new(), Vec::new(), 0);
    for card in cards {
        match sys::read_card(&card) {
            Ok(outputs) => {
                read += 1;
                found.extend(outputs);
            }
            Err(error) => failed.push(format!("{card}: {error}")),
        }
    }
    if read == 0 { return Err(format!("no card could be read ({})", failed.join(", "))); }
    Ok(found)
}

/// DRM is Linux's.
#[cfg(not(target_os = "linux"))]
#[allow(dead_code)] // Only Linux builds this module outside its tests.
pub fn outputs() -> Result<Vec<Output>, String> { Err("DRM KMS is Linux only".into()) }

/// The kernel's name of a connector type (`drm_connector_enum_list`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn connector_type_name(kind: u32) -> &'static str {
    match kind {
        1 => "VGA", 2 => "DVI-I", 3 => "DVI-D", 4 => "DVI-A", 5 => "Composite", 6 => "SVIDEO", 7 => "LVDS", 8 => "Component",
        9 => "DIN", 10 => "DP", 11 => "HDMI-A", 12 => "HDMI-B", 13 => "TV", 14 => "eDP", 15 => "Virtual", 16 => "DSI",
        17 => "DPI", 18 => "Writeback", 19 => "SPI", 20 => "USB", _ => "Unknown",
    }
}

/// A mode's refresh from its timings, as the kernel's `drm_mode_vrefresh` works it out.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn mode_refresh_hz(clock_khz: u32, htotal: u16, vtotal: u16, vscan: u16, flags: u32, vrefresh: u32) -> f64 {
    const INTERLACE: u32 = 1 << 4;
    const DBLSCAN: u32 = 1 << 5;
    if htotal == 0 || vtotal == 0 { return f64::from(vrefresh); }
    let mut hz = f64::from(clock_khz) * 1000.0 / (f64::from(htotal) * f64::from(vtotal));
    if flags & INTERLACE != 0 { hz *= 2.0; }
    if flags & DBLSCAN != 0 { hz /= 2.0; }
    if vscan > 1 { hz /= f64::from(vscan); }
    hz
}

/// The mode ioctls `drm_info` makes as a normal user, raw (`drm_mode.h`), so no libdrm is needed.
#[cfg(target_os = "linux")]
mod sys {
    use std::collections::HashMap;
    use std::os::fd::{AsRawFd, RawFd};
    use std::os::unix::fs::OpenOptionsExt;

    use super::{connector_type_name, mode_refresh_hz, Output};

    #[repr(C)]
    #[derive(Default)]
    struct CardRes {
        fb_id_ptr: u64,
        crtc_id_ptr: u64,
        connector_id_ptr: u64,
        encoder_id_ptr: u64,
        count_fbs: u32,
        count_crtcs: u32,
        count_connectors: u32,
        count_encoders: u32,
        min_width: u32,
        max_width: u32,
        min_height: u32,
        max_height: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct ModeInfo {
        clock: u32,
        hdisplay: u16,
        hsync_start: u16,
        hsync_end: u16,
        htotal: u16,
        hskew: u16,
        vdisplay: u16,
        vsync_start: u16,
        vsync_end: u16,
        vtotal: u16,
        vscan: u16,
        vrefresh: u32,
        flags: u32,
        kind: u32,
        name: [u8; 32],
    }

    #[repr(C)]
    #[derive(Default)]
    struct Crtc {
        set_connectors_ptr: u64,
        count_connectors: u32,
        crtc_id: u32,
        fb_id: u32,
        x: u32,
        y: u32,
        gamma_size: u32,
        mode_valid: u32,
        mode: ModeInfo,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Encoder {
        encoder_id: u32,
        encoder_type: u32,
        crtc_id: u32,
        possible_crtcs: u32,
        possible_clones: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Connector {
        encoders_ptr: u64,
        modes_ptr: u64,
        props_ptr: u64,
        prop_values_ptr: u64,
        count_modes: u32,
        count_props: u32,
        count_encoders: u32,
        encoder_id: u32,
        connector_id: u32,
        connector_type: u32,
        connector_type_id: u32,
        connection: u32,
        mm_width: u32,
        mm_height: u32,
        subpixel: u32,
        pad: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ObjProperties {
        props_ptr: u64,
        prop_values_ptr: u64,
        count_props: u32,
        obj_id: u32,
        obj_type: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Property {
        values_ptr: u64,
        enum_blob_ptr: u64,
        prop_id: u32,
        flags: u32,
        name: [u8; 32],
        count_values: u32,
        count_enum_blobs: u32,
    }

    // The kernel's layouts; a wrong size would be a different ioctl number.
    const _: () = assert!(size_of::<CardRes>() == 64);
    const _: () = assert!(size_of::<ModeInfo>() == 68);
    const _: () = assert!(size_of::<Crtc>() == 104);
    const _: () = assert!(size_of::<Encoder>() == 20);
    const _: () = assert!(size_of::<Connector>() == 80);
    const _: () = assert!(size_of::<ObjProperties>() == 32);
    const _: () = assert!(size_of::<Property>() == 64);

    /// `DRM_IOWR(nr, T)`: read and write, type 'd'.
    const fn iowr<T>(nr: u32) -> u32 { (3 << 30) | ((size_of::<T>() as u32) << 16) | (0x64 << 8) | nr }

    const GETRESOURCES: u32 = iowr::<CardRes>(0xA0);
    const GETCRTC: u32 = iowr::<Crtc>(0xA1);
    const GETENCODER: u32 = iowr::<Encoder>(0xA6);
    const GETCONNECTOR: u32 = iowr::<Connector>(0xA7);
    const GETPROPERTY: u32 = iowr::<Property>(0xAA);
    const OBJ_GETPROPERTIES: u32 = iowr::<ObjProperties>(0xB9);
    /// `DRM_IO(0x1f)`: no data.
    const DROP_MASTER: u32 = (0x64 << 8) | 0x1f;

    const OBJECT_CRTC: u32 = 0xcccc_cccc;
    const OBJECT_CONNECTOR: u32 = 0xc0c0_c0c0;
    const CONNECTED: u32 = 1;

    fn ioctl<T>(fd: RawFd, request: u32, arg: &mut T) -> std::io::Result<()> {
        loop {
            // SAFETY: `arg` is the request's struct, and the pointers in it address buffers that
            // live through the call with the counts it gives.
            if unsafe { libc::ioctl(fd, request as _, arg as *mut T) } == 0 { return Ok(()); }
            let error = std::io::Error::last_os_error();
            if !matches!(error.raw_os_error(), Some(libc::EINTR | libc::EAGAIN)) { return Err(error); }
        }
    }

    fn name_of(raw: &[u8; 32]) -> String {
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        String::from_utf8_lossy(&raw[..end]).into_owned()
    }

    /// An object's properties by name. `names` caches the property names of this card.
    fn properties(fd: RawFd, id: u32, kind: u32, names: &mut HashMap<u32, String>) -> std::io::Result<HashMap<String, u64>> {
        let mut query = ObjProperties { obj_id: id, obj_type: kind, ..Default::default() };
        ioctl(fd, OBJ_GETPROPERTIES, &mut query)?;
        let mut ids = vec![0u32; query.count_props as usize];
        let mut values = vec![0u64; ids.len()];
        query.props_ptr = ids.as_mut_ptr() as u64;
        query.prop_values_ptr = values.as_mut_ptr() as u64;
        ioctl(fd, OBJ_GETPROPERTIES, &mut query)?;
        // The count can only have shrunk into the buffers' length; a grown one copied nothing more.
        let count = (query.count_props as usize).min(ids.len());
        let mut found = HashMap::new();
        for (&prop, &value) in ids[..count].iter().zip(&values[..count]) {
            let name = match names.entry(prop) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let mut property = Property { prop_id: prop, ..Default::default() };
                    if ioctl(fd, GETPROPERTY, &mut property).is_err() { continue; }
                    entry.insert(name_of(&property.name))
                }
            };
            found.insert(name.clone(), value);
        }
        Ok(found)
    }

    pub(super) fn read_card(card: &str) -> std::io::Result<Vec<Output>> {
        let file = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_CLOEXEC).open(format!("/dev/dri/{card}"))?;
        let fd = file.as_raw_fd();
        // Opening a card no one is master of (no compositor on it) makes this the master, which
        // a compositor starting now would be refused. Nothing here needs it: give it up at once.
        // SAFETY: DROP_MASTER takes no argument; refused when not master, which is fine.
        unsafe { libc::ioctl(fd, DROP_MASTER as _, 0) };
        let mut res = CardRes::default();
        ioctl(fd, GETRESOURCES, &mut res)?;
        let mut connectors = vec![0u32; res.count_connectors as usize];
        res = CardRes { connector_id_ptr: connectors.as_mut_ptr() as u64, count_connectors: connectors.len() as u32, ..Default::default() };
        ioctl(fd, GETRESOURCES, &mut res)?;
        connectors.truncate(res.count_connectors as usize);
        let mut names = HashMap::new();
        let mut outputs = Vec::new();
        for id in connectors {
            // One mode's room: with none, a call can make the kernel probe the connector (an EDID
            // read) where it lets the caller, as libdrm's drmModeGetConnectorCurrent avoids.
            let mut stub = ModeInfo::default();
            let mut connector = Connector { connector_id: id, modes_ptr: &mut stub as *mut ModeInfo as u64, count_modes: 1, ..Default::default() };
            if ioctl(fd, GETCONNECTOR, &mut connector).is_err() || connector.connection != CONNECTED || connector.encoder_id == 0 { continue; }
            let mut encoder = Encoder { encoder_id: connector.encoder_id, ..Default::default() };
            if ioctl(fd, GETENCODER, &mut encoder).is_err() || encoder.crtc_id == 0 { continue; }
            let mut crtc = Crtc { crtc_id: encoder.crtc_id, ..Default::default() };
            if ioctl(fd, GETCRTC, &mut crtc).is_err() || crtc.mode_valid == 0 { continue; }
            let mode = crtc.mode;
            let connector_props = properties(fd, id, OBJECT_CONNECTOR, &mut names).unwrap_or_default();
            let crtc_props = properties(fd, crtc.crtc_id, OBJECT_CRTC, &mut names).unwrap_or_default();
            outputs.push(Output {
                card: card.to_string(),
                name: format!("{}-{}", connector_type_name(connector.connector_type), connector.connector_type_id),
                width: u32::from(mode.hdisplay),
                height: u32::from(mode.vdisplay),
                refresh_hz: mode_refresh_hz(mode.clock, mode.htotal, mode.vtotal, mode.vscan, mode.flags, mode.vrefresh),
                vrr_capable: connector_props.get("vrr_capable").copied(),
                vrr_enabled: crtc_props.get("VRR_ENABLED").copied(),
            });
        }
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(card: &str, name: &str, size: (u32, u32), hz: f64, enabled: Option<u64>) -> Output {
        Output { card: card.into(), name: name.into(), width: size.0, height: size.1, refresh_hz: hz, vrr_capable: Some(1), vrr_enabled: enabled }
    }

    fn display(name: Option<&str>, size: (u32, u32), hz: f64) -> Display {
        Display { name: name.map(Into::into), width: size.0, height: size.1, refresh_hz: hz }
    }

    #[test]
    fn the_only_output_kms_shows_must_still_show_the_windows_mode() {
        let outputs = [output("card1", "DP-1", (3440, 1440), 143.97, Some(0))];
        assert_eq!(select(&outputs, &display(Some("DisplayPort-0"), (3440, 1440), 144.0)).unwrap().name, "DP-1", "whatever its name");
        // A G-SYNC monitor on a card KMS does not show (NVIDIA without modesetting) beside the
        // laptop panel with VRR off: not the window's, so the stream keeps its pacing.
        let panel = [output("card1", "eDP-1", (1920, 1200), 60.0, Some(0))];
        let why = select(&panel, &display(Some("DP-0"), (2560, 1440), 165.0)).unwrap_err();
        assert!(why.contains("0 of 1"), "{why}");
        assert!(matches!(Reading::of(Ok(panel.to_vec()), &display(Some("DP-0"), (2560, 1440), 165.0)), Reading::Unknown(_)));
        // Without SDL's mode only the name decides.
        assert!(select(&panel, &display(Some("DP-0"), (0, 0), 0.0)).is_err());
        assert_eq!(select(&panel, &display(Some("eDP-1"), (0, 0), 0.0)).unwrap().name, "eDP-1");
    }

    #[test]
    fn a_wayland_description_names_its_connector_and_a_rounded_size_still_matches() {
        let outputs = [output("card0", "DP-1", (2560, 1440), 144.0, Some(1)), output("card0", "DP-2", (2560, 1440), 144.0, Some(0))];
        let found = select(&outputs, &display(Some("Dell Inc. DELL U2724D (DP-2)"), (2560, 1440), 144.0)).unwrap();
        assert_eq!(found.name, "DP-2");
        // 1707 x 1.5 at a 150% scale, rounded.
        let scaled = [output("card0", "eDP-1", (2560, 1600), 120.0, Some(1))];
        assert_eq!(select(&scaled, &display(None, (2561, 1599), 120.0)).unwrap().name, "eDP-1");
        assert!(select(&scaled, &display(None, (2564, 1600), 120.0)).is_err(), "3 px is another mode");
        // No refresh from SDL: the size alone, and the name still picks.
        assert_eq!(select(&scaled, &display(Some("eDP-1"), (2560, 1600), 0.0)).unwrap().name, "eDP-1");
        assert_eq!(select(&outputs, &display(Some("Dell Inc. DELL U2724D (DP-1)"), (2560, 1440), 0.0)).unwrap().name, "DP-1");
        assert!(select(&outputs, &display(None, (2560, 1440), 0.0)).is_err(), "two of that size are still ambiguous");
    }

    #[test]
    fn the_name_picks_among_outputs_and_the_mode_among_names_on_two_cards() {
        let outputs = [
            output("card0", "DP-1", (2560, 1440), 165.0, Some(1)),
            output("card1", "DP-1", (3440, 1440), 143.97, Some(0)),
            output("card1", "HDMI-A-1", (1920, 1080), 60.0, Some(0)),
        ];
        assert_eq!(select(&outputs, &display(Some("HDMI-A-1"), (1920, 1080), 60.0)).unwrap().name, "HDMI-A-1");
        let found = select(&outputs, &display(Some("DP-1"), (3440, 1440), 144.0)).unwrap();
        assert_eq!((found.card.as_str(), found.name.as_str()), ("card1", "DP-1"), "the mode decides between the two DP-1");
        // The amdgpu X driver's names: the mode alone, rotated too.
        assert_eq!(select(&outputs, &display(Some("DisplayPort-1"), (1440, 2560), 165.2)).unwrap().card, "card0");
    }

    #[test]
    fn an_ambiguous_output_is_not_guessed() {
        let outputs = [output("card0", "DP-1", (2560, 1440), 144.0, Some(1)), output("card0", "DP-2", (2560, 1440), 144.0, Some(0))];
        let why = select(&outputs, &display(Some("Dell U2724D"), (2560, 1440), 144.0)).unwrap_err();
        assert!(why.contains("2 of 2 active outputs") && why.contains("DP-2"), "{why}");
        assert!(select(&outputs, &display(None, (1920, 1080), 60.0)).is_err(), "and none that matches");
        assert!(select(&[], &display(None, (1920, 1080), 60.0)).is_err());
        assert_eq!(Reading::of(Ok(outputs.to_vec()), &display(None, (2560, 1440), 144.0)).line().split(" (").next(), Some("VRR not known, the pacing stays"));
    }

    #[test]
    fn the_readings_and_their_log_lines() {
        let only = |enabled| Ok(vec![output("card1", "DP-1", (3440, 1440), 143.97, enabled)]);
        let shown = display(Some("DP-1"), (3440, 1440), 144.0);
        let off = Reading::of(only(Some(0)), &shown);
        assert_eq!(off.line(), "no VRR (DRM: VRR_ENABLED 0 on DP-1, vrr_capable 1)");
        assert_eq!(Reading::of(only(Some(1)), &shown).line(), "VRR on (DRM: VRR_ENABLED 1 on DP-1, vrr_capable 1)");
        assert!(matches!(Reading::of(only(None), &shown), Reading::Unknown(why) if why.contains("no VRR_ENABLED")));
        assert!(matches!(Reading::of(only(Some(7)), &shown), Reading::Unknown(why) if why.contains("reads 7")));
        assert_eq!(Reading::of(Err("no card could be read (card0: Permission denied)".into()), &shown).line(),
            "VRR not known, the pacing stays (DRM: no card could be read (card0: Permission denied))");
    }

    #[test]
    fn vrr_off_is_no_vrr_on_refreshes_on_present_and_unknown_says_nothing() {
        let off = Reading::Vrr { enabled: false, output: "DP-1".into(), capable: Some(1) };
        let on = Reading::Vrr { enabled: true, output: "DP-1".into(), capable: Some(1) };
        assert!(verdict(true, Some(&off)).fixed_refresh());
        assert_eq!(verdict(true, Some(&off)).line(), "off (no VRR)");
        assert!(verdict(true, Some(&on)).refreshes_on_present());
        assert!(!verdict(true, Some(&on)).fixed_refresh());
        let unknown = verdict(true, Some(&Reading::Unknown("x".into())));
        assert!(!unknown.fixed_refresh() && !unknown.refreshes_on_present());
        assert_eq!(unknown.line(), "off (no scanline)", "today's verdict");
        assert_eq!(verdict(true, None), Verdict::Checking);
        assert_eq!(verdict(false, Some(&off)).line(), "off (no tearing)");
    }

    #[test]
    fn vrr_enabled_on_a_connector_that_is_not_vrr_capable_is_no_vrr() {
        // amdgpu scans such a display at a fixed rate whatever the CRTC says.
        let incapable = Reading::Vrr { enabled: true, output: "HDMI-A-1".into(), capable: Some(0) };
        assert!(!incapable.runs());
        assert!(verdict(true, Some(&incapable)).fixed_refresh());
        assert_eq!(incapable.line(), "no VRR (DRM: VRR_ENABLED 1 on HDMI-A-1, vrr_capable 0)");
        let unsaid = Reading::Vrr { enabled: true, output: "DP-1".into(), capable: None };
        assert!(unsaid.runs(), "a connector without the property is taken at the CRTC's word");
        assert_eq!(unsaid.line(), "VRR on (DRM: VRR_ENABLED 1 on DP-1, vrr_capable unknown)");
    }

    #[test]
    fn a_read_waits_for_a_second_fullscreen_and_focused_and_comes_again_with_the_window() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut settle = Settle::default();
        assert_eq!(settle.present(false, 1, at(0)), Step::Wait, "windowed");
        assert_eq!(settle.present(true, 1, at(100)), Step::Wait);
        assert_eq!(settle.present(true, 1, at(1000)), Step::Wait, "900 ms");
        assert_eq!(settle.present(true, 1, at(1100)), Step::Read);
        assert_eq!(settle.present(true, 1, at(5000)), Step::Wait, "read once");
        // Alt-Tab and back: KDE's automatic VRR went off and comes on again.
        assert_eq!(settle.present(false, 1, at(5100)), Step::Wait);
        assert_eq!(settle.present(true, 1, at(5200)), Step::Wait);
        assert_eq!(settle.present(true, 1, at(6200)), Step::Read);
        // A recheck reads again a second later.
        settle.restart();
        assert_eq!(settle.present(true, 1, at(7000)), Step::Wait);
        assert_eq!(settle.present(true, 1, at(8000)), Step::Read);
        // Another display.
        assert_eq!(settle.present(true, 2, at(8100)), Step::Moved);
        assert_eq!(settle.present(true, 2, at(8200)), Step::Wait);
        assert_eq!(settle.present(true, 0, at(9200)), Step::Read, "an unknown display is no move");
    }

    #[test]
    fn a_mode_refresh_from_its_timings() {
        // 3440x1440 at 144 Hz, CVT-RB.
        assert!((mode_refresh_hz(799_750, 3600, 1543, 0, 0, 144) - 143.97).abs() < 0.01);
        assert_eq!(mode_refresh_hz(0, 0, 0, 0, 0, 60), 60.0, "no timings: the mode's own");
        assert!((mode_refresh_hz(74_250, 2200, 1125, 0, 1 << 4, 30) - 60.0).abs() < 0.01, "interlaced");
        assert_eq!(connector_type_name(14), "eDP");
        assert_eq!(connector_type_name(11), "HDMI-A");
    }
}
