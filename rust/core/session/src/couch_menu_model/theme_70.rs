//! The couch menu's Rubylight 7.0 colours ("Studio Console Cards"): the
//! single home of every colour the panel draws. They mirror the dark tokens
//! of `rust/web/src/styles/tokens.css`, and a test keeps the two equal.

/// An opaque sRGB colour.
pub type Rgb = [u8; 3];

const fn hex(value: u32) -> Rgb {
    [(value >> 16) as u8, (value >> 8) as u8, value as u8]
}

/// The container behind everything.
pub const STAGE: Rgb = hex(0x0d0f14);
/// A row card.
pub const CARD: Rgb = hex(0x161922);
/// The container border and the rule under the header (`--media-line`).
pub const HAIRLINE: Rgb = hex(0x2b3140);
/// A slider's rail and an off switch's track.
pub const TRACK: Rgb = hex(0x2b3140);
/// Labels, the title and messages (`--ink`).
pub const INK: Rgb = hex(0xf5f7fa);
/// Values (`--ink-2`).
pub const INK_2: Rgb = hex(0xc8cedb);
/// Section labels, notes, the hint and locked rows (`--muted`).
pub const MUTED: Rgb = hex(0x9aa3b5);
/// The focused row's card (`--accent`).
pub const FOCUS: Rgb = hex(0xce1e49);
/// Controls on the focused card (`--accent-strong`).
pub const FOCUS_STRONG: Rgb = hex(0xab153b);
/// Text on the focused card.
pub const FOCUS_INK: Rgb = hex(0xffffff);
/// The header badge (`--brand`).
pub const BRAND: Rgb = hex(0xc41242);
/// An on switch (`--ok`).
pub const OK: Rgb = hex(0x45e0a0);
/// `--warn`, for parity with the console; no state of the menu uses it yet.
pub const WARN: Rgb = hex(0xf5b041);
/// A failure message (`--danger`).
pub const ERROR: Rgb = hex(0xff7070);

#[cfg(test)]
mod tests {
    use super::*;

    const TOKENS: &str = include_str!("../../../../web/src/styles/tokens.css");

    fn luminance(colour: Rgb) -> f64 {
        let [r, g, b] = colour.map(|c| {
            let c = f64::from(c) / 255.;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        });
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    fn contrast(a: Rgb, b: Rgb) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// The value of `--name` in the block that starts with `selector`.
    fn token(selector: &str, name: &str) -> Rgb {
        let start = TOKENS.find(selector).expect(selector);
        let block = &TOKENS[start..start + TOKENS[start..].find('}').unwrap()];
        let key = format!("--{name}:");
        let at = block.find(&key).unwrap_or_else(|| panic!("{name}")) + key.len();
        let value = block[at..].trim_start();
        let digits = value.strip_prefix('#').unwrap();
        hex(u32::from_str_radix(&digits[..6], 16).unwrap())
    }

    #[test]
    fn couch_menu_theme_70_matches_the_console_tokens() {
        let dark = ":root[data-theme='dark'] {";
        for (name, colour) in [
            ("accent", FOCUS),
            ("accent-strong", FOCUS_STRONG),
            ("ink", INK),
            ("ink-2", INK_2),
            ("muted", MUTED),
            ("ok", OK),
            ("warn", WARN),
            ("danger", ERROR),
            ("brand", BRAND),
        ] {
            assert_eq!(token(dark, name), colour, "--{name}");
        }
        assert_eq!(token(":root {", "media-line"), HAIRLINE);
    }

    #[test]
    fn couch_menu_theme_70_meets_wcag_contrast() {
        for ground in [STAGE, CARD] {
            for (name, ink) in [
                ("ink", INK),
                ("ink-2", INK_2),
                ("muted", MUTED),
                ("error", ERROR),
            ] {
                let ratio = contrast(ink, ground);
                assert!(ratio >= 4.5, "{name} on {ground:?}: {ratio:.2}");
            }
            for (name, graphic) in [
                ("ok", OK),
                ("warn", WARN),
                ("focus", FOCUS),
                ("muted", MUTED),
            ] {
                let ratio = contrast(graphic, ground);
                assert!(ratio >= 3.0, "{name} on {ground:?}: {ratio:.2}");
            }
        }
        let ratio = contrast(FOCUS_INK, FOCUS);
        assert!(ratio >= 4.5, "focus ink: {ratio:.2}");
        // The focused card's controls: a white knob on the strong rail.
        assert!(contrast(FOCUS_INK, FOCUS_STRONG) >= 3.0);
    }
}
