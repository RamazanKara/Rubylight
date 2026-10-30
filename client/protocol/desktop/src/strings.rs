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
pub(crate) mod tests {
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
    const SOURCES: [(&str, &str); 17] = [
        ("ui.rs", include_str!("ui.rs")),
        ("ui/files.rs", include_str!("ui/files.rs")),
        ("host_page.rs", include_str!("host_page.rs")),
        ("folders_page.rs", include_str!("folders_page.rs")),
        ("coop_page.rs", include_str!("coop_page.rs")),
        ("session_ui.rs", include_str!("session_ui.rs")),
        ("session_page.rs", include_str!("session_page.rs")),
        ("main.rs", include_str!("main.rs")),
        ("console.rs", include_str!("console.rs")),
        ("hosts.rs", include_str!("hosts.rs")),
        ("config.rs", include_str!("config.rs")),
        ("shortcuts.rs", include_str!("shortcuts.rs")),
        ("radeon.rs", include_str!("radeon.rs")),
        ("gamepad.rs", include_str!("gamepad.rs")),
        ("usb.rs", include_str!("usb.rs")),
        ("usb_linux.rs", include_str!("usb_linux.rs")),
        ("stream/doctor.rs", include_str!("stream/doctor.rs")),
        ("update_check.rs", include_str!("update_check.rs")),
    ];

    /// Read for its marked texts and its toasts only: the stream code keeps its own overlay lines.
    const MARKED_ONLY: [(&str, &str); 4] = [
        ("stream/mod.rs", include_str!("stream/mod.rs")),
        ("stream/menu_extras.rs", include_str!("stream/menu_extras.rs")),
        ("stream/rate_change.rs", include_str!("stream/rate_change.rs")),
        ("stream/clipboard_sync.rs", include_str!("stream/clipboard_sync.rs")),
    ];

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

    /// The length of the char literal at the start of `text` (which begins with `'`), or None
    /// for a lifetime.
    fn char_literal_len(text: &str) -> Option<usize> {
        let mut chars = text.chars();
        chars.next();
        match chars.next()? {
            '\\' => text.get(3..)?.find('\'').map(|end| end + 4),
            c => text.get(1 + c.len_utf8()..)?.starts_with('\'').then_some(c.len_utf8() + 2),
        }
    }

    /// A source without its comments (blanked, so offsets and lines stay), and every string
    /// literal in it with the offset of its opening quote.
    struct Lexed {
        code: String,
        literals: Vec<(usize, String)>,
    }

    fn lex(source: &str) -> Lexed {
        let text = production(source);
        let mut code = String::with_capacity(text.len());
        let mut literals = Vec::new();
        let mut i = 0;
        while i < text.len() {
            let rest = &text[i..];
            let Some(c) = rest.chars().next() else { break };
            if rest.starts_with("//") {
                let end = rest.find('\n').unwrap_or(rest.len());
                code.push_str(&" ".repeat(end));
                i += end;
            } else if rest.starts_with("/*") {
                let end = rest.find("*/").map_or(rest.len(), |e| e + 2);
                for ch in rest[..end].chars() {
                    if ch == '\n' {
                        code.push('\n');
                    } else {
                        code.push_str(&" ".repeat(ch.len_utf8()));
                    }
                }
                i += end;
            } else if c == '"' {
                let used = match literal(rest) {
                    Some((value, used)) => {
                        literals.push((i, value));
                        used
                    }
                    None => 1,
                };
                code.push_str(&rest[..used]);
                i += used;
            } else if c == '\'' {
                let used = char_literal_len(rest).unwrap_or(1);
                code.push_str(&rest[..used]);
                i += used;
            } else {
                code.push(c);
                i += c.len_utf8();
            }
        }
        Lexed { code, literals }
    }

    /// The arguments of a call: `after` is the text right after its opening parenthesis.
    fn split_args(after: &str) -> Vec<&str> {
        let mut args = Vec::new();
        let (mut depth, mut start, mut i) = (0i32, 0, 0);
        while i < after.len() {
            let rest = &after[i..];
            let Some(c) = rest.chars().next() else { break };
            match c {
                '"' => {
                    i += literal(rest).map_or(1, |(_, used)| used);
                    continue;
                }
                '\'' => {
                    i += char_literal_len(rest).unwrap_or(1);
                    continue;
                }
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                ',' if depth == 0 => {
                    args.push(&after[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
            i += c.len_utf8();
        }
        if !after[start..i.min(after.len())].trim().is_empty() {
            args.push(&after[start..i.min(after.len())]);
        }
        args
    }

    /// Helpers that translate their text arguments themselves, and how many of the arguments
    /// after the prefix are text: call sites pass the English literal.
    const HELPERS: [(&str, usize); 10] = [
        ("quiet_button(ui, ", 1),
        ("rows.section(ui, ", 1),
        ("rows.group(ui, ", 1),
        ("primary_button(ui, ", 1),
        ("text_button(ui, ", 1),
        ("chip(ui, ", 1),
        ("section(ui, ", 1),
        ("section_at(ui, ", 1),
        ("row(ui, ", 2),
        ("setting_row(", 2),
    ];

    /// The text arguments of the helper call that `prefix` starts at `start` in `code`.
    fn helper_args<'a>(code: &'a str, prefix: &str, start: usize) -> Vec<&'a str> {
        let from = start + prefix.len();
        // `setting_row(ui, s, layer, &["key"], "Label", "Description", ...)`
        let args = if prefix == "setting_row(" {
            match code[from..].find("], ") {
                Some(end) if end < 200 => &code[from + end + 3..],
                _ => return Vec::new(),
            }
        } else {
            &code[from..]
        };
        split_args(args)
    }

    fn offset_in(code: &str, part: &str) -> usize {
        part.as_ptr() as usize - code.as_ptr() as usize
    }

    fn line_of(code: &str, at: usize) -> usize {
        code[..at].matches('\n').count() + 1
    }

    /// A call starts at `start`, not as the tail of a longer name or a method of something.
    fn starts_call(code: &str, start: usize) -> bool {
        let before = code[..start].chars().next_back();
        !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
    }

    struct Marked {
        file: &'static str,
        text: String,
        line: usize,
        /// Where the literal starts in the lexed source.
        at: usize,
    }

    /// Every literal inside `tr(...)`, `trf(...)` and `en(...)`, and the text arguments of the
    /// translating helpers.
    fn marked() -> Vec<Marked> {
        let mut found = Vec::new();
        for (file, source) in SOURCES.into_iter().chain(MARKED_ONLY) {
            let lexed = lex(source);
            let code = lexed.code.as_str();
            for call in ["tr(", "trf(", "en("] {
                let mut from = 0;
                while let Some(at) = code[from..].find(call) {
                    let start = from + at;
                    from = start + call.len();
                    if !starts_call(code, start) {
                        continue;
                    }
                    let tail = code[from..].trim_start();
                    if tail.starts_with('"') {
                        if let Some((text, _)) = literal(tail) {
                            found.push(Marked { file, text, line: line_of(code, start), at: offset_in(code, tail) });
                        }
                    }
                }
            }
            for (prefix, count) in HELPERS {
                let mut from = 0;
                while let Some(at) = code[from..].find(prefix) {
                    let start = from + at;
                    from = start + prefix.len();
                    if !starts_call(code, start) {
                        continue;
                    }
                    for arg in helper_args(code, prefix, start).into_iter().take(count) {
                        let arg = arg.trim();
                        if arg.starts_with('"') {
                            if let Some((text, _)) = literal(arg) {
                                found.push(Marked { file, text, line: line_of(code, start), at: offset_in(code, arg) });
                            }
                        }
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
    fn the_lexer_blanks_comments_and_keeps_strings() {
        let lexed = lex("a(\"x // y\", '\"'); // tr(\"gone\")\n/* tr(\"also\") */ b<'a>(\"z\")");
        assert!(!lexed.code.contains("gone") && !lexed.code.contains("also"), "{}", lexed.code);
        assert_eq!(lexed.literals.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), ["x // y", "z"]);
        assert_eq!(lexed.code.lines().count(), 2, "lines stay where they were");
        assert_eq!(split_args("\"a, b\", f(1, 2), ','), tail"), ["\"a, b\"", " f(1, 2)", " ','"]);
    }

    #[test]
    fn every_marked_text_has_a_german_entry() {
        let table = german();
        let missing: Vec<String> = marked()
            .into_iter()
            .filter(|m| !table.contains_key(m.text.as_str()))
            .map(|m| format!("{}:{}: {:?}", m.file, m.line, m.text))
            .collect();
        assert!(missing.is_empty(), "no German for:\n{}", missing.join("\n"));
    }

    #[test]
    fn the_german_table_is_clean() {
        let mut seen = std::collections::HashSet::new();
        for (english, german) in de::TABLE {
            assert!(seen.insert(*english), "{english:?} is in the table twice");
            assert!(!english.trim().is_empty() && !german.trim().is_empty(), "{english:?} has an empty side");
            let (e, g) = (placeholders(english), placeholders(german));
            let sorted = |p: &[(usize, bool)]| {
                let mut indexes: Vec<usize> = p.iter().map(|p| p.0).collect();
                indexes.sort_unstable();
                indexes
            };
            assert_eq!(sorted(&e), sorted(&g), "{{}} differ in {english:?}");
            // With several placeholders the German sentence may need another order, so it names
            // the English argument each one shows: {0}, {1}, ...
            if e.len() > 1 {
                assert!(g.iter().all(|p| p.1), "several placeholders need {{0}}, {{1}}, ... in German: {english:?}");
            }
            assert_eq!(english.starts_with(' '), german.starts_with(' '), "leading space differs in {english:?}");
            assert_eq!(english.ends_with('\n'), german.ends_with('\n'), "trailing newline differs in {english:?}");
            assert_eq!(english.matches('\n').count(), german.matches('\n').count(), "line breaks differ in {english:?}");
        }
        let used: std::collections::HashSet<String> = marked().into_iter().map(|m| m.text).collect();
        let unused: Vec<&str> = de::TABLE.iter().map(|(e, _)| *e).filter(|e| !used.contains(*e)).collect();
        assert!(unused.is_empty(), "German entries no source uses (renamed English text?):\n{}", unused.join("\n"));
    }

    #[test]
    fn no_shown_text_says_experimental_in_either_language() {
        // Features ship plainly (ux/labels.md); setting keys such as experimental_packed_444 are not text.
        let says = |text: &str| text.to_lowercase().contains("experiment");
        let labelled: Vec<String> = marked().into_iter().map(|m| m.text).filter(|t| says(t)).collect();
        assert!(labelled.is_empty(), "English text says Experimental:\n{}", labelled.join("\n"));
        let german: Vec<&str> = de::TABLE.iter().map(|(_, g)| *g).filter(|g| says(g)).collect();
        assert!(german.is_empty(), "German text says Experimentell:\n{}", german.join("\n"));
        let shown = |file: &str, text: &str| marked().iter().any(|m| m.file == file && m.text == text);
        for (file, text) in [("main.rs", "Send image to PC"), ("main.rs", "Copy PC image here"), ("ui.rs", "Shared folders"),
            ("ui.rs", "Full chroma"), ("ui.rs", "Co-op"), ("ui.rs", "Client roaming"), ("ui.rs", "USB forwarding")]
        {
            assert!(shown(file, text), "{file} shows {text:?}");
        }
    }

    #[test]
    fn placeholders_report_order_and_form() {
        assert_eq!(placeholders("{} of {}"), [(0, false), (1, false)]);
        assert_eq!(placeholders("{1} von {0} {{}}"), [(1, true), (0, true)]);
    }

    /// The last argument of a call.
    const LAST: usize = usize::MAX;

    /// Calls that draw text, and which argument is the text. A bare literal there is untranslated.
    const TEXT_CALLS: [(&str, usize); 32] = [
        ("RichText::new(", 0),
        ("Label::new(", 0),
        ("Button::new(", 0),
        ("Checkbox::new(", LAST),
        (".label(", 0),
        (".colored_label(", 1),
        (".weak(", 0),
        (".small(", 0),
        (".strong(", 0),
        (".monospace(", 0),
        (".heading(", 0),
        (".hint_text(", 0),
        (".on_hover_text(", 0),
        (".on_disabled_hover_text(", 0),
        (".selected_text(", 0),
        (".button(", 0),
        (".menu_button(", 0),
        (".collapsing(", 0),
        ("CollapsingHeader::new(", 0),
        ("Window::new(", 0),
        (".message(", 0),
        (".message(", LAST),
        // `painter.text(pos, align, "text", font, color)`
        (".text(", 2),
        ("show_toast(", 0),
        // `selectable_value(&mut v, VALUE, "text")`, `.checkbox(&mut v, "text")`
        ("selectable_value(", LAST),
        ("selectable_label(", LAST),
        (".radio_value(", LAST),
        (".checkbox(", LAST),
        (".toggle_value(", LAST),
        (".link(", 0),
        (".hyperlink_to(", 0),
        (".on_hover_text_at_pointer(", 0),
    ];

    /// The stream code is read for its toasts only.
    const STREAM_CALLS: [(&str, usize); 1] = [("show_toast(", 0)];

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

    /// Whether a bare literal needs no translation: a symbol, an example, a format string that
    /// is all placeholders, or in the list above.
    fn same_everywhere(text: &str) -> bool {
        let (mut letters, mut inside) = (0, false);
        for c in text.chars() {
            match c {
                '{' => inside = true,
                '}' => inside = false,
                c if !inside && c.is_alphabetic() => letters += 1,
                _ => {}
            }
        }
        SAME_EVERYWHERE.contains(&text) || letters < 2
    }

    /// The bare literal a UI call starts at `text`, which begins with the quote.
    fn bare(text: &str) -> Option<String> {
        literal(text).map(|(t, _)| t).filter(|t| !same_everywhere(t))
    }

    /// The bare literals a text argument is made of: the literal itself, the format string of a
    /// `format!`, or the literals an `if` picks between.
    fn bare_in(arg: &str) -> Vec<String> {
        let arg = arg.trim().trim_start_matches('&').trim_start();
        if arg.starts_with('"') {
            return bare(arg).into_iter().collect();
        }
        if let Some(rest) = arg.strip_prefix("format!(") {
            return split_args(rest).first().map(|first| bare_in(first)).unwrap_or_default();
        }
        if arg.starts_with("if ") {
            let mut out = Vec::new();
            for (at, _) in arg.match_indices('{') {
                let tail = arg[at + 1..].trim_start();
                if tail.starts_with('"') {
                    out.extend(bare(tail));
                }
            }
            return out;
        }
        Vec::new()
    }

    /// `file:line: text` for every bare literal in the text argument of one of `calls`.
    fn bare_texts(file: &str, source: &str, calls: &[(&str, usize)]) -> Vec<String> {
        let lexed = lex(source);
        let code = lexed.code.as_str();
        let mut found = Vec::new();
        for (call, which) in calls {
            for (start, _) in code.match_indices(call) {
                let args = split_args(&code[start + call.len()..]);
                let arg = if *which == LAST { args.last() } else { args.get(*which) };
                for text in arg.map(|arg| bare_in(arg)).unwrap_or_default() {
                    found.push(format!("{file}:{}: {text:?}", line_of(code, start)));
                }
            }
        }
        found
    }

    #[test]
    fn no_text_is_left_in_the_ui_code() {
        let mut found = Vec::new();
        for (file, source) in SOURCES {
            found.extend(bare_texts(file, source, &TEXT_CALLS));
            // A formatted text passed to a helper is looked up as a whole and never found.
            let lexed = lex(source);
            for (prefix, count) in HELPERS {
                for (start, _) in lexed.code.match_indices(prefix) {
                    for arg in helper_args(&lexed.code, prefix, start).into_iter().take(count) {
                        if arg.trim_start().trim_start_matches('&').starts_with("format!(") && !bare_in(arg).is_empty() {
                            found.push(format!("{file}:{}: format! in {}", line_of(&lexed.code, start), prefix.trim_end_matches(", ")));
                        }
                    }
                }
            }
        }
        for (file, source) in MARKED_ONLY {
            found.extend(bare_texts(file, source, &STREAM_CALLS));
        }
        found.sort();
        found.dedup();
        assert!(found.is_empty(), "text without tr():\n{}", found.join("\n"));
    }

    #[test]
    fn the_scan_finds_what_it_is_meant_to_find() {
        let found = |code: &str| bare_texts("t.rs", code, &TEXT_CALLS);
        assert_eq!(found("ui.colored_label(ERROR, \"Broken\");").len(), 1);
        assert_eq!(found("ui.label(\n    RichText::new(\n        \"Broken\"));").len(), 1);
        assert_eq!(found("ui.add(Label::new(\"Broken\"));").len(), 1);
        assert_eq!(found("painter.text(p, A, \"Broken\", f, c);").len(), 1);
        assert_eq!(found("ui.radio_value(&mut v, 1, \"Broken\");").len(), 1);
        assert_eq!(found("r.on_disabled_hover_text(\"Broken\");").len(), 1);
        assert_eq!(found("ui.label(format!(\"Broken {}\", x));").len(), 1);
        assert_eq!(found("show_toast(if a { \"One\" } else { \"Two\" });").len(), 2);
        assert!(found("ui.label(tr(\"Fine\")); ui.small(format!(\"{}\", x)); // ui.label(\"x\")").is_empty());
        assert!(found("ui.label(trf(\"Fine {}\", &[&x])); ui.label(\"1:2\");").is_empty());
    }

    #[test]
    fn windows_and_headers_keep_their_ids_across_languages() {
        // egui derives a window's or header's id from its title, so a translated title would give
        // each language its own saved state (and shift the focus when the language changes).
        let mut found = Vec::new();
        for (file, source) in SOURCES.into_iter().chain(MARKED_ONLY) {
            let lexed = lex(source);
            for pattern in ["Window::new(", "CollapsingHeader::new("] {
                for (at, _) in lexed.code.match_indices(pattern) {
                    let call = lexed.code[at..].split(".show(").next().unwrap_or("");
                    if !call.contains(".id(") && !call.contains(".id_salt(") {
                        found.push(format!("{file}:{}: {pattern}", line_of(&lexed.code, at)));
                    }
                }
            }
            for (at, _) in lexed.code.match_indices("from_label(") {
                found.push(format!("{file}:{}: ComboBox::from_label takes its id from the label", line_of(&lexed.code, at)));
            }
        }
        assert!(found.is_empty(), "give these an explicit id:
{}", found.join("
"));
        let source = lex("egui::Window::new(tr(\"T\")).anchor(a).show(&ctx, |ui| {});").code;
        assert!(!source.split(".show(").next().unwrap().contains(".id("), "the check can fail");
    }

    #[test]
    fn match_arm_texts_in_radeon_go_through_en() {
        let mut found = Vec::new();
        for (file, source) in SOURCES.into_iter().filter(|(file, _)| *file == "radeon.rs") {
            let lexed = lex(source);
            for (at, _) in lexed.code.match_indices("=> ") {
                let tail = lexed.code[at + 3..].trim_start();
                if tail.starts_with('"') && bare(tail).is_some() {
                    found.push(format!("{file}:{}: {}", line_of(&lexed.code, at), tail.lines().next().unwrap_or("")));
                }
            }
        }
        assert!(found.is_empty(), "match arms with a bare text, wrap it in en():\n{}", found.join("\n"));
    }

    #[test]
    fn a_translated_text_is_not_used_bare() {
        // The catch-all: whatever call it is in, a literal that has a German entry and is not in
        // tr(), trf(), en() or a helper's text argument would show in English. The stream
        // overlay's labels stay English and are not read here.
        let table = german();
        let at: std::collections::HashSet<(&str, usize)> = marked().iter().map(|m| (m.file, m.at)).collect();
        let mut found = Vec::new();
        for (file, source) in SOURCES {
            let lexed = lex(source);
            for (start, text) in &lexed.literals {
                // `name == "App"` compares; it does not show anything.
                let before = lexed.code[..*start].trim_end();
                if before.ends_with("==") || before.ends_with("!=") {
                    continue;
                }
                if table.contains_key(text.as_str()) && !at.contains(&(file, *start)) {
                    found.push(format!("{file}:{}: {text:?}", line_of(&lexed.code, *start)));
                }
            }
        }
        assert!(found.is_empty(), "translated text used without tr():\n{}", found.join("\n"));
    }
}
