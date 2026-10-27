//! The layout the host turns a device's keys into scan codes with (`keyboard_layout`,
//! control message 0x553E, Experimental).
//!
//! Moonlight marks each key either normalized (the virtual-key code a US keyboard sends
//! for that key's position) or not (the code of the device's own layout). Normalized keys
//! keep the fixed US table. A key that is not normalized is mapped with the device's
//! layout: the `keyboard_layout` setting when it names one, else the layout the device
//! reports, else the host's own as before. A German device's Z key then reaches games as
//! the scan code of the key it sits on, and a German PC types the z it shows.
pub use rubylight_protocol::keyboard_layout::{
    KEYBOARD_LAYOUT_MESSAGE_TYPE, KeyboardLayout, LayoutSource,
};

use crate::config::KeyboardLayoutSetting;

/// US English, `00000409`.
pub const US: u32 = 0x0000_0409;

/// The Windows layout each BCP 47 language and region usually types with.
const BCP47: &[(&str, u32)] = &[
    ("en-gb", 0x0809),
    ("en-ie", 0x1809),
    ("de-ch", 0x0807),
    ("de-li", 0x0807),
    ("fr-ch", 0x100C),
    ("fr-be", 0x080C),
    ("fr-ca", 0x1009),
    ("nl-be", 0x0813),
    ("pt-br", 0x0416),
    ("es-es", 0x040A),
    ("en", US),
    ("de", 0x0407),
    ("fr", 0x040C),
    // Spanish outside Spain types on the Latin American layout.
    ("es", 0x080A),
    ("it", 0x0410),
    ("pt", 0x0816),
    ("nl", 0x0413),
    ("sv", 0x041D),
    ("nb", 0x0414),
    ("nn", 0x0414),
    ("no", 0x0414),
    ("da", 0x0406),
    ("fi", 0x040B),
    ("pl", 0x0001_0415),
    ("cs", 0x0405),
    ("sk", 0x041B),
    ("hu", 0x040E),
    ("ru", 0x0419),
    ("uk", 0x0422),
    ("tr", 0x041F),
    ("el", 0x0408),
    ("ja", 0x0411),
    ("ko", 0x0412),
];

/// The Windows layout of each XKB layout name.
const XKB: &[(&str, u32)] = &[
    ("ch(fr)", 0x100C),
    ("ca(fr)", 0x1009),
    ("us", US),
    ("gb", 0x0809),
    ("ie", 0x1809),
    ("de", 0x0407),
    ("at", 0x0407),
    ("ch", 0x0807),
    ("fr", 0x040C),
    ("be", 0x080C),
    ("it", 0x0410),
    ("es", 0x040A),
    ("latam", 0x080A),
    ("pt", 0x0816),
    ("br", 0x0416),
    ("nl", 0x0413),
    ("se", 0x041D),
    ("no", 0x0414),
    ("dk", 0x0406),
    ("fi", 0x040B),
    ("pl", 0x0001_0415),
    ("cz", 0x0405),
    ("sk", 0x041B),
    ("hu", 0x040E),
    ("ru", 0x0419),
    ("ua", 0x0422),
    ("tr", 0x041F),
    ("gr", 0x0408),
    ("jp", 0x0411),
    ("kr", 0x0412),
];

/// The Windows layout id (KLID) a device's report names: its KLID, or the usual Windows
/// layout of its BCP 47 or XKB tag. `None` for an unknown source or a tag this host
/// does not know.
pub fn reported_klid(layout: &KeyboardLayout) -> Option<u32> {
    let tag = layout.tag.to_ascii_lowercase().replace('_', "-");
    match layout.source {
        LayoutSource::Klid => (layout.klid != 0).then_some(layout.klid),
        LayoutSource::Bcp47 => {
            // `de-DE-x-…` and `zh-Hant-TW` reduce to language and region, then language.
            let mut parts = tag.split('-');
            let language = parts.next().unwrap_or_default();
            let region = parts.find(|part| part.len() == 2 || part == &"419");
            let region = region.map(|region| format!("{language}-{region}"));
            region
                .as_deref()
                .and_then(|tag| lookup(BCP47, tag))
                .or_else(|| lookup(BCP47, language))
        }
        LayoutSource::Xkb => lookup(XKB, &tag).or_else(|| {
            // `de(nodeadkeys)` types like `de`.
            let base = tag.split('(').next().unwrap_or_default();
            lookup(XKB, base)
        }),
        LayoutSource::Unknown => None,
    }
}

fn lookup(table: &[(&str, u32)], tag: &str) -> Option<u32> {
    table
        .iter()
        .find(|(name, _)| *name == tag)
        .map(|(_, klid)| *klid)
}

/// The layout the host maps a device's keys that are not normalized with: the
/// `keyboard_layout` setting when it names one, else the device's report. `None` keeps the
/// host's own layout, as before 3.0.
pub fn effective(setting: KeyboardLayoutSetting, reported: Option<&KeyboardLayout>) -> Option<u32> {
    match setting {
        KeyboardLayoutSetting::Us => Some(US),
        KeyboardLayoutSetting::Klid(klid) => Some(klid),
        KeyboardLayoutSetting::Client => reported.and_then(reported_klid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(source: LayoutSource, klid: u32, tag: &str) -> KeyboardLayout {
        KeyboardLayout {
            source,
            klid,
            tag: tag.into(),
        }
    }

    #[test]
    fn reports_name_windows_layouts() {
        use LayoutSource::*;
        for (layout, klid) in [
            (report(Klid, 0x0407, "de-DE"), Some(0x0407)),
            (report(Klid, 0, "de-DE"), None),
            (report(Bcp47, 0, "de-DE"), Some(0x0407)),
            (report(Bcp47, 0, "de-AT"), Some(0x0407)),
            (report(Bcp47, 0, "de-CH"), Some(0x0807)),
            (report(Bcp47, 0, "de"), Some(0x0407)),
            (report(Bcp47, 0, "fr-FR"), Some(0x040C)),
            (report(Bcp47, 0, "fr_BE"), Some(0x080C)),
            (report(Bcp47, 0, "en-GB"), Some(0x0809)),
            (report(Bcp47, 0, "en-US"), Some(US)),
            (report(Bcp47, 0, "en-AU"), Some(US)),
            (report(Bcp47, 0, "es-419"), Some(0x080A)),
            (report(Bcp47, 0, "es-ES"), Some(0x040A)),
            (report(Bcp47, 0, "sr-Latn-RS"), None),
            (report(Bcp47, 0, "xx"), None),
            (report(Xkb, 0, "de(nodeadkeys)"), Some(0x0407)),
            (report(Xkb, 0, "ch(fr)"), Some(0x100C)),
            (report(Xkb, 0, "gb"), Some(0x0809)),
            (report(Xkb, 0, "fr"), Some(0x040C)),
            (report(Xkb, 0, "zz"), None),
            (report(Unknown, 0x0407, "de"), None),
        ] {
            assert_eq!(reported_klid(&layout), klid, "{layout:?}");
        }
    }

    #[test]
    fn the_setting_wins_over_the_report() {
        let german = report(LayoutSource::Klid, 0x0407, "de-DE");
        assert_eq!(
            effective(KeyboardLayoutSetting::Client, Some(&german)),
            Some(0x0407)
        );
        assert_eq!(effective(KeyboardLayoutSetting::Client, None), None);
        assert_eq!(
            effective(KeyboardLayoutSetting::Us, Some(&german)),
            Some(US)
        );
        assert_eq!(
            effective(KeyboardLayoutSetting::Klid(0x0809), Some(&german)),
            Some(0x0809)
        );
        assert_eq!(
            effective(KeyboardLayoutSetting::Klid(0x040C), None),
            Some(0x040C)
        );
    }
}
