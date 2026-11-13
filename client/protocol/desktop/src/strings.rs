//! The launcher's words. English is the source: the literal at the call site is the key, and
//! [`tr`] returns it unchanged for English. Every other language has a table keyed by the English
//! literal (`strings/<code>.rs`), worded like the Android app's translations of the same text.
//!
//! - `tr("Add PC")` is the text of a button, label or message.
//! - `trf("Remove {}?", &[&name])` fills `{}` (or `{0}`, `{1}` when a language needs another order).
//! - `en("Offline")` marks a literal in a table or constant whose text is translated where it is
//!   used, with `tr(value)`.
//!
//! A test reads the UI sources, collects the literals inside these three calls and requires an
//! entry for each in every table; another one finds UI calls that still take a bare literal.
//! Nothing here allocates except `trf`, which is for text that is formatted anyway.

use std::collections::HashMap;
use std::fmt::{Display, Write};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use crate::config::Language;

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

/// A language Nightjar has. The order is [`LANGS`]'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    En,
    De,
    Es,
    Fr,
    It,
    Ja,
    Ko,
    Pl,
    PtBr,
    Ru,
    Tr,
    Uk,
    Vi,
    ZhHans,
    ZhHant,
}

/// Each language: the system language code it answers to, its own name and its table.
/// A language's entries: (English, its text).
type Table = &'static [(&'static str, &'static str)];

const LANGS: [(Lang, &str, &str, Table); 15] = [
    (Lang::En, "en", "English", &[]),
    (Lang::De, "de", "Deutsch", de::TABLE),
    (Lang::Es, "es", "Español", es::TABLE),
    (Lang::Fr, "fr", "Français", fr::TABLE),
    (Lang::It, "it", "Italiano", it::TABLE),
    (Lang::Ja, "ja", "日本語", ja::TABLE),
    (Lang::Ko, "ko", "한국어", ko::TABLE),
    (Lang::Pl, "pl", "Polski", pl::TABLE),
    (Lang::PtBr, "pt", "Português (Brasil)", pt_br::TABLE),
    (Lang::Ru, "ru", "Русский", ru::TABLE),
    (Lang::Tr, "tr", "Türkçe", tr::TABLE),
    (Lang::Uk, "uk", "Українська", uk::TABLE),
    (Lang::Vi, "vi", "Tiếng Việt", vi::TABLE),
    (Lang::ZhHans, "zh-hans", "简体中文", zh_hans::TABLE),
    (Lang::ZhHant, "zh-hant", "繁體中文", zh_hant::TABLE),
];

impl Lang {
    fn index(self) -> usize {
        LANGS.iter().position(|l| l.0 == self).unwrap_or(0)
    }

    /// Its name in itself ("Deutsch").
    pub fn name(self) -> &'static str {
        LANGS[self.index()].2
    }

    /// Its English name, for a list whose fonts cannot draw [`Lang::name`] (Japanese in a
    /// Russian menu: each language loads only its own system font).
    pub fn english_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::De => "German",
            Lang::Es => "Spanish",
            Lang::Fr => "French",
            Lang::It => "Italian",
            Lang::Ja => "Japanese",
            Lang::Ko => "Korean",
            Lang::Pl => "Polish",
            Lang::PtBr => "Portuguese (Brazil)",
            Lang::Ru => "Russian",
            Lang::Tr => "Turkish",
            Lang::Uk => "Ukrainian",
            Lang::Vi => "Vietnamese",
            Lang::ZhHans => "Chinese (Simplified)",
            Lang::ZhHant => "Chinese (Traditional)",
        }
    }

    /// Whether numbers take a decimal comma ("1,5 ms"). English, Japanese, Korean and Chinese
    /// write a point.
    pub fn decimal_comma(self) -> bool {
        !matches!(self, Lang::En | Lang::Ja | Lang::Ko | Lang::ZhHans | Lang::ZhHant)
    }

    /// Every language, in the settings' order.
    #[cfg(test)]
    pub fn all() -> impl Iterator<Item = Lang> {
        LANGS.iter().map(|l| l.0)
    }

    /// The text of its table: what the fonts have to draw.
    #[cfg(test)]
    pub fn texts(self) -> impl Iterator<Item = &'static str> {
        self.table().iter().map(|e| e.1)
    }

    fn table(self) -> Table {
        LANGS[self.index()].3
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

#[cfg(test)]
thread_local! {
    /// A test's own language, so tests that look at German do not change English ones.
    static TEST_LANG: std::cell::Cell<Option<Lang>> = const { std::cell::Cell::new(None) };
}

/// The language in use.
pub fn current() -> Lang {
    #[cfg(test)]
    if let Some(lang) = TEST_LANG.with(std::cell::Cell::get) {
        return lang;
    }
    LANGS.get(usize::from(CURRENT.load(Ordering::Relaxed))).map_or(Lang::En, |l| l.0)
}

pub fn set(lang: Lang) {
    // Tests run in parallel: each sets its own language.
    #[cfg(test)]
    TEST_LANG.with(|l| l.set(Some(lang)));
    #[cfg(not(test))]
    CURRENT.store(u8::try_from(lang.index()).unwrap_or(0), Ordering::Relaxed);
}

/// The language a system locale code names: "de", "de-at", "pt-br", "zh-tw". Chinese is
/// Traditional for Taiwan, Hong Kong and Macao (or a "hant" script), else Simplified.
pub fn from_code(code: &str) -> Option<Lang> {
    let code = code.to_ascii_lowercase().replace('_', "-");
    let mut parts = code.split('-');
    let language = parts.next()?;
    if language == "zh" {
        // A script ("zh-Hans-HK") says it; else the region does.
        let rest: Vec<&str> = parts.collect();
        let traditional = rest.contains(&"hant") || (!rest.contains(&"hans") && rest.iter().any(|p| matches!(*p, "tw" | "hk" | "mo")));
        let code = if traditional { "zh-hant" } else { "zh-hans" };
        return LANGS.iter().find(|l| l.1 == code).map(|l| l.0);
    }
    LANGS.iter().find(|l| l.1 == language).map(|l| l.0)
}

/// What the Language setting means on this system: Auto picks the first of the system's
/// preferred languages that Nightjar has.
pub fn resolve(setting: Language, system: &[String]) -> Lang {
    chosen(setting).unwrap_or_else(|| system.iter().find_map(|code| from_code(code)).unwrap_or(Lang::En))
}

/// The language a setting names, or None for System default.
pub fn chosen(setting: Language) -> Option<Lang> {
    match setting {
        Language::System => None,
        Language::English => Some(Lang::En),
        Language::German => Some(Lang::De),
        Language::Spanish => Some(Lang::Es),
        Language::French => Some(Lang::Fr),
        Language::Italian => Some(Lang::It),
        Language::Japanese => Some(Lang::Ja),
        Language::Korean => Some(Lang::Ko),
        Language::Polish => Some(Lang::Pl),
        Language::PortugueseBrazil => Some(Lang::PtBr),
        Language::Russian => Some(Lang::Ru),
        Language::Turkish => Some(Lang::Tr),
        Language::Ukrainian => Some(Lang::Uk),
        Language::Vietnamese => Some(Lang::Vi),
        Language::ChineseSimplified => Some(Lang::ZhHans),
        Language::ChineseTraditional => Some(Lang::ZhHant),
    }
}

fn lookup(lang: Lang) -> &'static HashMap<&'static str, &'static str> {
    static TABLES: [OnceLock<HashMap<&'static str, &'static str>>; LANGS.len()] = [const { OnceLock::new() }; LANGS.len()];
    TABLES[lang.index()].get_or_init(|| lang.table().iter().copied().collect())
}

/// `text` in `lang`: its entry when there is one, else `text` itself.
pub fn translate(lang: Lang, text: &str) -> &str {
    match lang {
        Lang::En => text,
        lang => lookup(lang).get(text).copied().unwrap_or(text),
    }
}

/// `text` in the language in use.
pub fn tr(text: &str) -> &str {
    translate(current(), text)
}

/// Marks a literal that is translated where it is used (`tr(value)`).
pub const fn en(text: &'static str) -> &'static str {
    text
}

/// `template` in the language in use, with `args` in place of its `{}` and `{0}`, `{1}`.
pub fn trf(template: &str, args: &[&dyn Display]) -> String {
    fill(tr(template), args)
}

/// Fills `{}` (the next argument) and `{n}` (argument `n`); `{{` and `}}` are braces.
pub fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut next = 0;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut digits = String::new();
                for d in chars.by_ref() {
                    if d == '}' {
                        break;
                    }
                    digits.push(d);
                }
                let index = digits.parse::<usize>().unwrap_or_else(|_| {
                    next += 1;
                    next - 1
                });
                if let Some(arg) = args.get(index) {
                    let _ = write!(out, "{arg}");
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// The placeholders of a template in the order they appear, each with the argument it shows
/// and whether it names it (`{1}`) rather than taking the next one (`{}`).
#[cfg(test)]
fn placeholders(template: &str) -> Vec<(usize, bool)> {
    let mut found = Vec::new();
    let mut next = 0;
    let mut rest = template;
    while let Some(at) = rest.find(['{', '}']) {
        let tail = &rest[at..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            rest = &tail[2..];
            continue;
        }
        let Some(end) = tail.find('}') else { break };
        found.push(match tail[1..end].parse::<usize>() {
            Ok(index) => (index, true),
            Err(_) => {
                next += 1;
                (next - 1, false)
            }
        });
        rest = &tail[end + 1..];
    }
    found
}

#[cfg(test)]
pub(crate) mod tests;
