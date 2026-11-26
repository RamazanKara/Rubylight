//! Where each connected display sits on the desktop, for the console's read-only display map.
//!
//! Only reads what Windows reports (the current mode and DPI scale); nothing here changes a display.
use super::*;

/// A display's place and mode on the desktop, in Windows' desktop pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// The current refresh rate in Hz, rounded as Windows reports it.
    pub refresh: u32,
    /// Windows' scale for the display in percent; left out when Windows does not say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<u32>,
}

/// A display with its place on the desktop, as `/api/display-devices` lists it.
#[derive(Serialize)]
pub struct Placed<'a> {
    #[serde(flatten)]
    pub monitor: &'a Monitor,
    /// Left out when Windows cannot report the display's mode, as while it comes or goes.
    #[serde(flatten)]
    pub geometry: Option<Geometry>,
    /// A Rubylight virtual display rather than a physical one.
    #[serde(rename = "virtual")]
    pub is_virtual: bool,
}

impl Geometry {
    fn of(mode: &DEVMODEW, scale: Option<u32>) -> Option<Self> {
        // SAFETY: A display DEVMODEW's union holds its desktop position.
        let position = unsafe { mode.Anonymous1.Anonymous2.dmPosition };
        (mode.dmPelsWidth > 0 && mode.dmPelsHeight > 0).then_some(Self {
            x: position.x,
            y: position.y,
            width: mode.dmPelsWidth,
            height: mode.dmPelsHeight,
            refresh: mode.dmDisplayFrequency,
            scale,
        })
    }
}

/// The display's current place and mode, or `None` when Windows cannot report it.
pub fn geometry(monitor: &Monitor) -> Option<Geometry> {
    let mode = modes::mode(&monitor.display_name).ok()?;
    Geometry::of(&mode, modes::dpi_scale(monitor).ok())
}

/// Every display with its place on the desktop.
pub fn placed(monitors: &[Monitor]) -> Vec<Placed<'_>> {
    monitors
        .iter()
        .map(|monitor| Placed {
            monitor,
            geometry: geometry(monitor),
            is_virtual: monitor.is_rubylight(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> Monitor {
        Monitor {
            device_id: "{id}".into(),
            monitor_device_path: String::new(),
            display_name: r"\\.\DISPLAY1".into(),
            friendly_name: "Studio".into(),
            hdr_supported: true,
            hdr_enabled: false,
            hdr_requested: false,
            primary: true,
            adapter: LUID::default(),
            target: 0,
            source: 0,
        }
    }

    fn mode(x: i32, y: i32, width: u32, height: u32, refresh: u32) -> DEVMODEW {
        let mut mode = DEVMODEW {
            dmPelsWidth: width,
            dmPelsHeight: height,
            dmDisplayFrequency: refresh,
            ..Default::default()
        };
        mode.Anonymous1.Anonymous2.dmPosition = POINTL { x, y };
        mode
    }

    #[test]
    fn a_mode_gives_the_place_size_refresh_and_scale() {
        let geometry = Geometry::of(&mode(-2560, 120, 2560, 1440, 144), Some(125)).unwrap();
        assert_eq!(
            geometry,
            Geometry {
                x: -2560,
                y: 120,
                width: 2560,
                height: 1440,
                refresh: 144,
                scale: Some(125)
            }
        );
        // A mode without a size is a display that is going away: no place is claimed for it.
        assert_eq!(Geometry::of(&mode(0, 0, 0, 1080, 60), None), None);
    }

    #[test]
    fn the_list_keeps_every_field_it_had_and_adds_the_place() {
        let monitor = monitor();
        let placed = Placed {
            monitor: &monitor,
            geometry: Geometry::of(&mode(0, 0, 3840, 2160, 120), None),
            is_virtual: false,
        };
        let value = serde_json::to_value(&placed).unwrap();
        let before = serde_json::to_value(&monitor).unwrap();
        for (key, field) in before.as_object().unwrap() {
            assert_eq!(&value[key], field, "{key}");
        }
        assert_eq!(value["width"], 3840);
        assert_eq!(value["refresh"], 120);
        assert_eq!(value["virtual"], false);
        assert!(value.get("scale").is_none());
        // Without a mode the display is still listed, with no place.
        let bare = serde_json::to_value(Placed {
            monitor: &monitor,
            geometry: None,
            is_virtual: true,
        })
        .unwrap();
        assert!(bare.get("x").is_none());
        assert_eq!(bare["virtual"], true);
        assert_eq!(bare["device_id"], "{id}");
    }
}
