//! Desktop text added in Nightjar 7.0, one table per language in `strings_70/<code>.rs`.
//!
//! [`crate::strings`] looks a text up here as well as in its own (grandfathered) tables, so new
//! entries never grow those. Each entry is keyed by its English literal, as there.

use crate::strings::Lang;

mod de;
mod es;
mod fr;
mod it;
mod ja;
mod ko;
mod pl;
mod pt_br;
mod ru;
mod tr;
mod uk;
mod vi;
mod zh_hans;
mod zh_hant;

/// The 7.0 entries of `lang`; English has none.
pub(crate) fn table(lang: Lang) -> &'static [(&'static str, &'static str)] {
    match lang {
        Lang::En => &[],
        Lang::De => de::TABLE,
        Lang::Es => es::TABLE,
        Lang::Fr => fr::TABLE,
        Lang::It => it::TABLE,
        Lang::Ja => ja::TABLE,
        Lang::Ko => ko::TABLE,
        Lang::Pl => pl::TABLE,
        Lang::PtBr => pt_br::TABLE,
        Lang::Ru => ru::TABLE,
        Lang::Tr => tr::TABLE,
        Lang::Uk => uk::TABLE,
        Lang::Vi => vi::TABLE,
        Lang::ZhHans => zh_hans::TABLE,
        Lang::ZhHant => zh_hant::TABLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every language lists the same English keys, once each, with the same placeholders, none
    /// left in English and none already in the language's older table.
    #[test]
    fn every_language_has_every_7_0_entry_once() {
        let keys: Vec<&str> = table(Lang::De).iter().map(|e| e.0).collect();
        for lang in Lang::all().filter(|l| *l != Lang::En) {
            let entries = table(lang);
            let mut own: Vec<&str> = entries.iter().map(|e| e.0).collect();
            assert_eq!(own, keys, "{lang:?} lists other keys or another order");
            own.sort_unstable();
            own.dedup();
            assert_eq!(own.len(), keys.len(), "{lang:?} repeats a key");
            for (english, text) in entries {
                assert_ne!(english, text, "{lang:?} leaves {english:?} in English");
                assert_eq!(english.matches("{}").count(), text.matches("{}").count(), "{lang:?}: {english:?}");
                assert!(!lang.older_table_has(english), "{english:?} is already in the {lang:?} table");
            }
        }
    }

    #[test]
    fn lookups_reach_the_7_0_tables() {
        assert_eq!(crate::strings::translate(Lang::De, "Frame limiter"), "Bildratenbegrenzer");
        assert_eq!(crate::strings::translate(Lang::En, "Frame limiter"), "Frame limiter");
        assert!(Lang::De.texts().any(|t| t == "Bildratenbegrenzer"), "fonts see the 7.0 text");
    }
}
