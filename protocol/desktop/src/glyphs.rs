//! Button names in hints follow the connected controller's layout: Xbox by default, PlayStation
//! symbols or Switch labels when such a controller is attached. Ported from Moonlight V+
//! (streaming/input/gamepadglyphs.h and SdlGamepadKeyNavigation::detectUiStyle).

use sdl3_sys::everything::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Xbox,
    PlayStation,
    Nintendo,
}

/// A face button by its position, as SDL names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    South,
    East,
    West,
    North,
}

impl Style {
    /// The first PlayStation or Switch controller sets the style; anything else reads as Xbox.
    pub fn of_types(types: impl IntoIterator<Item = SDL_GamepadType>) -> Style {
        types
            .into_iter()
            .find_map(|kind| match kind {
                SDL_GAMEPAD_TYPE_PS3 | SDL_GAMEPAD_TYPE_PS4 | SDL_GAMEPAD_TYPE_PS5 => Some(Style::PlayStation),
                SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_PRO
                | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_LEFT
                | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_RIGHT
                | SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_PAIR => Some(Style::Nintendo),
                _ => None,
            })
            .unwrap_or(Style::Xbox)
    }

    /// The style of the controllers attached now.
    pub fn of(pads: &crate::gamepad::Gamepads) -> Style {
        // SAFETY: gamepads the list opened and still holds.
        Style::of_types(pads.pads.iter().map(|pad| unsafe { SDL_GetGamepadType(pad.handle) }))
    }

    /// The label on the face button at `face`: Switch labels follow its layout (south is B).
    pub fn face(self, face: Face) -> &'static str {
        match (self, face) {
            // V+ shows ✕ (U+2715); no font here has it, × (U+00D7) is the same mark.
            (Style::PlayStation, Face::South) => "×",
            (Style::PlayStation, Face::East) => "○",
            (Style::PlayStation, Face::West) => "□",
            (Style::PlayStation, Face::North) => "△",
            (Style::Nintendo, Face::South) => "B",
            (Style::Nintendo, Face::East) => "A",
            (Style::Nintendo, Face::West) => "Y",
            (Style::Nintendo, Face::North) => "X",
            (Style::Xbox, Face::South) => "A",
            (Style::Xbox, Face::East) => "B",
            (Style::Xbox, Face::West) => "X",
            (Style::Xbox, Face::North) => "Y",
        }
    }

    pub fn start(self) -> &'static str {
        match self {
            Style::PlayStation => "Options",
            Style::Nintendo => "+",
            Style::Xbox => "Start",
        }
    }

    pub fn select(self) -> &'static str {
        match self {
            Style::PlayStation => "Share",
            Style::Nintendo => "−",
            Style::Xbox => "Select",
        }
    }

    pub fn left_shoulder(self) -> &'static str {
        match self {
            Style::PlayStation => "L1",
            Style::Nintendo => "L",
            Style::Xbox => "LB",
        }
    }

    pub fn right_shoulder(self) -> &'static str {
        match self {
            Style::PlayStation => "R1",
            Style::Nintendo => "R",
            Style::Xbox => "RB",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_playstation_or_switch_controller_sets_the_style() {
        assert_eq!(Style::of_types([]), Style::Xbox);
        assert_eq!(Style::of_types([SDL_GAMEPAD_TYPE_XBOXONE, SDL_GAMEPAD_TYPE_STANDARD]), Style::Xbox);
        assert_eq!(Style::of_types([SDL_GAMEPAD_TYPE_XBOX360, SDL_GAMEPAD_TYPE_PS5]), Style::PlayStation);
        assert_eq!(Style::of_types([SDL_GAMEPAD_TYPE_PS4]), Style::PlayStation);
        assert_eq!(Style::of_types([SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_JOYCON_PAIR, SDL_GAMEPAD_TYPE_PS3]), Style::Nintendo);
        assert_eq!(Style::of_types([SDL_GAMEPAD_TYPE_NINTENDO_SWITCH_PRO]), Style::Nintendo);
    }

    #[test]
    fn labels_follow_each_layout() {
        let west = [Style::Xbox, Style::PlayStation, Style::Nintendo].map(|s| s.face(Face::West));
        assert_eq!(west, ["X", "□", "Y"]);
        let south = [Style::Xbox, Style::PlayStation, Style::Nintendo].map(|s| s.face(Face::South));
        assert_eq!(south, ["A", "×", "B"]);
        assert_eq!(Style::PlayStation.face(Face::East), "○");
        assert_eq!(Style::PlayStation.face(Face::North), "△");
        assert_eq!((Style::Nintendo.start(), Style::Nintendo.select()), ("+", "−"));
        assert_eq!((Style::PlayStation.left_shoulder(), Style::PlayStation.right_shoulder()), ("L1", "R1"));
    }

    #[test]
    fn the_fonts_draw_every_label() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        // The fonts load with the first frame; its textures are let go like the other tests do.
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        for style in [Style::Xbox, Style::PlayStation, Style::Nintendo] {
            let labels = [Face::South, Face::East, Face::West, Face::North].map(|f| style.face(f)).join(" ")
                + &[style.start(), style.select(), style.left_shoulder(), style.right_shoulder()].join(" ");
            for font in [crate::theme::regular(12.0), crate::theme::semibold(12.0)] {
                let missing: String = labels.chars().filter(|c| !ctx.fonts_mut(|f| f.has_glyph(&font, *c))).collect();
                assert!(missing.is_empty(), "{missing:?} of {labels:?} missing in {font:?}");
            }
        }
    }
}
