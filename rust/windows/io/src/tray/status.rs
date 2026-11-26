//! What state the host is in and how the tray shows it: the status, its 7.0
//! badge colour (mirroring `rust/web/src/styles/tokens.css`), the badge
//! composited onto the brand icon as pure RGBA, the menu's header line and the
//! status-first menu. Win32 stays in `tray.rs` and `tray/icon.rs`.
use super::Action;
use butterpollo_session::tray::Icon;

/// `--brand`, the ruby of the Rubylight mark.
pub const BRAND: [u8; 3] = [0xc4, 0x12, 0x42];
/// `--media-ok`.
pub const MEDIA_OK: [u8; 3] = [0x45, 0xe0, 0xa0];
/// `--media-warn`.
pub const MEDIA_WARN: [u8; 3] = [0xf5, 0xb0, 0x41];
/// `--media-danger`.
pub const MEDIA_DANGER: [u8; 3] = [0xff, 0x70, 0x70];
/// The badge's outer hairline: reads on dark taskbars.
pub const RING_LIGHT: [u8; 3] = [0xff, 0xff, 0xff];
/// The badge's inner ring: reads on light taskbars and gives the fill a crisp edge.
pub const RING_DARK: [u8; 3] = [0x0b, 0x0d, 0x12];

/// What the host is doing, as the tray shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nothing runs: the plain brand icon.
    Idle,
    /// An app runs and a client streams it.
    Streaming,
    /// The app runs without a client (paused).
    Attention,
    /// The app closed right after it started.
    Error,
}

impl Status {
    pub const ALL: [Status; 4] = [
        Status::Idle,
        Status::Streaming,
        Status::Attention,
        Status::Error,
    ];

    /// The tray's status from the running app's icon state, and whether that
    /// app's launch failed.
    pub fn from_icon(icon: Icon, launch_failed: bool) -> Self {
        match icon {
            Icon::Idle => Status::Idle,
            _ if launch_failed => Status::Error,
            Icon::Streaming => Status::Streaming,
            Icon::Paused => Status::Attention,
        }
    }

    /// The status's 7.0 colour.
    pub fn color(self) -> [u8; 3] {
        match self {
            Status::Idle => BRAND,
            Status::Streaming => MEDIA_OK,
            Status::Attention => MEDIA_WARN,
            Status::Error => MEDIA_DANGER,
        }
    }

    /// Position in [`Status::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// Whether the icon carries a badge: Idle keeps the plain brand icon, as before 7.0.
    pub fn badged(self) -> bool {
        self != Status::Idle
    }
}

/// The badge's circle in a `size` px icon: diameter, outer hairline and dark ring widths.
/// About 45% of the icon, at least 8 px so the colour still reads at 16 px, and a whole
/// number of pixels so its centre sits on the pixel grid.
pub fn badge_geometry(size: usize) -> (f32, f32, f32) {
    let diameter = ((size as f32 * 0.45).round()).max(8.0).min(size as f32);
    let light = (size as f32 / 32.0).max(0.75);
    let dark = (size as f32 / 20.0).max(1.0);
    (diameter, light, dark)
}

/// Draw the status's badge in the bottom-right corner of a `width`×`height`
/// icon image: BGRA rows with straight alpha. False, leaving the image as is,
/// for Idle or an image without alpha (it then shows as is).
pub fn composite(pixels: &mut [u8], width: usize, height: usize, status: Status) -> bool {
    if !status.badged()
        || width == 0
        || height == 0
        || pixels.len() < width * height * 4
        || !pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 0)
    {
        return false;
    }
    let size = width.min(height);
    let (diameter, light, dark) = badge_geometry(size);
    let radius = diameter / 2.0;
    let inside_light = radius - light;
    let fill_radius = inside_light - dark;
    let (cx, cy) = (width as f32 - radius, height as f32 - radius);
    let fill = status.color();
    // Fraction of the pixel at `distance` inside a circle of `r`: a one-pixel ramp.
    let within = |r: f32, distance: f32| (r - distance + 0.5).clamp(0.0, 1.0);
    for y in 0..height {
        for x in 0..width {
            let distance = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let coverage = within(radius, distance);
            if coverage == 0.0 {
                continue;
            }
            let in_dark = within(inside_light, distance);
            let in_fill = within(fill_radius, distance);
            let mut color = [0f32; 3];
            for (c, value) in color.iter_mut().enumerate() {
                *value = f32::from(RING_LIGHT[c]) * (1.0 - in_dark)
                    + f32::from(RING_DARK[c]) * (in_dark - in_fill)
                    + f32::from(fill[c]) * in_fill;
            }
            let pixel = &mut pixels[(y * width + x) * 4..][..4];
            let below = f32::from(pixel[3]) / 255.0 * (1.0 - coverage);
            let alpha = coverage + below;
            // BGRA: the colour is RGB.
            for (c, rgb) in [2usize, 1, 0].into_iter().enumerate() {
                pixel[c] =
                    ((color[rgb] * coverage + f32::from(pixel[c]) * below) / alpha).round() as u8;
            }
            pixel[3] = (alpha * 255.0).round() as u8;
        }
    }
    true
}

/// The menu's first, disabled line: what is happening.
pub fn header(status: Status, app: Option<&str>) -> String {
    match (status, app) {
        (Status::Streaming, Some(app)) => format!("Streaming {app}"),
        (Status::Attention, Some(app)) => format!("Needs attention: {app} is paused"),
        (Status::Error, Some(app)) => {
            format!("Needs attention: {app} closed right after it started")
        }
        _ => "Ready".to_owned(),
    }
}

/// A line of the tray's menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// What is happening; never clickable.
    Header(String),
    Separator,
    Entry {
        action: Action,
        label: String,
        enabled: bool,
    },
}

fn entry(action: Action, label: impl Into<String>) -> Item {
    Item::Entry {
        action,
        label: label.into(),
        enabled: true,
    }
}

/// The status-first menu. `hide_controls` leaves out what changes the host
/// or ends a stream, as before.
pub fn menu(status: Status, app: Option<&str>, hide_controls: bool) -> Vec<Item> {
    let mut items = vec![
        Item::Header(header(status, app)),
        Item::Separator,
        entry(Action::Open, "Open Rubylight"),
        entry(Action::PairDevice, "Pair a device"),
        entry(Action::EnterPin, "Enter PIN"),
    ];
    if !hide_controls {
        items.extend([
            Item::Separator,
            entry(Action::StopSessions, "Stop streams and restore displays"),
            Item::Entry {
                action: Action::QuitApp,
                label: match app {
                    Some(app) => format!("Quit {app}"),
                    None => "No app running".to_owned(),
                },
                enabled: app.is_some(),
            },
        ]);
    }
    items.extend([
        Item::Separator,
        entry(Action::CheckUpdates, "Check for updates"),
    ]);
    if !hide_controls {
        items.extend([
            entry(Action::BackupSettings, "Back up settings"),
            entry(Action::RestoreSettings, "Restore settings..."),
            Item::Separator,
            entry(Action::Restart, "Restart Rubylight"),
            entry(Action::Quit, "Quit Rubylight"),
        ]);
    }
    items
}

/// A label as a menu shows it: an ampersand would underline the next letter.
pub fn menu_label(label: &str) -> String {
    label.replace('&', "&&")
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
