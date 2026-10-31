//! Display decisions that do not need Windows: whether a session gets a
//! virtual display, which mode to report, stable virtual display identities,
//! render sizes, letterboxing and display arrangements.
use crate::topology::{Node, Position};
use anyhow::{Result, bail};

/// A client request can enable a virtual display; an absent request leaves
/// the host, app and device settings in control, as does `virtualDisplay=0`,
/// which Apollo-compatible clients send whenever their own toggle is off.
#[derive(Default)]
pub struct VirtualDisplayRequest<'a> {
    pub client_requested: bool,
    pub client_forced: bool,
    pub app_requested: bool,
    pub configured: bool,
    pub output_override: Option<&'a str>,
    pub configured_output: &'a str,
    /// The client asked to stream a physical display (`hostDisplay=physical`):
    /// the host, app and per-device settings that would add a virtual display
    /// are overridden for this stream. A headless host and an output that
    /// names the virtual display still use one; there is nothing else to show.
    pub client_physical: bool,
    /// No display is active (a headless host, or every monitor off): there
    /// is no physical display to stream, so a virtual one is used whatever
    /// the settings say, as in Vibepollo.
    pub headless: bool,
}
impl VirtualDisplayRequest<'_> {
    /// Whether the stream gets a virtual display. Without a usable driver a
    /// request from the client, the app or the host settings streams the
    /// physical display (the caller warns), as Apollo and Vibepollo do: an
    /// Artemis client asked to proceed without the driver must still get a
    /// stream. Only an output that names the virtual display itself has no
    /// physical display to fall back to.
    pub fn uses_virtual(
        &self,
        physical_only: bool,
        driver_available: impl FnOnce() -> bool,
    ) -> bool {
        !physical_only && self.requested() && (self.output_virtual() || driver_available())
    }
    pub fn output_virtual(&self) -> bool {
        matches!(
            self.output_override
                .unwrap_or(self.configured_output)
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "sunshine:virtual_display" | "virtual" | "virtual_display" | "virtual-display"
        )
    }
    pub fn requested(&self) -> bool {
        if self.client_physical {
            return self.headless || self.output_virtual();
        }
        self.client_requested
            || self.client_forced
            || self.headless
            || self.output_virtual()
            || (self.output_override.is_none() && (self.app_requested || self.configured))
    }
}

/// The physical display mode to apply for a stream, given the modes the display
/// supports as (width, height, refresh Hz), the requested resolution and refresh
/// (millihertz), and the current mode.
///
/// Windows substitutes an arbitrary mode for one a display does not support:
/// a 2560x1600 request on a 32:9 monitor became 3840x1080 at 60 Hz, halving a
/// 120 fps stream. An unsupported resolution keeps the current one (the stream
/// is scaled on the GPU), and a refresh is applied only where it exists.
pub fn physical_mode(
    supported: &[(u32, u32, u32)],
    requested: (Option<(u32, u32)>, Option<u32>),
    current: (u32, u32, u32),
) -> (u32, u32, u32) {
    let (current_width, current_height, current_rate) = current;
    let hz = |millihertz: u32| millihertz.saturating_add(500) / 1000;
    let (width, height) = requested
        .0
        .filter(|(w, h)| supported.iter().any(|m| m.0 == *w && m.1 == *h))
        .unwrap_or((current_width, current_height));
    let rate = requested
        .1
        .filter(|rate| {
            supported
                .iter()
                .any(|m| m.0 == width && m.1 == height && m.2 == hz(*rate))
        })
        .or_else(|| {
            // Keep the current refresh when it exists at the chosen resolution;
            // otherwise the highest that does.
            let at = |rate: u32| {
                supported
                    .iter()
                    .any(|m| m.0 == width && m.1 == height && m.2 == hz(rate))
            };
            if at(current_rate) {
                Some(current_rate)
            } else {
                supported
                    .iter()
                    .filter(|m| m.0 == width && m.1 == height)
                    .map(|m| m.2 * 1000)
                    .max()
            }
        })
        .unwrap_or(current_rate);
    (width, height, rate)
}
pub fn report_mode(
    warnings: &crate::session::Warnings,
    requested: (Option<(u32, u32)>, Option<u32>, Option<bool>),
    actual: (u32, u32, u32, bool),
    stream_rate: u32,
) {
    if let Some((width, height)) = requested.0
        && (width, height) != (actual.0, actual.1)
    {
        warnings.set("display_resolution", format!("Display resolution {width}x{height} was not applied; using {}x{} and scaling the stream. The mode may be unsupported or shared with another stream; select a supported mode or a virtual display.", actual.0, actual.1));
    } else {
        warnings.clear("display_resolution");
    }
    if requested
        .1
        .is_some_and(|rate| rate.abs_diff(actual.2) > 500)
        || actual.2.saturating_add(500) < stream_rate
    {
        warnings.set("display_refresh", format!("Display refresh is {:.3} Hz for a {:.3} fps stream; the requested rate may be unsupported or held by another stream. Fresh frames cannot exceed the display rate. Select a supported refresh or a virtual display.", f64::from(actual.2) / 1000., f64::from(stream_rate) / 1000.));
    } else {
        warnings.clear("display_refresh");
    }
    if requested.2.is_some_and(|hdr| hdr != actual.3) {
        warnings.set("display_hdr", format!("Requested display HDR state was not applied; capture source HDR is {}. The display may lack HDR or be shared. Check Windows HDR and the display policy, or reconnect with HDR disabled.", if actual.3 { "on" } else { "off" }));
    } else {
        warnings.clear("display_hdr");
    }
}

/// Preserve libdisplaydevice's UUIDv5 identity, including its UTF-16 byte order
/// and removal of the unstable parent portion of Windows' instance ID.
pub fn legacy_device_id(path: &str, instance: Option<&str>, edid: &[u8]) -> String {
    let mut bytes = Vec::new();
    if let Some(instance) = instance {
        let separators: Vec<_> = instance
            .match_indices('&')
            .map(|(at, _)| at)
            .take(3)
            .collect();
        if separators.len() == 3 {
            bytes.extend_from_slice(edid);
            for part in [&instance[..separators[1]], &instance[separators[2]..]] {
                bytes.extend(part.encode_utf16().flat_map(u16::to_le_bytes));
            }
        }
    }
    if bytes.is_empty() {
        bytes.extend(path.encode_utf16().flat_map(u16::to_le_bytes));
    }
    format!("{{{}}}", uuid::Uuid::new_v5(&uuid::Uuid::nil(), &bytes))
}
fn fnv(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}
/// Match uuid_util::parse's Windows GUID byte order and the previous host's
/// deterministic fallback domains. The driver hashes those 16 memory bytes.
pub fn virtual_identity(stable_id: &str) -> [u8; 16] {
    if let Ok(uuid) = uuid::Uuid::parse_str(stable_id) {
        return uuid.to_bytes_le();
    }
    let mut bytes = [0; 16];
    for (at, domain) in [
        (0, "sunshine-virtual-display-a:"),
        (8, "sunshine-virtual-display-b:"),
    ] {
        bytes[at..at + 8]
            .copy_from_slice(&fnv(domain.bytes().chain(stable_id.bytes())).to_le_bytes());
    }
    bytes[6] = (bytes[6] & 15) | 0x50;
    bytes[8] = (bytes[8] & 63) | 0x80;
    bytes
}
pub fn virtual_display_id(stable_id: &str) -> u64 {
    fnv(virtual_identity(stable_id)).max(1)
}
pub fn app_client_identity(app: &str, client: &str) -> String {
    let mut bytes = virtual_identity(app);
    for (byte, client_byte) in bytes.iter_mut().zip(virtual_identity(client)) {
        *byte ^= client_byte;
    }
    uuid::Uuid::from_bytes_le(bytes).to_string()
}

/// A non-default application scale takes precedence over the client's scale.
/// The previous host rounded down to even dimensions and ignored overflow.
pub fn render_dimensions(width: u32, height: u32, client_scale: i64, app_scale: i64) -> (u32, u32) {
    let scale = if app_scale != 100 {
        app_scale
    } else {
        client_scale
    };
    if scale <= 0 || scale == 100 {
        return (width, height);
    }
    let dimension = |value: u32| {
        u64::from(value)
            .checked_mul(scale as u64)
            .map(|value| (value / 100) & !1)
            .filter(|value| *value > 0 && *value <= i32::MAX as u64)
            .map(|value| value as u32)
    };
    match (dimension(width), dimension(height)) {
        (Some(width), Some(height)) => (width, height),
        _ => (width, height),
    }
}
/// The launch option a client sets the Windows scale (text and interface
/// size) of its virtual display with, in percent. `scaleFactor` is a
/// different thing: it multiplies the size the PC renders at
/// ([`render_dimensions`]).
pub const DISPLAY_SCALE_PARAMETER: &str = "rlDisplayScale";
/// The serverinfo element that says the host reads [`DISPLAY_SCALE_PARAMETER`].
pub const DISPLAY_SCALE_SERVERINFO_FIELD: &str = "DisplayScaleVersion";
/// The display scales Windows offers, in percent.
pub const DISPLAY_SCALES: [u32; 12] = [100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];

/// The client's `rlDisplayScale` choice: None when it sent none, an error for
/// a scale Windows does not offer. A choice comes before the device, app and
/// host `dd_virtual_display_scale`, as the client's display arrangement does.
pub fn client_display_scale(
    options: &std::collections::BTreeMap<String, String>,
) -> Result<Option<i64>> {
    let Some(value) = options.get(DISPLAY_SCALE_PARAMETER) else {
        return Ok(None);
    };
    match value.trim().parse::<u32>() {
        Ok(percent) if DISPLAY_SCALES.contains(&percent) => Ok(Some(i64::from(percent))),
        _ => bail!(
            "invalid {DISPLAY_SCALE_PARAMETER} {value:?}: Windows offers {DISPLAY_SCALES:?} percent"
        ),
    }
}

/// Where a source of another shape sits in the stream, as `(x, y, width,
/// height)`: centred, keeping its aspect ratio, with black bars around it.
/// Sizes and offsets are even for 4:2:0 chroma. A shape within a pixel of
/// the stream's fills it, as in the GPU converter.
pub fn letterbox(source: (u32, u32), target: (u32, u32)) -> (u32, u32, u32, u32) {
    let full = (0, 0, target.0, target.1);
    if source.0 == 0 || source.1 == 0 || source == target {
        return full;
    }
    let (sw, sh) = (f64::from(source.0), f64::from(source.1));
    let (tw, th) = (f64::from(target.0), f64::from(target.1));
    let scale = (tw / sw).min(th / sh);
    let (w, h) = (sw * scale, sh * scale);
    if (w - tw).abs() < 1. && (h - th).abs() < 1. {
        return full;
    }
    let even = |value: f64, limit: u32| ((value.round() as u32) & !1).max(2).min(limit);
    let (w, h) = (even(w, target.0), even(h, target.1));
    (((target.0 - w) / 2) & !1, ((target.1 - h) / 2) & !1, w, h)
}
/// Where the display's picture sits in the stream, or None when it fills the
/// stream: a display of another shape is letterboxed (see `letterbox`).
pub fn picture(display: (u32, u32), stream: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    if stream.0 == 0 || stream.1 == 0 {
        return None;
    }
    let picture = letterbox(display, stream);
    (picture != (0, 0, stream.0, stream.1)).then_some(picture)
}
/// A point the client gives as fractions of the whole stream, black bars
/// included, as fractions of the picture, that is of the display. A point on
/// a bar moves to the picture's edge.
pub fn stream_to_picture(
    point: (f64, f64),
    stream: (u32, u32),
    picture: (u32, u32, u32, u32),
) -> (f64, f64) {
    let map = |value: f64, total: u32, offset: u32, size: u32| {
        ((value.clamp(0., 1.) * f64::from(total) - f64::from(offset)) / f64::from(size.max(1)))
            .clamp(0., 1.)
    };
    (
        map(point.0, stream.0, picture.0, picture.2),
        map(point.1, stream.1, picture.1, picture.3),
    )
}
/// A device's own display mode, `WIDTHxHEIGHTxREFRESH` (refresh in Hz, up to
/// three decimals, e.g. `1920x1080x59.94`).
pub fn parse_display_mode(text: &str) -> Option<(u32, u32, crate::framegen::Rate)> {
    let mut parts = text.trim().split(['x', 'X']);
    let width = parts.next()?.trim().parse::<u32>().ok()?;
    let height = parts.next()?.trim().parse::<u32>().ok()?;
    let rate = crate::framegen::Rate::parse(parts.next()?).ok()?;
    (parts.next().is_none()
        && (1..=16384).contains(&width)
        && (1..=16384).contains(&height)
        && rate.0 >= 1000)
        .then_some((width, height, rate))
}
/// A resolution a device keeps for its virtual display, `WIDTHxHEIGHT`.
pub fn parse_resolution(text: &str) -> Option<(u32, u32)> {
    let (width, height) = text.trim().split_once(['x', 'X'])?;
    let width = width.trim().parse::<u32>().ok()?;
    let height = height.trim().parse::<u32>().ok()?;
    ((1..=16384).contains(&width) && (1..=16384).contains(&height)).then_some((width, height))
}
/// A `DISPLAYCONFIG_ROTATION` (1 upright to 4, 270° clockwise) in degrees.
pub fn rotation_degrees(rotation: u32) -> Option<u16> {
    match rotation {
        1 => Some(0),
        2 => Some(90),
        3 => Some(180),
        4 => Some(270),
        _ => None,
    }
}
/// Degrees clockwise as a `DISPLAYCONFIG_ROTATION`.
pub fn rotation_code(degrees: u16) -> Option<u32> {
    match degrees {
        0 => Some(1),
        90 => Some(2),
        180 => Some(3),
        270 => Some(4),
        _ => None,
    }
}
fn portrait((width, height): (u32, u32)) -> bool {
    height > width
}
/// `size` in portrait or landscape: swapped when it has the other shape. A
/// square stays as it is.
pub fn oriented(size: (u32, u32), portrait: bool) -> (u32, u32) {
    if size.0 != size.1 && self::portrait(size) != portrait {
        (size.1, size.0)
    } else {
        size
    }
}
/// The rotation in degrees a stream turns a physical display to under
/// `dd_rotation_option`, or None to leave it as it is. `current` is its
/// rotation now and `desktop` its size as the desktop shows it, turned.
/// Following the stream turns the display only when its shape differs from
/// the stream's: a turned display goes back upright, an upright one to 90°.
pub fn physical_rotation(
    option: crate::config::DisplayAdjust,
    manual: u16,
    current: u16,
    desktop: (u32, u32),
    stream: (u32, u32),
) -> Option<u16> {
    use crate::config::DisplayAdjust;
    let wanted = match option {
        DisplayAdjust::Disabled => return None,
        DisplayAdjust::Manual => manual,
        DisplayAdjust::Auto => {
            if stream.0 == stream.1
                || desktop.0 == desktop.1
                || portrait(stream) == portrait(desktop)
            {
                return None;
            }
            if current.is_multiple_of(180) { 90 } else { 0 }
        }
    };
    (wanted != current).then_some(wanted)
}
/// The mode a physical display gets before it turns: the requested size in the
/// shape the display shows now (`desktop`), so that it has the shape the turn
/// gives it afterwards. Windows lists a display's modes in its current shape.
pub fn mode_before_turn(requested: (u32, u32), desktop: (u32, u32)) -> (u32, u32) {
    oriented(requested, portrait(desktop))
}
/// The size a stream's own virtual display is made at under
/// `dd_rotation_option`: `size` swapped for 90° and 270°, or in the stream's
/// shape when it follows the stream. A virtual display is shaped rather than
/// turned, since a turned one would reach the encoder sideways; 180° leaves it
/// as it is.
pub fn virtual_shape(
    option: crate::config::DisplayAdjust,
    manual: u16,
    size: (u32, u32),
    stream: (u32, u32),
) -> (u32, u32) {
    use crate::config::DisplayAdjust;
    match option {
        DisplayAdjust::Disabled => size,
        DisplayAdjust::Manual if manual % 180 == 90 => (size.1, size.0),
        DisplayAdjust::Manual => size,
        DisplayAdjust::Auto if stream.0 == stream.1 => size,
        DisplayAdjust::Auto => oriented(size, portrait(stream)),
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrangement {
    Extended,
    Primary,
    Exclusive,
    Isolated,
    PrimaryIsolated,
}
impl Arrangement {
    /// The `virtual_display_layout` value that selects this arrangement.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Extended => "extended",
            Self::Primary => "extended_primary",
            Self::Exclusive => "exclusive",
            Self::Isolated => "extended_isolated",
            Self::PrimaryIsolated => "extended_primary_isolated",
        }
    }
    pub fn parse(value: &str) -> Result<Self> {
        Ok(
            match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
                "extended" | "ensure_active" => Self::Extended,
                "extended_primary" | "ensure_primary" => Self::Primary,
                "exclusive" | "ensure_only_display" => Self::Exclusive,
                "extended_isolated" => Self::Isolated,
                "extended_primary_isolated" => Self::PrimaryIsolated,
                _ => bail!("unknown display arrangement"),
            },
        )
    }
    /// The layout a remote monitor's stream applies. Its position on the
    /// desktop comes from the remote monitor layout, so it takes only which
    /// displays stay on and which is primary: exclusive turns the others off,
    /// the primary layouts make it primary, and the extended and isolated
    /// placements leave it where the remote monitor layout put it.
    pub const fn for_remote_monitor(self) -> Option<Self> {
        match self {
            Self::Exclusive => Some(Self::Exclusive),
            Self::Primary | Self::PrimaryIsolated => Some(Self::Primary),
            Self::Extended | Self::Isolated => None,
        }
    }
    /// Apply the first stream's virtual-display layout after hotplug. Windows
    /// can recall this virtual display as primary even though the user's
    /// physical displays stay on. Extended layouts keep the primary from
    /// before hotplug; deliberate primary/exclusive layouts still replace it.
    pub fn compose_after_hotplug(
        self,
        nodes: &[Node],
        target: &str,
        retained: &[String],
        before: &[Node],
    ) -> Result<Vec<Node>> {
        let mut current = nodes.to_vec();
        self.preserve_primary(&mut current, before)?;
        self.compose(&current, target, retained)
    }
    /// Keep the first owner's primary when another stream joins or an owned
    /// virtual display is recreated. Owner order still decides which stream
    /// is primary for the deliberate primary/exclusive layouts.
    pub fn compose_all_after_hotplug(
        self,
        nodes: &[Node],
        targets: &[String],
        retained: &[String],
        before: &[Node],
    ) -> Result<Vec<Node>> {
        let mut current = nodes.to_vec();
        self.preserve_primary(&mut current, before)?;
        self.compose_all(&current, targets, retained)
    }
    fn preserve_primary(self, nodes: &mut [Node], before: &[Node]) -> Result<()> {
        if !matches!(self, Self::Extended | Self::Isolated) {
            return Ok(());
        }
        let primary = before
            .iter()
            .find(|node| node.active && node.primary)
            .and_then(|original| {
                nodes
                    .iter()
                    .find(|node| node.active && node.device_id == original.device_id)
            })
            .map(|node| (node.device_id.clone(), node.desired_position));
        let Some((primary_id, origin)) = primary else {
            // Headless, disconnected or unreadable original layout: don't
            // invent a primary or activate a display the user switched off.
            return Ok(());
        };
        for node in nodes {
            node.primary = node.device_id == primary_id;
            node.desired_position.x = node
                .desired_position
                .x
                .checked_sub(origin.x)
                .ok_or_else(|| anyhow::anyhow!("display position overflow"))?;
            node.desired_position.y = node
                .desired_position
                .y
                .checked_sub(origin.y)
                .ok_or_else(|| anyhow::anyhow!("display position overflow"))?;
        }
        Ok(())
    }
    pub fn compose(self, nodes: &[Node], target: &str, retained: &[String]) -> Result<Vec<Node>> {
        let target_node = nodes
            .iter()
            .find(|n| n.device_id == target)
            .ok_or_else(|| anyhow::anyhow!("display arrangement target is missing"))?;
        let mut result = nodes.to_vec();
        if self == Self::Exclusive {
            for node in &mut result {
                node.active = node.device_id == target || retained.contains(&node.device_id);
            }
        }
        let primary = matches!(
            self,
            Self::Primary | Self::Exclusive | Self::PrimaryIsolated
        );
        if primary {
            for node in &mut result {
                node.primary = node.device_id == target;
                node.desired_position.x = node
                    .desired_position
                    .x
                    .checked_sub(target_node.desired_position.x)
                    .ok_or_else(|| anyhow::anyhow!("display position overflow"))?;
                node.desired_position.y = node
                    .desired_position
                    .y
                    .checked_sub(target_node.desired_position.y)
                    .ok_or_else(|| anyhow::anyhow!("display position overflow"))?;
            }
        }
        if self == Self::Isolated {
            result
                .iter_mut()
                .find(|n| n.device_id == target)
                .unwrap()
                .desired_position = Position { x: 64000, y: 64000 };
        }
        if self == Self::PrimaryIsolated {
            let min_x = result
                .iter()
                .filter(|n| n.device_id != target)
                .map(|n| n.desired_position.x)
                .min()
                .unwrap_or(0);
            let min_y = result
                .iter()
                .filter(|n| n.device_id != target)
                .map(|n| n.desired_position.y)
                .min()
                .unwrap_or(0);
            for node in result.iter_mut().filter(|n| n.device_id != target) {
                node.desired_position.x = 64000i32
                    .checked_add(
                        node.desired_position
                            .x
                            .checked_sub(min_x)
                            .ok_or_else(|| anyhow::anyhow!("display position overflow"))?,
                    )
                    .ok_or_else(|| anyhow::anyhow!("display position overflow"))?;
                node.desired_position.y = 64000i32
                    .checked_add(
                        node.desired_position
                            .y
                            .checked_sub(min_y)
                            .ok_or_else(|| anyhow::anyhow!("display position overflow"))?,
                    )
                    .ok_or_else(|| anyhow::anyhow!("display position overflow"))?;
            }
        }
        Ok(result)
    }
    /// One layout for several streams. The first target is laid out as for a
    /// single stream; each later target stays active and is placed to the right
    /// of the streamed displays, so no two of them overlap. An isolated layout
    /// keeps the streamed displays next to each other, away from the others.
    pub fn compose_all(
        self,
        nodes: &[Node],
        targets: &[String],
        retained: &[String],
    ) -> Result<Vec<Node>> {
        let (first, rest) = targets
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("display arrangement has no target"))?;
        let mut keep = retained.to_vec();
        keep.extend(rest.iter().cloned());
        let mut result = self.compose(nodes, first, &keep)?;
        let isolated = matches!(self, Self::Isolated | Self::PrimaryIsolated);
        let mut placed = vec![first.clone()];
        for target in rest {
            if !result.iter().any(|n| n.device_id == *target) {
                bail!("display arrangement target is missing");
            }
            let y = result
                .iter()
                .find(|n| n.device_id == *first)
                .map_or(0, |n| n.desired_position.y);
            let right = result
                .iter()
                .filter(|n| {
                    n.active
                        && if isolated {
                            placed.contains(&n.device_id)
                        } else {
                            !rest.contains(&n.device_id) || placed.contains(&n.device_id)
                        }
                })
                .map(|n| {
                    n.desired_position
                        .x
                        .saturating_add(i32::try_from(n.mode.width).unwrap_or(i32::MAX))
                })
                .max()
                .unwrap_or(0);
            let node = result
                .iter_mut()
                .find(|n| n.device_id == *target)
                .expect("checked above");
            node.active = true;
            node.primary = false;
            node.desired_position = Position { x: right, y };
            placed.push(target.clone());
        }
        Ok(result)
    }
}
/// What a client asked the PC's displays to do for one stream: the
/// `hostDisplay` launch and resume parameter. It overrides the host, app and
/// per-device display settings for that stream only; without it (or with a
/// value this host does not know, such as `default`) those settings decide,
/// so older clients are unaffected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostDisplay {
    /// Stream a physical display; no virtual display is added.
    Physical,
    /// Add a virtual display for the stream and lay the desktop out like this
    /// (`exclusive` turns the other displays off for the stream).
    Virtual(Arrangement),
}
impl HostDisplay {
    pub const PARAMETER: &'static str = "hostDisplay";
    /// `physical`, or any `virtual_display_layout` value; anything else is
    /// None, leaving the host's settings in control.
    pub fn parse(value: &str) -> Option<Self> {
        if value.trim().eq_ignore_ascii_case("physical") {
            return Some(Self::Physical);
        }
        let value = value.trim().to_ascii_lowercase().replace('-', "_");
        Arrangement::parse(&value)
            .ok()
            .filter(|arrangement| value == arrangement.name())
            .map(Self::Virtual)
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::Physical => "physical",
            Self::Virtual(arrangement) => arrangement.name(),
        }
    }
    /// Whether a display prepared for one launch can serve the next: a
    /// launch that asks for nothing takes it as it is, and one that asks
    /// gets a display set up for exactly that request.
    pub fn reusable(prepared_for: Option<Self>, requested: Option<Self>) -> bool {
        requested.is_none() || prepared_for == requested
    }
}
/// Displays a stream's layout switched off that are on again. Windows puts
/// back the layout it has saved for the connected displays when an
/// exclusive-fullscreen game loses focus (the Win key, Alt+Tab, Ctrl+Alt+Del),
/// and that layout has the physical monitor on. `before`: the user's layout
/// from before the stream; `applied`: the stream's; `active`: the displays on
/// now. A display that was already off before the stream is the user's to
/// switch on, and one the stream never laid out (another client's display on
/// its way in) is not counted.
pub fn switched_back_on(before: &[Node], applied: &[Node], active: &[String]) -> Vec<String> {
    applied
        .iter()
        .filter(|n| !n.active && active.contains(&n.device_id))
        .filter(|n| {
            before
                .iter()
                .any(|b| b.device_id == n.device_id && b.active)
        })
        .map(|n| n.device_id.clone())
        .collect()
}

/// The layout the user left a stream's virtual display in, when it differs from
/// `planned`, for the device to keep (issue #33: Windows' "Show only on 2"
/// during an extended stream was back to extended at the next connection).
/// `active` lists the displays on now and whether each is primary. Only the
/// stream's display on is `exclusive`; with others on it is `extended`, or
/// `extended_primary` when it is the primary. A planned `exclusive` is left
/// alone: the stream puts that layout back itself when Windows switches a
/// display on, so a different one at the end came from Windows or a game, not
/// the user. Isolated layouts, which Windows' own choices cannot express, and a
/// stream display that is off are never learned.
pub fn chosen_layout(
    planned: &str,
    stream_display: &str,
    active: &[(String, bool)],
) -> Option<&'static str> {
    if !matches!(planned, "extended" | "extended_primary") {
        return None;
    }
    let primary = active
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(stream_display))
        .map(|(_, primary)| *primary)?;
    let chosen = if active.len() == 1 {
        "exclusive"
    } else if primary {
        "extended_primary"
    } else {
        "extended"
    };
    (chosen != planned).then_some(chosen)
}

/// When the heartbeat checks [`switched_back_on`] and puts the stream's layout
/// back. A display must stay on for [`LayoutWatch::SETTLE`] first, so a game's
/// own mode switch passes without a fight; a failed attempt (the secure
/// desktop refuses display changes) waits [`LayoutWatch::RETRY`].
#[derive(Debug, Default)]
pub struct LayoutWatch {
    next_check: Option<std::time::Instant>,
    since: Option<std::time::Instant>,
}
impl LayoutWatch {
    pub const INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
    pub const SETTLE: std::time::Duration = std::time::Duration::from_millis(500);
    pub const RETRY: std::time::Duration = std::time::Duration::from_secs(2);
    /// Whether to query the displays now.
    pub fn due(&mut self, now: std::time::Instant) -> bool {
        if self.next_check.is_some_and(|next| now < next) {
            return false;
        }
        self.next_check = Some(now + Self::INTERVAL);
        true
    }
    /// Record a check; true when the layout should be put back now.
    pub fn observe(&mut self, switched_on: bool, now: std::time::Instant) -> bool {
        if !switched_on {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        if now.duration_since(since) < Self::SETTLE {
            return false;
        }
        self.since = None;
        true
    }
    pub fn failed(&mut self, now: std::time::Instant) {
        self.since = None;
        self.next_check = Some(now + Self::RETRY);
    }
}
#[cfg(test)]
mod tests;
