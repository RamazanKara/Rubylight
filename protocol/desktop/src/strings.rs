//! The launcher's words. English is the source: the literal at the call site is the key, and
//! [`tr`] returns it unchanged for English. German comes from a table written by hand
//! (`strings/de.rs`). Other languages fall back to English; this is a preview, like the desktop
//! client itself.
//!
//! - `tr("Add PC")` is the text of a button, label or message.
//! - `trf("Remove {}?", &[&name])` fills `{}` (or `{0}`, `{1}` when German needs another order).
//! - `en("Offline")` marks a literal in a table or constant whose text is translated where it is
//!   used, with `tr(value)`.
//!
//! A test reads the UI sources, collects the literals inside these three calls and requires a
//! German entry for each; another one finds UI calls that still take a bare literal. Nothing here
//! allocates except `trf`, which is for text that is formatted anyway.

use std::collections::HashMap;
use std::fmt::{Display, Write};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use crate::config::Language;

mod de;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    De,
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
    if CURRENT.load(Ordering::Relaxed) == 1 { Lang::De } else { Lang::En }
}

pub fn set(lang: Lang) {
    // Tests run in parallel: each sets its own language.
    #[cfg(test)]
    TEST_LANG.with(|l| l.set(Some(lang)));
    #[cfg(not(test))]
    CURRENT.store(u8::from(lang == Lang::De), Ordering::Relaxed);
}

/// What the Language setting means on this system: Auto picks the first of the system's
/// preferred languages that Nightjar has.
pub fn resolve(setting: Language, system: &[String]) -> Lang {
    match setting {
        Language::English => Lang::En,
        Language::German => Lang::De,
        Language::System => system
            .iter()
            .find_map(|code| match code.as_str() {
                "de" => Some(Lang::De),
                "en" => Some(Lang::En),
                _ => None,
            })
            .unwrap_or(Lang::En),
    }
}

fn german() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| de::TABLE.iter().copied().collect())
}

/// `text` in `lang`: the German entry when there is one, else `text` itself.
pub fn translate(lang: Lang, text: &str) -> &str {
    match lang {
        Lang::En => text,
        Lang::De => german().get(text).copied().unwrap_or(text),
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

/// The placeholders of a template, sorted: `[0, 1]` for `"{} of {}"` and for `"{1} von {0}"`.
#[cfg(test)]
fn placeholders(template: &str) -> Vec<usize> {
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
        found.push(tail[1..end].parse::<usize>().unwrap_or_else(|_| {
            next += 1;
            next - 1
        }));
        rest = &tail[end + 1..];
    }
    found.sort_unstable();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `f` with the language set for this test only.
    pub fn with_lang<T>(lang: Lang, f: impl FnOnce() -> T) -> T {
        TEST_LANG.with(|l| l.set(Some(lang)));
        let out = f();
        TEST_LANG.with(|l| l.set(None));
        out
    }

    #[test]
    fn english_is_the_literal_and_german_is_looked_up() {
        assert_eq!(translate(Lang::En, "Add PC"), "Add PC");
        assert_eq!(translate(Lang::De, "Add PC"), "PC hinzufügen");
        assert_eq!(translate(Lang::De, "text nobody translated"), "text nobody translated");
        with_lang(Lang::De, || assert_eq!(tr("Cancel"), "Abbrechen"));
        assert_eq!(tr("Cancel"), "Cancel", "another test's language does not leak");
    }

    #[test]
    fn placeholders_are_filled_in_order_or_by_index() {
        assert_eq!(fill("{} of {}", &[&1, &"two"]), "1 of two");
        assert_eq!(fill("{1} von {0}", &[&"a", &"b"]), "b von a");
        assert_eq!(fill("{{}} {}", &[&5]), "{} 5");
        assert_eq!(fill("no holes", &[&1]), "no holes");
        assert_eq!(fill("{}", &[]), "");
        with_lang(Lang::De, || assert_eq!(trf("Remove {}?", &[&"Büro"]), "Büro entfernen?"));
    }

    #[test]
    fn the_language_setting_follows_the_system_only_on_auto() {
        let system = vec!["fr".to_owned(), "de".to_owned(), "en".to_owned()];
        assert_eq!(resolve(Language::System, &system), Lang::De, "the first language Nightjar has");
        assert_eq!(resolve(Language::System, &["fr".to_owned()]), Lang::En, "else English");
        assert_eq!(resolve(Language::System, &[]), Lang::En);
        assert_eq!(resolve(Language::English, &system), Lang::En);
        assert_eq!(resolve(Language::German, &[]), Lang::De);
    }

    // ---- The table against the sources. ----

    /// The UI sources, up to their test modules.
    const SOURCES: [(&str, &str); 10] = [
        ("ui.rs", include_str!("ui.rs")),
        ("session_ui.rs", include_str!("session_ui.rs")),
        ("session_page.rs", include_str!("session_page.rs")),
        ("main.rs", include_str!("main.rs")),
        ("console.rs", include_str!("console.rs")),
        ("hosts.rs", include_str!("hosts.rs")),
        ("config.rs", include_str!("config.rs")),
        ("shortcuts.rs", include_str!("shortcuts.rs")),
        ("radeon.rs", include_str!("radeon.rs")),
        ("gamepad.rs", include_str!("gamepad.rs")),
    ];

    /// Read for their marked texts only: the stream code keeps its own toasts and overlay lines.
    const MARKED_ONLY: [(&str, &str); 1] = [("stream/mod.rs", include_str!("stream/mod.rs"))];

    fn production(source: &str) -> &str {
        source.split("#[cfg(test)]").next().unwrap_or(source)
    }

    /// The string literal starting at `text[0] == '"'`: its value and the length it took.
    fn literal(text: &str) -> Option<(String, usize)> {
        let mut out = String::new();
        let mut chars = text.char_indices().skip(1);
        while let Some((at, c)) = chars.next() {
            match c {
                '"' => return Some((out, at + 1)),
                '\\' => match chars.next()?.1 {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '\'' => out.push('\''),
                    'u' => {
                        let _ = chars.next(); // {
                        let mut hex = String::new();
                        for (_, d) in chars.by_ref() {
                            if d == '}' {
                                break;
                            }
                            hex.push(d);
                        }
                        out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                    }
                    // A backslash at the end of a line joins it to the next, without the indent.
                    '\n' | '\r' => {
                        let mut peek = text[at + 2..].char_indices().peekable();
                        let mut skipped = 0;
                        while let Some((i, w)) = peek.peek().copied() {
                            if w.is_whitespace() {
                                skipped = i + w.len_utf8();
                                peek.next();
                            } else {
                                break;
                            }
                        }
                        for _ in 0..skipped_chars(&text[at + 2..], skipped) {
                            chars.next();
                        }
                    }
                    _ => return None,
                },
                c => out.push(c),
            }
        }
        None
    }

    fn skipped_chars(text: &str, bytes: usize) -> usize {
        text[..bytes].chars().count()
    }

    /// Helpers that translate their text arguments themselves, and how many of the arguments
    /// after the prefix are text: call sites pass the English literal.
    const HELPERS: [(&str, usize); 8] = [
        ("quiet_button(ui, ", 1),
        ("primary_button(ui, ", 1),
        ("text_button(ui, ", 1),
        ("chip(ui, ", 1),
        ("section(ui, ", 1),
        ("section_at(ui, ", 1),
        ("row(ui, ", 2),
        ("setting_row(", 2),
    ];

    /// The literals a helper call passes as its text arguments, up to `count`, in a row.
    fn helper_literals(after: &str, count: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = after;
        for _ in 0..count {
            let tail = rest.trim_start();
            if tail.starts_with('"') {
                let Some((text, used)) = literal(tail) else { break };
                out.push(text);
                rest = tail[used..].trim_start();
                match rest.strip_prefix(',') {
                    Some(next) => rest = next,
                    None => break,
                }
            } else {
                // Not a literal (a variable or an expression): skip to the next argument.
                let mut depth = 0i32;
                let mut end = None;
                for (i, c) in tail.char_indices() {
                    match c {
                        '(' | '[' | '{' => depth += 1,
                        ')' | ']' | '}' => depth -= 1,
                        ',' if depth == 0 => {
                            end = Some(i + 1);
                            break;
                        }
                        _ => {}
                    }
                    if depth < 0 {
                        break;
                    }
                }
                match end {
                    Some(end) => rest = &tail[end..],
                    None => break,
                }
            }
        }
        out
    }

    /// Every literal inside `tr(...)`, `trf(...)` and `en(...)`, and the text arguments of the
    /// translating helpers, with the file and line.
    fn marked() -> Vec<(String, String, usize)> {
        let mut found = Vec::new();
        for (file, source) in SOURCES.into_iter().chain(MARKED_ONLY) {
            let code = production(source);
            for call in ["tr(", "trf(", "en("] {
                let mut from = 0;
                while let Some(at) = code[from..].find(call) {
                    let start = from + at;
                    from = start + call.len();
                    let before = code[..start].chars().next_back();
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                        continue;
                    }
                    let tail = code[from..].trim_start();
                    if tail.starts_with('"') {
                        if let Some((text, _)) = literal(tail) {
                            let line = code[..start].matches('\n').count() + 1;
                            found.push((file.to_owned(), text, line));
                        }
                    }
                }
            }
            for (prefix, count) in HELPERS {
                let mut from = 0;
                while let Some(at) = code[from..].find(prefix) {
                    let start = from + at;
                    from = start + prefix.len();
                    let before = code[..start].chars().next_back();
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                        continue;
                    }
                    // `setting_row(ui, s, layer, &["key"], "Label", "Description", ...)`
                    let args = if prefix == "setting_row(" {
                        match code[from..].find("], ") {
                            Some(end) if end < 200 => &code[from + end + 3..],
                            _ => continue,
                        }
                    } else {
                        &code[from..]
                    };
                    let line = code[..start].matches('\n').count() + 1;
                    for text in helper_literals(args, count) {
                        found.push((file.to_owned(), text, line));
                    }
                }
            }
        }
        found
    }

    #[test]
    fn literal_reader_handles_escapes_and_line_joins() {
        assert_eq!(literal("\"a\\nb\" rest").unwrap().0, "a\nb");
        assert_eq!(literal("\"say \\\"hi\\\"\"").unwrap().0, "say \"hi\"");
        assert_eq!(literal("\"one \\\n      two\"").unwrap().0, "one two");
        assert_eq!(literal("\"\\u{b7}\"").unwrap().0, "\u{b7}");
    }

    #[test]
    fn every_marked_text_has_a_german_entry() {
        let table = german();
        let missing: Vec<String> = marked()
            .into_iter()
            .filter(|(_, text, _)| !table.contains_key(text.as_str()))
            .map(|(file, text, line)| format!("{file}:{line}: {text:?}"))
            .collect();
        assert!(missing.is_empty(), "no German for:\n{}", missing.join("\n"));
    }

    #[test]
    fn the_german_table_is_clean() {
        let mut seen = std::collections::HashSet::new();
        for (english, german) in de::TABLE {
            assert!(seen.insert(*english), "{english:?} is in the table twice");
            assert!(!english.trim().is_empty() && !german.trim().is_empty(), "{english:?} has an empty side");
            assert_eq!(placeholders(english), placeholders(german), "{{}} differ in {english:?}");
            assert_eq!(english.starts_with(' '), german.starts_with(' '), "leading space differs in {english:?}");
            assert_eq!(english.ends_with('\n'), german.ends_with('\n'), "trailing newline differs in {english:?}");
            assert_eq!(english.matches('\n').count(), german.matches('\n').count(), "line breaks differ in {english:?}");
        }
        let used: std::collections::HashSet<String> = marked().into_iter().map(|(_, text, _)| text).collect();
        let unused: Vec<&str> = de::TABLE.iter().map(|(e, _)| *e).filter(|e| !used.contains(*e)).collect();
        assert!(unused.is_empty(), "German entries no source uses (renamed English text?):\n{}", unused.join("\n"));
    }

    /// Calls that draw text. A bare literal as their first text argument is untranslated.
    const TEXT_CALLS: [&str; 15] = [
        "RichText::new(\"",
        "Button::new(\"",
        "Checkbox::new(&mut",
        ".label(\"",
        ".weak(\"",
        ".heading(\"",
        ".hint_text(\"",
        ".on_hover_text(\"",
        ".selected_text(\"",
        ".button(\"",
        ".menu_button(\"",
        ".collapsing(\"",
        "Window::new(\"",
        ".message(\"",
        "show_toast(\"",
    ];

    /// Literals that are the same in every language: names, key chords, codec names.
    const SAME_EVERYWHERE: [&str; 9] = [
        "Nightjar",
        "CTRL + ALT + DELETE",
        "H.264",
        "HEVC",
        "AV1",
        "PyroWave 4:2:0",
        "PyroWave 4:4:4",
        "English",
        "Deutsch",
    ];

    /// Whether a bare literal needs no translation: a symbol, an example, or in the list above.
    fn same_everywhere(text: &str) -> bool {
        SAME_EVERYWHERE.contains(&text) || text.chars().filter(|c| c.is_alphabetic()).count() < 2
    }

    /// The bare literal a UI call starts at `text`, which begins with the quote.
    fn bare(text: &str) -> Option<String> {
        literal(text).map(|(t, _)| t).filter(|t| !same_everywhere(t))
    }

    #[test]
    fn no_text_is_left_in_the_ui_code() {
        let mut found = Vec::new();
        for (file, source) in SOURCES {
            for (n, line) in production(source).lines().enumerate() {
                let code = line.split("//").next().unwrap_or(line);
                let mut flag = |text: &str| {
                    if bare(text).is_some() {
                        found.push(format!("{file}:{}: {}", n + 1, line.trim()));
                    }
                };
                for call in TEXT_CALLS {
                    // `Checkbox::new(&mut x, "text")`: a literal as the last argument.
                    if call == "Checkbox::new(&mut" {
                        if let (Some(_), Some(at)) = (code.find(call), code.rfind(", \"")) {
                            flag(&code[at + 2..]);
                        }
                    } else if let Some(at) = code.find(call) {
                        flag(&code[at + call.len() - 1..]);
                    }
                }
                // `selectable_value(&mut v, VALUE, "text")` and `.checkbox(&mut v, "text")`.
                for call in ["selectable_value(", "selectable_label(", ".checkbox(", ".message("] {
                    if let Some(at) = code.find(call) {
                        if let Some(last) = code[at..].rfind(", \"") {
                            flag(&code[at + last + 2..]);
                        }
                    }
                }
            }
        }
        found.sort();
        found.dedup();
        assert!(found.is_empty(), "text without tr():
{}", found.join("
"));
    }
}
