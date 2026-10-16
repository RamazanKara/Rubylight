//! A release's floor: the oldest Windows build, the GPU and the display driver it runs on.
//! `rust/release/floor.json` is the one source. Setup refuses to install below this
//! release's floor; every release's notes carry its floor as a marker
//! (`<!-- rubylight-floor {...} -->`), and the updater offers a release only to a PC that meets
//! it. Hosts from before the marker still offer such a release; their setup refusal is the
//! safeguard.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The marker's opening, followed by the floor as one line of JSON and ` -->`.
pub const MARKER: &str = "<!-- rubylight-floor ";

/// `gpu`: an AMD adapter at D3D12 feature level 12_2 (DirectX 12 Ultimate, which RDNA 2
/// brought to Radeon) on which AMF creates the HEVC encoder.
pub const GPU_RDNA2_HEVC: &str = "amd-rdna2-hevc";
/// `display_driver`: Rubylight's own display driver, which renders on D3D12 and loads only
/// where Windows reports IddCx's D3D12 swap chain.
pub const DRIVER_D3D12: &str = "rubylight-vdisplay-d3d12";

/// This release's floor, as JSON.
pub const THIS_RELEASE: &str = include_str!("../../release/floor.json");

/// This release's floor.
pub fn this_release() -> Map<String, Value> {
    serde_json::from_str(THIS_RELEASE).expect("rust/release/floor.json is a JSON object")
}

/// The marker for `floor`, for a release's notes.
pub fn marker(floor: &Map<String, Value>) -> String {
    format!("{MARKER}{} -->", Value::Object(floor.clone()))
}

/// The floor a release's notes carry, or `None` for a release from before the marker. A
/// marker that does not parse counts as a floor nobody meets.
pub fn from_notes(body: &str) -> Option<Map<String, Value>> {
    let start = body.find(MARKER)? + MARKER.len();
    let rest = &body[start..];
    let json = &rest[..rest.find(" -->").unwrap_or(rest.len())];
    Some(match serde_json::from_str::<Value>(json) {
        Ok(Value::Object(floor)) => floor,
        _ => Map::from_iter([("unreadable".into(), Value::String(json.into()))]),
    })
}

/// One display adapter as the floor check saw it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Adapter {
    pub name: String,
    /// PCI vendor ID.
    pub vendor: u32,
    /// It has D3D12 feature level 12_2, and AMF created and initialized the HEVC encoder on
    /// it (AMD adapters only).
    pub hevc: bool,
    /// Why not, for an AMD adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hevc_error: Option<String>,
}

/// What a PC offers against a floor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pc {
    /// `CurrentBuildNumber`; `None` where Windows did not say.
    pub windows_build: Option<u32>,
    pub adapters: Vec<Adapter>,
    /// Rubylight's display driver is running.
    pub display_driver: bool,
}

/// Which part of a floor a PC misses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Windows,
    Gpu,
    DisplayDriver,
    /// A requirement this version does not know, so it cannot show the PC meets it.
    Unknown,
}

/// One requirement a PC does not meet: what was found, and what is required.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unmet {
    pub part: Part,
    pub message: String,
}

/// Every requirement of `floor` that `pc` misses. `driver` says whether the display driver
/// counts: setup installs the driver itself and judges it by installing.
pub fn unmet(floor: &Map<String, Value>, pc: &Pc, driver: bool) -> Vec<Unmet> {
    let mut missing = Vec::new();
    for (key, value) in floor {
        match (key.as_str(), value) {
            ("windows_build", Value::Number(required)) => {
                let required = required.as_u64().unwrap_or(u64::MAX);
                if pc.windows_build.is_none_or(|build| u64::from(build) < required) {
                    let found = pc
                        .windows_build
                        .map_or("an unknown build".to_owned(), |build| format!("build {build}"));
                    missing.push(Unmet {
                        part: Part::Windows,
                        message: format!(
                            "Rubylight needs Windows 11 build {required} or newer; this PC runs {found}. Install the current Windows 11 update, then run setup again."
                        ),
                    });
                }
            }
            ("gpu", Value::String(gpu)) if gpu == GPU_RDNA2_HEVC => {
                if !pc.adapters.iter().any(|a| a.hevc) {
                    missing.push(Unmet {
                        part: Part::Gpu,
                        message: format!(
                            "Rubylight needs an AMD Radeon GPU of the RX 6000 series or newer (RDNA 2, including RDNA 2 APUs) whose video encoder creates HEVC streams; {}.",
                            found_gpus(&pc.adapters)
                        ),
                    });
                }
            }
            ("display_driver", Value::String(name)) if name == DRIVER_D3D12 => {
                if driver && !pc.display_driver {
                    missing.push(Unmet {
                        part: Part::DisplayDriver,
                        message: "Rubylight needs its display driver, which is not running on this PC. Run the Rubylight installer again; it says what Windows refused.".into(),
                    });
                }
            }
            _ => missing.push(Unmet {
                part: Part::Unknown,
                message: format!(
                    "the release requires {key} = {value}, which this version of Rubylight cannot check"
                ),
            }),
        }
    }
    missing
}

fn found_gpus(adapters: &[Adapter]) -> String {
    if adapters.is_empty() {
        return "this PC has no hardware display adapter".into();
    }
    let found = adapters
        .iter()
        .map(|a| match &a.hevc_error {
            Some(error) => format!("{} ({error})", a.name),
            None if a.vendor == 0x1002 => format!("{} (no HEVC encoder)", a.name),
            None => a.name.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("this PC has {found}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn floor() -> Map<String, Value> {
        json!({"windows_build": 26200, "gpu": GPU_RDNA2_HEVC, "display_driver": DRIVER_D3D12})
            .as_object()
            .unwrap()
            .clone()
    }

    fn radeon(hevc: bool) -> Adapter {
        Adapter {
            name: "AMD Radeon RX 5700 XT".into(),
            vendor: 0x1002,
            hevc,
            hevc_error: (!hevc).then(|| "D3D12 feature level 12_2: not supported".into()),
        }
    }

    fn pc() -> Pc {
        Pc {
            windows_build: Some(26200),
            adapters: vec![radeon(true)],
            display_driver: true,
        }
    }

    #[test]
    fn this_release_has_a_floor_its_own_check_understands() {
        let floor = this_release();
        assert!(floor.contains_key("windows_build"));
        assert_eq!(floor["gpu"], GPU_RDNA2_HEVC);
        assert_eq!(floor["display_driver"], DRIVER_D3D12);
        assert!(unmet(&floor, &pc(), true).is_empty());
    }

    #[test]
    fn a_marker_in_release_notes_round_trips() {
        let body = format!("Notes.\n\n{}\n", marker(&floor()));
        assert_eq!(from_notes(&body), Some(floor()));
        assert_eq!(from_notes("Notes from before the marker."), None);
    }

    #[test]
    fn an_unreadable_marker_is_a_floor_nobody_meets() {
        let floor = from_notes("<!-- rubylight-floor {broken -->").unwrap();
        assert_eq!(unmet(&floor, &pc(), true)[0].part, Part::Unknown);
    }

    #[test]
    fn each_part_below_the_floor_is_named_with_what_was_found() {
        let old = Pc {
            windows_build: Some(22631),
            ..pc()
        };
        let missing = unmet(&floor(), &old, true);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].part, Part::Windows);
        assert!(missing[0].message.contains("26200"));
        assert!(missing[0].message.contains("build 22631"));

        let rdna1 = Pc {
            adapters: vec![radeon(false)],
            ..pc()
        };
        let missing = unmet(&floor(), &rdna1, true);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].part, Part::Gpu);
        assert!(missing[0].message.contains("RX 6000"));
        assert!(
            missing[0]
                .message
                .contains("RX 5700 XT (D3D12 feature level 12_2: not supported)")
        );

        let other = Pc {
            adapters: vec![Adapter {
                name: "Some Other GPU".into(),
                vendor: 0x10de,
                ..Default::default()
            }],
            ..pc()
        };
        assert!(
            unmet(&floor(), &other, true)[0]
                .message
                .contains("this PC has Some Other GPU")
        );

        let no_driver = Pc {
            display_driver: false,
            ..pc()
        };
        assert_eq!(
            unmet(&floor(), &no_driver, true)[0].part,
            Part::DisplayDriver
        );
        // Setup installs the driver and judges it by installing.
        assert!(unmet(&floor(), &no_driver, false).is_empty());
    }

    #[test]
    fn an_unknown_build_or_requirement_is_not_met() {
        let unknown = Pc {
            windows_build: None,
            ..pc()
        };
        assert_eq!(unmet(&floor(), &unknown, true)[0].part, Part::Windows);
        let mut later = floor();
        later.insert("npu".into(), json!("something new"));
        let missing = unmet(&later, &pc(), true);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].part, Part::Unknown);
    }

    #[test]
    fn one_capable_adapter_is_enough() {
        let hybrid = Pc {
            adapters: vec![
                Adapter {
                    name: "AMD Radeon(TM) Vega 8 Graphics".into(),
                    vendor: 0x1002,
                    hevc: false,
                    hevc_error: Some("D3D12 feature level 12_2: not supported".into()),
                },
                radeon(true),
            ],
            ..pc()
        };
        assert!(unmet(&floor(), &hybrid, true).is_empty());
    }
}
