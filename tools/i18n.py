"""Batch translation CLI and verifier across Host JSON, Desktop Rust, and Android XML locales.

Spans all three localization surfaces in Rubylight / Nightjar:
  - Host JSONs:    rust/assets/locale/*.json (22 locales)
  - Desktop Rust:  client/protocol/desktop-config/src/strings/*.rs (14 frozen tables, merge=union)
                   + client/protocol/desktop-config/src/strings_70/*.rs (14 tables for 7.0 text);
                   checks cover the union per language, a key lives in exactly one of the two,
                   `apply` writes new keys to strings_70/ and updates an existing key in place
  - Android XML:   client/app/src/main/res/values*/ (all 42 values* folders: base + TV + qualifiers)

Usage:
  python tools/i18n.py check [--surface all|host|desktop|android] [--strict-host] [--json]
  python tools/i18n.py apply SPEC.json|- [--dry-run] [--no-cross-fill] [--update-glossary]
  python tools/i18n.py rename {host,desktop,android,android-tv} OLD_KEY NEW_KEY [--dry-run]
  python tools/i18n.py prune  {host,desktop,android,android-tv} [KEY ...] [--stale] [--dry-run]
"""

import argparse
import json
import pathlib
import re
import sys
import tomllib
import unicodedata
import xml.etree.ElementTree as ET

ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_GLOSSARY = pathlib.Path(__file__).resolve().parent / 'i18n_glossary.json'

HOST_REL = pathlib.PurePosixPath('rust/assets/locale')
HOST_LOCALE_RS = pathlib.PurePosixPath('rust/host/stream/src/locale.rs')
DESKTOP_REL = pathlib.PurePosixPath('client/protocol/desktop-config/src/strings')
DESKTOP_70_REL = pathlib.PurePosixPath('client/protocol/desktop-config/src/strings_70')
# A language's tables: the frozen one (size-ratcheted, never grows) first, then 7.0's.
DESKTOP_TABLE_DIRS = (DESKTOP_REL, DESKTOP_70_REL)
DESKTOP_TESTS_RS = DESKTOP_REL / 'tests.rs'
ANDROID_REL = pathlib.PurePosixPath('client/app/src/main/res')
ANDROID_TESTS_JAVA = pathlib.PurePosixPath('client/app/src/test/java/com/limelight/TranslationResourcesTest.java')

HOST_LOCALES = (
    'bg', 'cs', 'de', 'en', 'en_GB', 'en_US', 'es', 'fr', 'hu', 'it', 'ja',
    'ko', 'pl', 'pt', 'pt_BR', 'ru', 'sv', 'tr', 'uk', 'vi', 'zh', 'zh_TW',
)
DESKTOP_LOCALES = (
    'de', 'es', 'fr', 'it', 'ja', 'ko', 'pl', 'pt_br', 'ru', 'tr', 'uk', 'vi',
    'zh_hans', 'zh_hant',
)
ANDROID_LOCALES = (
    'de', 'es', 'fr', 'it', 'ja', 'ko', 'pl', 'pt-rBR', 'ru', 'tr', 'uk', 'vi',
    'zh-rCN', 'zh-rTW',
)

CANON_TO_DESKTOP = {
    'de': 'de', 'es': 'es', 'fr': 'fr', 'it': 'it', 'ja': 'ja', 'ko': 'ko',
    'pl': 'pl', 'pt_BR': 'pt_br', 'ru': 'ru', 'tr': 'tr', 'uk': 'uk', 'vi': 'vi',
    'zh': 'zh_hans', 'zh_TW': 'zh_hant',
}
DESKTOP_TO_CANON = {v: k for k, v in CANON_TO_DESKTOP.items()}

CANON_TO_ANDROID = {
    'de': 'de', 'es': 'es', 'fr': 'fr', 'it': 'it', 'ja': 'ja', 'ko': 'ko',
    'pl': 'pl', 'pt_BR': 'pt-rBR', 'ru': 'ru', 'tr': 'tr', 'uk': 'uk', 'vi': 'vi',
    'zh': 'zh-rCN', 'zh_TW': 'zh-rTW',
}
ANDROID_TO_CANON = {v: k for k, v in CANON_TO_ANDROID.items()}

LOCALE_ALIASES = {
    'en': 'en', 'en-gb': 'en_GB', 'en_gb': 'en_GB', 'en-us': 'en_US', 'en_us': 'en_US',
    'bg': 'bg', 'cs': 'cs', 'de': 'de', 'es': 'es', 'fr': 'fr', 'hu': 'hu',
    'it': 'it', 'ja': 'ja', 'ko': 'ko', 'pl': 'pl', 'pt': 'pt',
    'pt-br': 'pt_BR', 'pt_br': 'pt_BR', 'pt-rbr': 'pt_BR',
    'ru': 'ru', 'sv': 'sv', 'tr': 'tr', 'uk': 'uk', 'vi': 'vi',
    'zh': 'zh', 'zh-cn': 'zh', 'zh_cn': 'zh', 'zh-rcn': 'zh', 'zh-hans': 'zh', 'zh_hans': 'zh',
    'zh-tw': 'zh_TW', 'zh_tw': 'zh_TW', 'zh-rtw': 'zh_TW', 'zh-hant': 'zh_TW', 'zh_hant': 'zh_TW',
}

HOST_PH_RE = re.compile(r'\{[A-Za-z0-9_]+\}')
ANDROID_PH_RE = re.compile(r'%(\d+\$)?[-#+0,(]*\d*(\.\d+)?[sdfxXc%]')
ANDROID_BARE_APOS_RE = re.compile(r"(?<!\\)'")
ANDROID_BARE_QUOTE_RE = re.compile(r'(?<!\\)"')
ANDROID_BASE_DIR_RE = re.compile(r'^values-([a-z]{2}(?:-r[A-Z]{2})?)$')
ANDROID_TV_DIR_RE = re.compile(r'^values-([a-z]{2}(?:-r[A-Z]{2})?)-television$')
TOOLS_IGNORE_ATTR = '{http://schemas.android.com/tools}ignore'

EXPERIMENTAL_WORDS = (
    'experiment', 'expérimental', 'sperimental', 'eksperyment', 'эксперимент',
    'експеримент', 'deneysel', '実験', '실험', '实验', '實驗',
)
HOTKEY_CAPS = (('ctrl', 'Ctrl'), ('shift', 'Shift'), ('enter', 'Enter'), ('del', 'Del'))
DEFAULT_SAME_IN_ENGLISH = frozenset({
    'Xbox', 'PlayStation', 'Nintendo', 'HDR', 'CPU', 'GPU', 'Guide', 'Start',
    'Ctrl', 'Shift', 'Enter', 'Del', 'OK', 'PIN', 'Co-op', 'Stereo', 'Codec',
    'Software', 'Video', 'Audio', 'PC', 'PCs', 'App', 'USB', 'PyroWave',
    'RTX HDR', 'RTX Video', 'V-Sync', 'Bitrate', 'Debug', 'Overlay', 'Mouse',
})
DEFAULT_TIGHT_LABELS = frozenset({
    'addpc_button', 'applist_menu_cancel', 'crash_report_not_now',
    'keyboard_shortcuts_save', 'keyboard_shortcuts_delete', 'keyboard_shortcuts_replace_button',
    'host_profile_save', 'host_menu_rename', 'host_rename_save',
    'latency_benchmark_start', 'latency_benchmark_share', 'library_retry',
    'remote_next', 'remote_previous', 'remote_test', 'report_problem_share',
    'help', 'settings', 'library_search', 'library_options',
    'help_connect_link', 'help_pairing_link', 'help_controls_link',
    'help_profiles_link', 'help_problems_link', 'help_report_link',
})


# ---- Common helpers ----

def normalize_locale(code):
    """Normalize any surface locale code (e.g. pt-rBR, zh_hans, values-de) to canonical form."""
    raw = str(code).strip()
    if raw.startswith('values-'):
        raw = raw[len('values-'):]
    if raw.endswith('-television'):
        raw = raw[:-len('-television')]
    key = raw.lower()
    if key in LOCALE_ALIASES:
        return LOCALE_ALIASES[key]
    raise ValueError(f'unknown locale code: {code!r}')


def normalize_translations_map(raw_map):
    """Return {canonical_locale: value} from a user-provided translations dict."""
    out = {}
    for code, text in (raw_map or {}).items():
        canon = normalize_locale(code)
        if not isinstance(text, str):
            raise ValueError(f'translation for {code!r} must be a string')
        out[canon] = text
    return out


def load_glossary(path=None):
    """Load the shared glossary JSON file."""
    target = pathlib.Path(path) if path else DEFAULT_GLOSSARY
    if not target.is_file():
        return {'schema_version': 1, 'locales': {}, 'host_legacy_missing': {}, 'entries': {}}
    return json.loads(target.read_text(encoding='utf-8'))


def save_glossary(glossary, path=None):
    target = pathlib.Path(path) if path else DEFAULT_GLOSSARY
    target.write_text(json.dumps(glossary, indent=2, ensure_ascii=False) + '\n', encoding='utf-8', newline='\n')


def write_lf(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text.replace('\r\n', '\n'), encoding='utf-8', newline='\n')


def check_lf(path):
    return b'\r\n' not in path.read_bytes()


# ---- Host JSON surface ----

def flatten_json(obj, prefix=''):
    out = {}
    for k, v in obj.items():
        key = f'{prefix}.{k}' if prefix else k
        if isinstance(v, dict):
            out.update(flatten_json(v, key))
        else:
            out[key] = v
    return out


def set_nested(obj, dotted_key, value):
    parts = dotted_key.split('.')
    cur = obj
    for part in parts[:-1]:
        nxt = cur.get(part)
        if nxt is None:
            nxt = {}
            cur[part] = nxt
        elif not isinstance(nxt, dict):
            raise ValueError(f'key conflict at {part!r} in {dotted_key!r}: existing scalar')
        cur = nxt
    cur[parts[-1]] = value


def get_nested(obj, dotted_key):
    cur = obj
    for part in dotted_key.split('.'):
        if not isinstance(cur, dict) or part not in cur:
            return None, False
        cur = cur[part]
    return cur, True


def delete_nested(obj, dotted_key):
    parts = dotted_key.split('.')
    stack = []
    cur = obj
    for part in parts[:-1]:
        if not isinstance(cur, dict) or part not in cur:
            return False
        stack.append((cur, part))
        cur = cur[part]
    if not isinstance(cur, dict) or parts[-1] not in cur:
        return False
    del cur[parts[-1]]
    for parent, part in reversed(stack):
        if isinstance(parent[part], dict) and not parent[part]:
            del parent[part]
    return True


def expected_host_locales(root):
    locale_rs = root / HOST_LOCALE_RS
    if locale_rs.is_file():
        text = locale_rs.read_text(encoding='utf-8')
        m = re.search(r'catalogues!\s*\((.*?)\)\s*;', text, re.S)
        if m:
            found = re.findall(r'"([^"]+)"', m.group(1))
            if found:
                return tuple(sorted(found))
    host_dir = root / HOST_REL
    if host_dir.is_dir():
        found = sorted(p.stem for p in host_dir.glob('*.json'))
        if found:
            return tuple(found)
    return HOST_LOCALES


def host_legacy_allowed(glossary, locale):
    """Frozen legacy missing keys permitted in a host locale unless --strict-host."""
    cfg = glossary.get('host_legacy_missing') or {}
    if not cfg or locale == 'en':
        return set()
    allowed = set(cfg.get('common') or [])
    if locale != 'de':
        allowed.update(cfg.get('couch_menu_en_de_only') or [])
        allowed.update(cfg.get('non_de_extra') or [])
    override = (cfg.get('overrides') or {}).get(locale) or {}
    allowed.update(override.get('extra_missing') or [])
    allowed.difference_update(override.get('present_from_common') or [])
    return allowed


def check_host(root, glossary, strict_host=False):
    issues = []
    host_dir = root / HOST_REL
    if not host_dir.is_dir():
        return {'locales': 0, 'keys': 0, 'issues': [f'host: missing directory {HOST_REL}']}
    expected = expected_host_locales(root)
    actual_files = {p.stem: p for p in sorted(host_dir.glob('*.json'))}
    for loc in expected:
        if loc not in actual_files:
            issues.append(f'host/{loc}.json: missing locale file')
    if 'en' not in actual_files:
        return {'locales': len(actual_files), 'keys': 0, 'issues': issues or ['host/en.json: missing']}

    parsed = {}
    for loc, path in actual_files.items():
        if not check_lf(path):
            issues.append(f'host/{loc}.json: contains CRLF line endings')
        try:
            data = json.loads(path.read_text(encoding='utf-8'))
        except json.JSONDecodeError as exc:
            issues.append(f'host/{loc}.json: invalid JSON ({exc})')
            continue
        if not isinstance(data, dict):
            issues.append(f'host/{loc}.json: root must be a JSON object')
            continue
        parsed[loc] = flatten_json(data)

    en_flat = parsed.get('en', {})
    for key, val in en_flat.items():
        if isinstance(val, str) and not val.strip():
            issues.append(f'host/en.json/{key}: empty string')

    legacy_skipped = 0
    for loc, flat in parsed.items():
        if loc == 'en':
            continue
        allowed_miss = set() if strict_host else host_legacy_allowed(glossary, loc)
        for key in sorted(set(flat) - set(en_flat)):
            issues.append(f'host/{loc}.json/{key}: stale key not in en.json')
        for key in sorted(set(en_flat) - set(flat)):
            if key in allowed_miss:
                legacy_skipped += 1
            else:
                issues.append(f'host/{loc}.json/{key}: missing translation')
        for key, val in flat.items():
            if isinstance(val, str) and not val.strip():
                issues.append(f'host/{loc}.json/{key}: empty string')
            if key in en_flat and isinstance(val, str) and isinstance(en_flat[key], str):
                ep = sorted(HOST_PH_RE.findall(en_flat[key]))
                vp = sorted(HOST_PH_RE.findall(val))
                if ep != vp:
                    issues.append(f'host/{loc}.json/{key}: placeholders {vp} differ from en {ep}')

    return {
        'locales': len(actual_files),
        'keys': len(en_flat),
        'legacy_baselined': legacy_skipped,
        'issues': issues,
    }


# ---- Desktop Rust string table surface ----

def parse_rust_literal(text, pos):
    """Parse a Rust double-quoted string literal starting at text[pos] == '\"'.

    Returns (decoded_value, end_pos) where text[end_pos - 1] == '\"'.
    Matches client/protocol/desktop-config/src/strings/tests.rs::literal.
    """
    if pos >= len(text) or text[pos] != '"':
        raise ValueError(f'expected opening quote at offset {pos}')
    out = []
    i = pos + 1
    n = len(text)
    while i < n:
        ch = text[i]
        if ch == '"':
            return ''.join(out), i + 1
        if ch == '\\':
            if i + 1 >= n:
                raise ValueError('unterminated escape in Rust string literal')
            nxt = text[i + 1]
            if nxt == 'n':
                out.append('\n'); i += 2
            elif nxt == 't':
                out.append('\t'); i += 2
            elif nxt == 'r':
                out.append('\r'); i += 2
            elif nxt == '"':
                out.append('"'); i += 2
            elif nxt == '\\':
                out.append('\\'); i += 2
            elif nxt == "'":
                out.append("'"); i += 2
            elif nxt == 'u' and i + 2 < n and text[i + 2] == '{':
                end = text.find('}', i + 3)
                if end == -1:
                    raise ValueError('unterminated \\u{...} escape')
                out.append(chr(int(text[i + 3:end], 16)))
                i = end + 1
            elif nxt in ('\n', '\r'):
                i += 2
                while i < n and text[i] in ' \t\r\n':
                    i += 1
            else:
                raise ValueError(f'unsupported Rust escape \\{nxt} at offset {i}')
        else:
            out.append(ch)
            i += 1
    raise ValueError(f'unterminated Rust string literal starting at offset {pos}')


def escape_rust_literal(value):
    """Encode a Python str as a single-line Rust string literal body (without outer quotes)."""
    out = []
    for ch in value:
        if ch == '\\':
            out.append('\\\\')
        elif ch == '"':
            out.append('\\"')
        elif ch == '\n':
            out.append('\\n')
        elif ch == '\r':
            out.append('\\r')
        elif ch == '\t':
            out.append('\\t')
        else:
            out.append(ch)
    return ''.join(out)


def parse_rust_table(text):
    """Parse a Desktop Rust string table preserving exact character spans for surgical edits.

    Returns dict with keys: body_start, body_end, entries (list of dicts with
    en, tr, en_span, tr_span, entry_span).
    Raises ValueError if the table violates the merge=union structure.
    """
    marker = '= &[\n'
    idx = text.find(marker)
    if idx == -1:
        raise ValueError('missing `= &[\\n` table header')
    body_start = idx + len(marker)
    body_end = text.rfind('\n];')
    if body_end == -1 or body_end < body_start - 1:
        raise ValueError('missing `\\n];` table footer')
    body_end = max(body_end, body_start)  # an empty table: `= &[\n];`

    entries = []
    i = body_start
    while i < body_end:
        ch = text[i]
        if ch in ' \t\r\n':
            i += 1
            continue
        if text.startswith('//', i):
            nl = text.find('\n', i, body_end)
            i = body_end if nl == -1 else nl + 1
            continue
        if ch != '(':
            snippet = text[i:min(body_end, i + 30)].replace('\n', '\\n')
            raise ValueError(f'unexpected token inside TABLE body at offset {i}: {snippet!r}')
        entry_start = i
        i += 1
        while i < body_end and text[i] in ' \t\r\n':
            i += 1
        en_start = i
        en_val, i = parse_rust_literal(text, i)
        en_end = i
        while i < body_end and text[i] in ' \t\r\n':
            i += 1
        if i >= body_end or text[i] != ',':
            raise ValueError(f'expected `,` after key {en_val!r} at offset {i}')
        i += 1
        while i < body_end and text[i] in ' \t\r\n':
            i += 1
        tr_start = i
        tr_val, i = parse_rust_literal(text, i)
        tr_end = i
        while i < body_end and text[i] in ' \t\r\n':
            i += 1
        if i < body_end and text[i] == ',':
            i += 1
            while i < body_end and text[i] in ' \t\r\n':
                i += 1
        if i >= body_end or text[i] != ')':
            raise ValueError(f'expected `)` closing entry {en_val!r} at offset {i}')
        i += 1
        while i < body_end and text[i] in ' \t\r\n':
            i += 1
        if i >= body_end or text[i] != ',':
            raise ValueError(f'expected trailing `,` after entry {en_val!r} at offset {i}')
        i += 1
        entries.append({
            'en': en_val,
            'tr': tr_val,
            'en_span': (en_start, en_end),
            'tr_span': (tr_start, tr_end),
            'entry_span': (entry_start, i),
        })
    return {'body_start': body_start, 'body_end': body_end, 'entries': entries}


def desktop_placeholders(template):
    """Return [(arg_index, is_explicit)] matching strings.rs::placeholders."""
    found = []
    nxt = 0
    i = 0
    n = len(template)
    while i < n:
        if template[i] in '{}':
            if i + 1 < n and template[i + 1] == template[i]:
                i += 2
                continue
            if template[i] == '{':
                end = template.find('}', i + 1)
                if end == -1:
                    break
                inner = template[i + 1:end]
                if inner.isdigit():
                    found.append((int(inner), True))
                else:
                    found.append((nxt, False))
                    nxt += 1
                i = end + 1
                continue
        i += 1
    return found


def load_same_in_english(root):
    """Read SAME_IN_ENGLISH from strings/tests.rs when present."""
    tests_rs = root / DESKTOP_TESTS_RS
    if not tests_rs.is_file():
        return set(DEFAULT_SAME_IN_ENGLISH)
    text = tests_rs.read_text(encoding='utf-8')
    m = re.search(r'const\s+SAME_IN_ENGLISH\s*:\s*&\[&str\]\s*=\s*&\[(.*?)\];', text, re.S)
    if not m:
        return set(DEFAULT_SAME_IN_ENGLISH)
    items = set()
    body = m.group(1)
    i = 0
    while i < len(body):
        if body[i] == '"':
            val, i = parse_rust_literal(body, i)
            items.add(val)
        else:
            i += 1
    return items


def validate_desktop_pair(lang, english, translated, same_in_english):
    """Validate a single Desktop (english, translated) pair per strings/tests.rs rules."""
    errs = []
    if not english.strip() or not translated.strip():
        errs.append(f'desktop/{lang}: empty side for {english!r}')
        return errs
    ep = desktop_placeholders(english)
    tp = desktop_placeholders(translated)
    if sorted(p[0] for p in ep) != sorted(p[0] for p in tp):
        errs.append(f'desktop/{lang}: placeholders differ for {english!r}')
    elif len(ep) > 1 and not all(p[1] for p in tp):
        errs.append(f'desktop/{lang}: multiple placeholders must be indexed ({{0}}, {{1}}) for {english!r}')
    if english.startswith(' ') != translated.startswith(' '):
        errs.append(f'desktop/{lang}: leading space differs for {english!r}')
    if english.endswith('\n') != translated.endswith('\n'):
        errs.append(f'desktop/{lang}: trailing newline differs for {english!r}')
    if english.count('\n') != translated.count('\n'):
        errs.append(f'desktop/{lang}: line breaks differ for {english!r}')
    if english == translated and english not in same_in_english:
        errs.append(f'desktop/{lang}: untranslated label left in English: {english!r}')
    if 'experiment' in english.lower():
        errs.append(f'desktop/{lang}: English text says experimental: {english!r}')
    low_tr = translated.lower()
    if any(w in low_tr for w in EXPERIMENTAL_WORDS):
        errs.append(f'desktop/{lang}: translated text says experimental for {english!r}')
    return errs


def check_desktop(root):
    issues = []
    desk_dir = root / DESKTOP_REL
    if not desk_dir.is_dir():
        return {'locales': 0, 'keys': 0, 'issues': [f'desktop: missing directory {DESKTOP_REL}']}

    same_in_english = load_same_in_english(root)
    tables = {}
    for lang in DESKTOP_LOCALES:
        seen = {}
        for rel in DESKTOP_TABLE_DIRS:
            where = f'desktop/{desktop_table_name(rel, lang)}'
            path = root / rel / f'{lang}.rs'
            if not path.is_file():
                issues.append(f'{where}: missing table file')
                continue
            if not check_lf(path):
                issues.append(f'{where}: contains CRLF line endings')
            try:
                parsed = parse_rust_table(path.read_text(encoding='utf-8'))
            except ValueError as exc:
                issues.append(f'{where}: {exc}')
                continue
            for row in parsed['entries']:
                en, tr = row['en'], row['tr']
                if en in seen:
                    issues.append(f'{where}: duplicate key {en!r} (a key lives in one table of a language)')
                    continue
                seen[en] = tr
                issues.extend(validate_desktop_pair(lang, en, tr, same_in_english))
        tables[lang] = seen

    if tables:
        all_keys = set().union(*tables.values())
        for lang, entries in tables.items():
            for key in sorted(all_keys - set(entries)):
                issues.append(f'desktop/{lang}.rs: missing key {key!r}')
    else:
        all_keys = set()

    # Hotkey key-cap parity against Android strings_stream_hotkeys.xml (matches strings/tests.rs)
    res_dir = root / ANDROID_REL
    if tables and (res_dir / 'values/strings_stream_hotkeys.xml').is_file():
        for lang, entries in tables.items():
            and_loc = CANON_TO_ANDROID[DESKTOP_TO_CANON[lang]]
            hotkeys_xml = res_dir / f'values-{and_loc}/strings_stream_hotkeys.xml'
            if not hotkeys_xml.is_file():
                continue
            try:
                xml_root = ET.parse(hotkeys_xml).getroot()
            except ET.ParseError:
                continue
            by_name = {el.get('name'): ''.join(el.itertext()) for el in xml_root if el.tag == 'string'}
            for suffix, key in HOTKEY_CAPS:
                xml_key = f'key_cap_{suffix}'
                if xml_key in by_name and key in entries and by_name[xml_key] != entries[key]:
                    issues.append(
                        f'desktop/{lang}.rs vs android/values-{and_loc}: '
                        f'{key!r}={entries[key]!r} != {xml_key}={by_name[xml_key]!r}'
                    )

    return {'locales': len(tables), 'keys': len(all_keys), 'issues': issues}


def desktop_table_name(rel, lang):
    """`de.rs` for the frozen table, `strings_70/de.rs` for 7.0's (as issues name them)."""
    return f'{lang}.rs' if rel == DESKTOP_REL else f'{rel.name}/{lang}.rs'


def desktop_table_texts(root, lang):
    """[(path, text)] of a language's existing tables, frozen first."""
    paths = [root / rel / f'{lang}.rs' for rel in DESKTOP_TABLE_DIRS]
    return [(p, p.read_text(encoding='utf-8')) for p in paths if p.is_file()]


def desktop_keys(text):
    return {row['en'] for row in parse_rust_table(text)['entries']}


def apply_desktop_edit(text, updates):
    """Apply {english: translated} edits to a Desktop Rust table preserving comments & merge=union."""
    parsed = parse_rust_table(text)
    by_key = {row['en']: row for row in parsed['entries']}
    # Replace existing entries in reverse offset order so earlier offsets remain valid
    replacements = []
    to_append = []
    for en, tr in updates:
        if en in by_key:
            s, e = by_key[en]['tr_span']
            replacements.append((s, e, f'"{escape_rust_literal(tr)}"'))
        else:
            to_append.append((en, tr))
    out = text
    for s, e, rep in sorted(replacements, key=lambda x: x[0], reverse=True):
        out = out[:s] + rep + out[e:]
    if to_append:
        # Re-find footer after replacements
        footer = out.rfind('\n];')
        prefix = out[:footer]
        suffix = out[footer:]
        added = ''.join(
            f'\n    ("{escape_rust_literal(en)}", "{escape_rust_literal(tr)}"),'
            for en, tr in to_append
        )
        out = prefix + added + suffix
    return out


def rename_desktop_key(text, old_key, new_key):
    parsed = parse_rust_table(text)
    by_key = {row['en']: row for row in parsed['entries']}
    if old_key not in by_key:
        raise KeyError(old_key)
    if new_key in by_key:
        raise ValueError(f'target key already exists: {new_key!r}')
    s, e = by_key[old_key]['en_span']
    return text[:s] + f'"{escape_rust_literal(new_key)}"' + text[e:]


def prune_desktop_keys(text, keys_to_remove):
    parsed = parse_rust_table(text)
    remove_set = set(keys_to_remove)
    spans = []
    removed = []
    for row in parsed['entries']:
        if row['en'] in remove_set:
            s, e = row['entry_span']
            line_start = text.rfind('\n', 0, s)
            line_start = 0 if line_start == -1 else line_start + 1
            line_end = text.find('\n', e)
            line_end = len(text) if line_end == -1 else line_end + 1
            spans.append((line_start, line_end))
            removed.append(row['en'])
    out = text
    for s, e in sorted(spans, key=lambda x: x[0], reverse=True):
        out = out[:s] + out[e:]
    return out, removed


# ---- Android XML surface ----

def display_width(text):
    """Display columns matching TranslationResourcesTest.java::width (CJK = 2)."""
    cols = 0
    for ch in text:
        cp = ord(ch)
        if 0x3000 <= cp <= 0x303F or 0xFF00 <= cp <= 0xFF60:
            cols += 2
            continue
        eaw = unicodedata.east_asian_width(ch)
        if eaw in ('W', 'F'):
            cols += 2
        else:
            cols += 1
    return cols


def fits_tight_label(english, translated):
    en_w = display_width(english)
    return display_width(translated) <= max(2 * en_w, en_w + 10)


def load_tight_labels(root):
    java = root / ANDROID_TESTS_JAVA
    if not java.is_file():
        return set(DEFAULT_TIGHT_LABELS)
    text = java.read_text(encoding='utf-8')
    m = re.search(r'TIGHT_LABELS\s*=\s*Set\.of\((.*?)\);', text, re.S)
    if not m:
        return set(DEFAULT_TIGHT_LABELS)
    return set(re.findall(r'"([^"]+)"', m.group(1)))


def android_placeholders(text):
    return sorted(m.group(0) for m in ANDROID_PH_RE.finditer(text))


def android_unescape(text):
    return text.replace("\\'", "'").replace('\\"', '"').replace('\\n', '\n')


def android_escape_value(text, raw_xml=False):
    """Format a string for Android XML <string> body (matching TranslationResourcesTest rules)."""
    def _escape_quotes(segment):
        segment = segment.replace('\n', '\\n')
        segment = ANDROID_BARE_APOS_RE.sub(r"\\'", segment)
        segment = ANDROID_BARE_QUOTE_RE.sub(r'\\"', segment)
        return segment

    if not raw_xml:
        out = re.sub(r'&(?!amp;|lt;|gt;|quot;|apos;|#\d+;|#x[0-9a-fA-F]+;)', '&amp;', text)
        out = out.replace('<', '&lt;').replace('>', '&gt;')
        return _escape_quotes(out)
    # Preserve inline XML tags/attributes while escaping quotes only in text segments
    parts = re.split(r'(<[^>]+>)', text)
    return ''.join(p if p.startswith('<') and p.endswith('>') else _escape_quotes(p) for p in parts)


def load_android_dir(directory, require_translatable_only=False):
    """Load Android string/plurals/string-array resources from all *.xml files in a values* dir.

    Returns ({name: {type, formatted, values, file, only_refs, ignore_missing}}, issues).
    """
    entries = {}
    issues = []
    for path in sorted(directory.glob('*.xml')):
        if not check_lf(path):
            issues.append(f'android/{directory.name}/{path.name}: contains CRLF line endings')
        try:
            root_el = ET.parse(path).getroot()
        except ET.ParseError as exc:
            issues.append(f'android/{directory.name}/{path.name}: invalid XML ({exc})')
            continue
        file_ignore = 'MissingTranslation' in root_el.attrib.get(TOOLS_IGNORE_ATTR, '')
        for el in root_el:
            if el.tag not in ('string', 'plurals', 'string-array'):
                continue
            if el.get('translatable') == 'false':
                continue
            name = el.get('name')
            if not name:
                continue
            el_ignore = file_ignore or ('MissingTranslation' in el.attrib.get(TOOLS_IGNORE_ATTR, ''))
            formatted = el.get('formatted') != 'false'
            vals = {}
            only_refs = False
            if el.tag == 'string':
                vals[''] = ''.join(el.itertext())
            else:
                items = el.findall('item')
                only_refs = bool(items) and all((''.join(i.itertext())).strip().startswith('@') for i in items)
                for idx, item in enumerate(items):
                    k = item.get('quantity') if el.tag == 'plurals' else str(idx)
                    vals[k] = ''.join(item.itertext())
            if require_translatable_only and (el_ignore or (el.tag != 'string' and only_refs)):
                continue
            entries[name] = {
                'type': el.tag,
                'formatted': formatted,
                'values': vals,
                'file': path.name,
                'only_refs': only_refs,
                'ignore_missing': el_ignore,
            }
    return entries, issues


def android_xml_text(escaped_xml):
    """Return decoded XML text content (matching Android DOM getTextContent / itertext)."""
    return ''.join(ET.fromstring(f'<string>{escaped_xml}</string>').itertext())


def validate_android_text(where, decoded_text):
    """Validate already-decoded XML text content (matching TranslationResourcesTest.java)."""
    errs = []
    if ANDROID_BARE_APOS_RE.search(decoded_text):
        errs.append(f'{where}: unescaped apostrophe (use \\\' or ’)')
    if ANDROID_BARE_QUOTE_RE.search(decoded_text):
        errs.append(f'{where}: unescaped double quote (use \\" or typographic quotes)')
    return errs


def check_android(root):
    issues = []
    res_dir = root / ANDROID_REL
    if not res_dir.is_dir():
        return {'folders': 0, 'base_locales': 0, 'tv_locales': 0, 'keys': 0, 'issues': [f'android: missing {ANDROID_REL}']}

    tight_labels = load_tight_labels(root)
    all_values_dirs = sorted(d for d in res_dir.iterdir() if d.is_dir() and d.name.startswith('values'))
    base_dirs = {}
    tv_dirs = {}
    other_dirs = []
    for d in all_values_dirs:
        if d.name in ('values', 'values-television'):
            continue
        m_base = ANDROID_BASE_DIR_RE.match(d.name)
        m_tv = ANDROID_TV_DIR_RE.match(d.name)
        if m_base:
            base_dirs[m_base.group(1)] = d
        elif m_tv:
            tv_dirs[m_tv.group(1)] = d
        else:
            other_dirs.append(d)

    # Validate XML well-formedness & LF across ALL qualifier values-* folders (all 42 folders covered!)
    for d in other_dirs:
        _, dir_issues = load_android_dir(d)
        issues.extend(dir_issues)

    # Check base values/ vs 14 values-<lang>/
    base_default_dir = res_dir / 'values'
    if not base_default_dir.is_dir():
        issues.append('android/values: missing default directory')
        return {'folders': len(all_values_dirs), 'base_locales': 0, 'tv_locales': 0, 'keys': 0, 'issues': issues}

    defaults_all, def_issues = load_android_dir(base_default_dir)
    issues.extend(def_issues)
    defaults_req, _ = load_android_dir(base_default_dir, require_translatable_only=True)
    for name, entry in defaults_all.items():
        for val in entry['values'].values():
            issues.extend(validate_android_text(f'android/values/{name}', val))

    for loc in ANDROID_LOCALES:
        if loc not in base_dirs:
            issues.append(f'android/values-{loc}: missing locale directory')
    for loc, d in sorted(base_dirs.items()):
        loaded, dir_issues = load_android_dir(d)
        issues.extend(dir_issues)
        for name in sorted(set(defaults_req) - set(loaded)):
            issues.append(f'android/{d.name}/{name}: missing translation')
        for name, entry in loaded.items():
            where = f'android/{d.name}/{name}'
            source = defaults_all.get(name)
            if source is None:
                issues.append(f'{where}: stale key not in values/')
                continue
            for item_key, val in entry['values'].items():
                issues.extend(validate_android_text(where, val))
                if not source['formatted']:
                    continue
                eng = source['values'].get('other') if source['type'] == 'plurals' else source['values'].get(item_key)
                if eng is None:
                    issues.append(f'{where}: item {item_key!r} not in values/')
                    continue
                if android_placeholders(eng) != android_placeholders(val):
                    issues.append(
                        f'{where}: placeholders {android_placeholders(val)} differ from {android_placeholders(eng)}'
                    )
                if name in tight_labels and not fits_tight_label(eng, val):
                    issues.append(f'{where}: "{val}" is too long for tight label "{eng}"')

    # Check values-television/ vs 14 values-<lang>-television/
    tv_default_dir = res_dir / 'values-television'
    if tv_default_dir.is_dir():
        tv_defaults, tv_def_issues = load_android_dir(tv_default_dir, require_translatable_only=True)
        issues.extend(tv_def_issues)
        for name, entry in tv_defaults.items():
            for val in entry['values'].values():
                issues.extend(validate_android_text(f'android/values-television/{name}', val))
        for loc in ANDROID_LOCALES:
            if loc not in tv_dirs:
                issues.append(f'android/values-{loc}-television: missing TV locale directory')
        for loc, d in sorted(tv_dirs.items()):
            loaded, dir_issues = load_android_dir(d)
            issues.extend(dir_issues)
            for name in sorted(set(tv_defaults) - set(loaded)):
                issues.append(f'android/{d.name}/{name}: missing TV translation')
            for name, entry in loaded.items():
                where = f'android/{d.name}/{name}'
                source = tv_defaults.get(name)
                if source is None:
                    issues.append(f'{where}: stale key not in values-television/')
                    continue
                for item_key, val in entry['values'].items():
                    issues.extend(validate_android_text(where, val))
                    if not source['formatted']:
                        continue
                    eng = source['values'].get('other') if source['type'] == 'plurals' else source['values'].get(item_key)
                    if eng is not None and android_placeholders(eng) != android_placeholders(val):
                        issues.append(
                            f'{where}: placeholders {android_placeholders(val)} differ from {android_placeholders(eng)}'
                        )

    return {
        'folders': len(all_values_dirs),
        'base_locales': len(base_dirs),
        'tv_locales': len(tv_dirs),
        'keys': len(defaults_req),
        'issues': issues,
    }


def _find_android_key_file(directory, key):
    pat = re.compile(rf'<(string|plurals|string-array)\b[^>]*\bname="{re.escape(key)}"')
    for path in sorted(directory.glob('*.xml')):
        text = path.read_text(encoding='utf-8')
        if pat.search(text):
            return path
    return None


def upsert_android_string_xml(text, key, escaped_value, formatted=True):
    """Surgically update or insert <string name="key">...</string> preserving XML comments."""
    pat = re.compile(
        rf'(<string\b[^>]*\bname="{re.escape(key)}"[^>]*>)(.*?)(</string>)',
        re.S,
    )
    m = pat.search(text)
    if m:
        updated = text[:m.start(2)] + escaped_value + text[m.end(2):]
    else:
        fmt_attr = '' if formatted else ' formatted="false"'
        line = f'    <string name="{key}"{fmt_attr}>{escaped_value}</string>\n'
        close = text.rfind('</resources>')
        if close == -1:
            raise ValueError('missing </resources> closing tag')
        prefix = text[:close]
        if prefix and not prefix.endswith('\n'):
            prefix += '\n'
        updated = prefix + line + text[close:]
    # Verify XML stays strictly well-formed
    ET.fromstring(updated)
    return updated


def rename_android_key_xml(text, old_key, new_key):
    if re.search(rf'<(?:string|plurals|string-array)\b[^>]*\bname="{re.escape(new_key)}"', text):
        raise ValueError(f'target key already exists: {new_key!r}')
    pat = re.compile(rf'(<(?:string|plurals|string-array)\b[^>]*\bname="){re.escape(old_key)}(")')
    updated, count = pat.subn(rf'\g<1>{new_key}\g<2>', text)
    if count:
        ET.fromstring(updated)
    return updated, count


def prune_android_keys_xml(text, keys):
    removed = []
    updated = text
    for key in keys:
        pat = re.compile(
            rf'^[ \t]*<(string|plurals|string-array)\b[^>]*\bname="{re.escape(key)}"[^>]*>.*?</\1>[ \t]*\r?\n?',
            re.S | re.M,
        )
        new_text, count = pat.subn('', updated)
        if count:
            updated = new_text
            removed.append(key)
    if removed:
        ET.fromstring(updated)
    return updated, removed


# ---- Cross-fill engine (Glossary + Desktop <-> Android + Host) ----

def _normalize_ph_signature(text, style):
    """Map placeholders to canonical {0}, {1}, ... slots so Desktop <-> Android can match."""
    if style == 'desktop':
        idx = 0
        def repl(m):
            nonlocal idx
            tok = m.group(0)
            if tok in ('{{', '}}'):
                return tok
            inner = tok[1:-1]
            if inner.isdigit():
                return f'<<PH:{int(inner)}>>'
            slot = idx
            idx += 1
            return f'<<PH:{slot}>>'
        norm = re.sub(r'\{\{|\}\}|\{\d*\}', repl, text)
        return norm, None
    # Android style
    slots = []
    idx = 0
    def repl_and(m):
        nonlocal idx
        tok = m.group(0)
        if tok == '%%':
            return '%%'
        if m.group(1):
            slot = int(m.group(1)[:-1]) - 1
        else:
            slot = idx
            idx += 1
        slots.append((slot, tok))
        return f'<<PH:{slot}>>'
    norm = ANDROID_PH_RE.sub(repl_and, text)
    return norm, slots


def _adapt_placeholders(translated, src_style, dst_style, dst_android_slots=None):
    """Convert placeholder tokens when cross-filling between Desktop and Android."""
    if src_style == dst_style:
        return translated
    if src_style == 'desktop' and dst_style == 'android':
        slots = dst_android_slots or []
        by_idx = {s: tok for s, tok in slots}
        if not by_idx:
            return translated
        idx = 0
        def repl(m):
            nonlocal idx
            tok = m.group(0)
            if tok in ('{{', '}}'):
                return tok[0]
            inner = tok[1:-1]
            slot = int(inner) if inner.isdigit() else idx
            if not inner.isdigit():
                idx += 1
            return by_idx.get(slot, tok)
        return re.sub(r'\{\{|\}\}|\{\d*\}', repl, translated)
    if src_style == 'android' and dst_style == 'desktop':
        phs = [m for m in ANDROID_PH_RE.finditer(translated) if m.group(0) != '%%']
        multi = len(phs) > 1
        idx = 0
        def repl_a(m):
            nonlocal idx
            tok = m.group(0)
            if tok == '%%':
                return '%'
            if m.group(1):
                slot = int(m.group(1)[:-1]) - 1
            else:
                slot = idx
                idx += 1
            return f'{{{slot}}}' if multi else '{}'
        return ANDROID_PH_RE.sub(repl_a, translated)
    return translated


def _apos_norm(text):
    return text.replace('\u2019', "'")


def build_cross_fill_index(root, glossary, spec):
    """Build lookup maps from glossary, existing Desktop/Android tables, and current batch."""
    # Exact English -> {canon_locale: plain_text}
    exact = {}
    # Typographic-apostrophe-folded English -> {canon_locale: plain_text}
    folded = {}
    # Normalized placeholder English -> list of (style, {canon_locale: text})
    norm_entries = []

    def add_source(en_text, tr_map, style):
        if not en_text or not tr_map:
            return
        bucket = exact.setdefault(en_text, {})
        fbucket = folded.setdefault(_apos_norm(en_text), {})
        for loc, val in tr_map.items():
            bucket.setdefault(loc, val)
            fbucket.setdefault(loc, val)
        norm_en, _ = _normalize_ph_signature(_apos_norm(en_text), style)
        if '<<PH:' in norm_en:
            norm_entries.append((norm_en, style, tr_map))

    # 1. Same-batch Desktop & Android entries (highest priority for cross-fill!)
    for item in spec.get('desktop') or []:
        en = item.get('key', '')
        try:
            tr = normalize_translations_map(item.get('translations'))
        except ValueError:
            continue
        add_source(en, tr, 'desktop')
    for sec in ('android', 'android_tv'):
        for item in spec.get(sec) or []:
            raw_xml = bool(item.get('raw_xml', False))
            en_raw = item.get('en', '')
            try:
                en = android_unescape(android_xml_text(android_escape_value(en_raw, raw_xml=raw_xml))) if en_raw else ''
                raw_tr = normalize_translations_map(item.get('translations'))
            except (ValueError, ET.ParseError):
                continue
            tr = {}
            for k, v in raw_tr.items():
                try:
                    tr[k] = android_unescape(android_xml_text(android_escape_value(v, raw_xml=raw_xml)))
                except ET.ParseError:
                    tr[k] = android_unescape(v)
            add_source(en, tr, 'android')

    # 2. Shared glossary entries
    for en, raw_tr in (glossary.get('entries') or {}).items():
        add_source(en, normalize_translations_map(raw_tr), 'desktop')

    # 3. Live Desktop tables in repo
    desk_dir = root / DESKTOP_REL
    if desk_dir.is_dir():
        desk_by_en = {}
        for lang in DESKTOP_LOCALES:
            canon = DESKTOP_TO_CANON[lang]
            for _, text in desktop_table_texts(root, lang):
                try:
                    parsed = parse_rust_table(text)
                except ValueError:
                    continue
                for row in parsed['entries']:
                    desk_by_en.setdefault(row['en'], {})[canon] = row['tr']
        for en, tr in desk_by_en.items():
            add_source(en, tr, 'desktop')

    # 4. Live Android tables in repo
    res_dir = root / ANDROID_REL
    if (res_dir / 'values').is_dir():
        and_en, _ = load_android_dir(res_dir / 'values')
        and_by_lang = {}
        for loc in ANDROID_LOCALES:
            d = res_dir / f'values-{loc}'
            if d.is_dir():
                and_by_lang[ANDROID_TO_CANON[loc]], _ = load_android_dir(d)
        for key, entry in and_en.items():
            if entry['type'] != 'string':
                continue
            en_plain = android_unescape(entry['values'].get('', ''))
            tr = {}
            for canon, loaded in and_by_lang.items():
                if key in loaded and loaded[key]['type'] == 'string':
                    tr[canon] = android_unescape(loaded[key]['values'].get('', ''))
            if tr:
                add_source(en_plain, tr, 'android')

    return (exact, folded), norm_entries


def cross_fill_for(en_text, target_style, needed_locales, existing_map, exact_index, norm_entries):
    """Fill missing canonical locales from exact_index or placeholder-compatible norm_entries."""
    filled = dict(existing_map)
    cross_filled_count = 0
    exact, folded = exact_index

    # Exact plain match first, then typographic-apostrophe-folded match
    for cand in (exact.get(en_text) or {}, folded.get(_apos_norm(en_text)) or {}):
        for loc in needed_locales:
            if loc not in filled and loc in cand:
                filled[loc] = cand[loc]
                cross_filled_count += 1

    # Placeholder-normalized match across Desktop <-> Android if still missing
    still_missing = [loc for loc in needed_locales if loc not in filled]
    if still_missing:
        norm_en, dst_slots = _normalize_ph_signature(_apos_norm(en_text), target_style)
        if '<<PH:' in norm_en:
            for src_norm, src_style, src_map in norm_entries:
                if src_norm != norm_en:
                    continue
                for loc in list(still_missing):
                    if loc in src_map and loc not in filled:
                        filled[loc] = _adapt_placeholders(src_map[loc], src_style, target_style, dst_slots)
                        cross_filled_count += 1
                        still_missing.remove(loc)
                if not still_missing:
                    break
    return filled, cross_filled_count


# ---- Subcommands ----

def cmd_check(args):
    root = pathlib.Path(args.root).resolve()
    glossary = load_glossary(args.glossary)
    surfaces = ('host', 'desktop', 'android') if args.surface == 'all' else (args.surface,)
    report = {}
    all_issues = []
    if 'host' in surfaces:
        report['host'] = check_host(root, glossary, strict_host=args.strict_host)
        all_issues.extend(report['host']['issues'])
    if 'desktop' in surfaces:
        report['desktop'] = check_desktop(root)
        all_issues.extend(report['desktop']['issues'])
    if 'android' in surfaces:
        report['android'] = check_android(root)
        all_issues.extend(report['android']['issues'])

    if args.json:
        print(json.dumps({'ok': not all_issues, 'surfaces': report}, indent=2, ensure_ascii=False))
    else:
        if 'host' in report:
            h = report['host']
            base_note = f' ({h["legacy_baselined"]} legacy baselined)' if h.get('legacy_baselined') else ''
            print(f'host:    {h["locales"]} locales, {h["keys"]} keys, {len(h["issues"])} issue(s){base_note}')
        if 'desktop' in report:
            d = report['desktop']
            print(f'desktop: {d["locales"]} tables, {d["keys"]} keys, {len(d["issues"])} issue(s)')
        if 'android' in report:
            a = report['android']
            print(
                f'android: {a["folders"]} values* folders '
                f'({a["base_locales"]} base + {a["tv_locales"]} tv), '
                f'{a["keys"]} keys, {len(a["issues"])} issue(s)'
            )
        for issue in all_issues:
            print(f'  ERROR: {issue}')
    return 1 if all_issues else 0


def load_spec(spec_arg):
    if spec_arg == '-':
        raw = sys.stdin.read()
        return json.loads(raw)
    path = pathlib.Path(spec_arg)
    raw = path.read_text(encoding='utf-8')
    if path.suffix.lower() == '.toml':
        return tomllib.loads(raw)
    return json.loads(raw)


def cmd_apply(args):
    root = pathlib.Path(args.root).resolve()
    glossary = load_glossary(args.glossary)
    try:
        spec = load_spec(args.spec)
    except (OSError, ValueError, json.JSONDecodeError, tomllib.TOMLDecodeError) as exc:
        print(f'ERROR: failed to load spec: {exc}', file=sys.stderr)
        return 1
    if not isinstance(spec, dict):
        print('ERROR: spec root must be a JSON/TOML object', file=sys.stderr)
        return 1
    exact_index, norm_entries = (({}, {}), []) if args.no_cross_fill else build_cross_fill_index(root, glossary, spec)

    errors = []
    planned_writes = {}  # pathlib.Path -> new_text
    stats = {'host': 0, 'desktop': 0, 'android': 0, 'android_tv': 0, 'cross_filled': 0}
    glossary_updates = {}

    # 1. Host JSON entries
    host_items = spec.get('host') or []
    if host_items:
        host_dir = root / HOST_REL
        locales = expected_host_locales(root)
        host_docs = {}
        for loc in locales:
            p = host_dir / f'{loc}.json'
            if not p.is_file():
                errors.append(f'host/{loc}.json: missing target file')
                continue
            host_docs[loc] = json.loads(p.read_text(encoding='utf-8'))

        for item in host_items:
            key = item.get('key', '').strip()
            if not key:
                errors.append('host: entry missing non-empty "key"')
                continue
            try:
                tr_map = normalize_translations_map(item.get('translations'))
            except ValueError as exc:
                errors.append(f'host/{key}: {exc}')
                continue
            en_val = item.get('en', tr_map.get('en', ''))
            if not en_val or not en_val.strip():
                errors.append(f'host/{key}: missing English text ("en")')
                continue
            tr_map['en'] = en_val
            needed = [loc for loc in locales if loc != 'en']
            if not args.no_cross_fill:
                tr_map, cf = cross_fill_for(en_val, 'desktop', needed, tr_map, exact_index, norm_entries)
                stats['cross_filled'] += cf
                # Regional English variants fall back to en when unspecified
                for eng_var in ('en_GB', 'en_US'):
                    if eng_var in needed and eng_var not in tr_map:
                        tr_map[eng_var] = en_val
                        stats['cross_filled'] += 1
            missing = [loc for loc in locales if loc not in tr_map]
            if missing:
                errors.append(f'host/{key}: missing translations for {", ".join(missing)}')
                continue
            en_ph = sorted(HOST_PH_RE.findall(en_val))
            item_ok = True
            for loc in locales:
                val = tr_map[loc]
                if not val.strip():
                    errors.append(f'host/{key}/{loc}: empty translation')
                    item_ok = False
                    continue
                if sorted(HOST_PH_RE.findall(val)) != en_ph:
                    errors.append(f'host/{key}/{loc}: placeholder mismatch vs en')
                    item_ok = False
                    continue
                if loc in host_docs:
                    try:
                        set_nested(host_docs[loc], key, val)
                    except ValueError as exc:
                        errors.append(f'host/{loc}.json/{key}: {exc}')
                        item_ok = False
            if item_ok:
                stats['host'] += 1
                glossary_updates.setdefault(en_val, {}).update({k: v for k, v in tr_map.items() if k != 'en'})

        if not errors:
            for loc, doc in host_docs.items():
                planned_writes[host_dir / f'{loc}.json'] = json.dumps(doc, indent=2, ensure_ascii=False) + '\n'

    # 2. Desktop Rust table entries
    desk_items = spec.get('desktop') or []
    if desk_items:
        same_in_english = load_same_in_english(root)
        per_lang_updates = {lang: [] for lang in DESKTOP_LOCALES}
        needed_canon = [DESKTOP_TO_CANON[lang] for lang in DESKTOP_LOCALES]

        for item in desk_items:
            en_key = item.get('key', '')
            if not en_key or not en_key.strip():
                errors.append('desktop: entry missing non-empty "key"')
                continue
            try:
                tr_map = normalize_translations_map(item.get('translations'))
            except ValueError as exc:
                errors.append(f'desktop/{en_key!r}: {exc}')
                continue
            if not args.no_cross_fill:
                tr_map, cf = cross_fill_for(en_key, 'desktop', needed_canon, tr_map, exact_index, norm_entries)
                stats['cross_filled'] += cf
            missing = [CANON_TO_DESKTOP[c] for c in needed_canon if c not in tr_map]
            if missing:
                errors.append(f'desktop/{en_key!r}: missing translations for {", ".join(missing)}')
                continue
            item_ok = True
            for lang in DESKTOP_LOCALES:
                val = tr_map[DESKTOP_TO_CANON[lang]]
                pair_errs = validate_desktop_pair(lang, en_key, val, same_in_english)
                if pair_errs:
                    errors.extend(pair_errs)
                    item_ok = False
                else:
                    per_lang_updates[lang].append((en_key, val))
            if item_ok:
                stats['desktop'] += 1
                glossary_updates.setdefault(en_key, {}).update({c: tr_map[c] for c in needed_canon})

        if not errors:
            for lang, updates in per_lang_updates.items():
                frozen, new = (root / rel / f'{lang}.rs' for rel in DESKTOP_TABLE_DIRS)
                if not frozen.is_file() or not new.is_file():
                    errors.append(f'desktop/{lang}: missing table file in strings/ or strings_70/')
                    continue
                # An existing key is updated where it is; a new key goes to strings_70/ only,
                # since the frozen tables may never grow.
                frozen_text = frozen.read_text(encoding='utf-8')
                try:
                    in_frozen = desktop_keys(frozen_text)
                except ValueError as exc:
                    errors.append(f'desktop/{lang}.rs: {exc}')
                    continue
                old = [u for u in updates if u[0] in in_frozen]
                added = [u for u in updates if u[0] not in in_frozen]
                if old:
                    planned_writes[frozen] = apply_desktop_edit(frozen_text, old)
                if added:
                    planned_writes[new] = apply_desktop_edit(new.read_text(encoding='utf-8'), added)

    # 3. Android XML entries (base + optional android_tv)
    tight_labels = load_tight_labels(root)
    needed_and_canon = [ANDROID_TO_CANON[loc] for loc in ANDROID_LOCALES]
    for sec, tv_suffix in (('android', ''), ('android_tv', '-television')):
        and_items = spec.get(sec) or []
        if not and_items:
            continue
        res_dir = root / ANDROID_REL
        default_dir = res_dir / ('values-television' if tv_suffix else 'values')
        # Keep in-memory XML buffer per path so multiple entries in one file compose cleanly
        xml_buffers = {}

        def read_xml_buf(path):
            if path in xml_buffers:
                return xml_buffers[path]
            if path.is_file():
                xml_buffers[path] = path.read_text(encoding='utf-8')
            else:
                xml_buffers[path] = '<?xml version="1.0" encoding="utf-8"?>\n<resources>\n</resources>\n'
            return xml_buffers[path]

        for item in and_items:
            key = item.get('key', '').strip()
            if not key:
                errors.append(f'{sec}: entry missing non-empty "key"')
                continue
            en_raw = item.get('en', '')
            if not en_raw or not en_raw.strip():
                errors.append(f'{sec}/{key}: missing English text ("en")')
                continue
            filename = item.get('file', 'strings.xml')
            raw_xml = bool(item.get('raw_xml', False))
            formatted = bool(item.get('formatted', True))
            en_escaped = android_escape_value(en_raw, raw_xml=raw_xml)
            try:
                en_xml_text = android_xml_text(en_escaped)
            except ET.ParseError as exc:
                errors.append(f'{sec}/values/{key}: invalid XML ({exc})')
                continue
            en_errs = validate_android_text(f'{sec}/values/{key}', en_xml_text)
            if en_errs:
                errors.extend(en_errs)
                continue
            en_plain = android_unescape(en_xml_text)

            try:
                tr_map = normalize_translations_map(item.get('translations'))
            except ValueError as exc:
                errors.append(f'{sec}/{key}: {exc}')
                continue
            if not args.no_cross_fill:
                tr_map, cf = cross_fill_for(en_plain, 'android', needed_and_canon, tr_map, exact_index, norm_entries)
                stats['cross_filled'] += cf
            missing = [CANON_TO_ANDROID[c] for c in needed_and_canon if c not in tr_map]
            if missing:
                errors.append(f'{sec}/{key}: missing translations for {", ".join(missing)}')
                continue

            item_errs = []
            en_ph = android_placeholders(en_xml_text) if formatted else []
            escaped_by_loc = {}
            plain_by_canon = {}
            for loc in ANDROID_LOCALES:
                canon = ANDROID_TO_CANON[loc]
                val_esc = android_escape_value(tr_map[canon], raw_xml=raw_xml)
                where = f'{sec}/values-{loc}{tv_suffix}/{key}'
                if not val_esc.strip():
                    item_errs.append(f'{where}: empty translation')
                    continue
                try:
                    val_xml_text = android_xml_text(val_esc)
                except ET.ParseError as exc:
                    item_errs.append(f'{where}: invalid XML ({exc})')
                    continue
                loc_errs = validate_android_text(where, val_xml_text)
                if loc_errs:
                    item_errs.extend(loc_errs)
                    continue
                if formatted and android_placeholders(val_xml_text) != en_ph:
                    item_errs.append(
                        f'{where}: placeholders {android_placeholders(val_xml_text)} differ from {en_ph}'
                    )
                if key in tight_labels and not fits_tight_label(en_xml_text, val_xml_text):
                    item_errs.append(f'{where}: "{val_xml_text}" exceeds tight label width for "{en_xml_text}"')
                escaped_by_loc[loc] = val_esc
                plain_by_canon[canon] = android_unescape(val_xml_text)

            if item_errs:
                errors.extend(item_errs)
                continue

            # Update default XML
            def_target = _find_android_key_file(default_dir, key) or (default_dir / filename)
            xml_buffers[def_target] = upsert_android_string_xml(
                read_xml_buf(def_target), key, en_escaped, formatted=formatted
            )
            # Update all 14 translated XMLs
            for loc, val_esc in escaped_by_loc.items():
                loc_dir = res_dir / f'values-{loc}{tv_suffix}'
                target = _find_android_key_file(loc_dir, key) or (loc_dir / filename)
                xml_buffers[target] = upsert_android_string_xml(
                    read_xml_buf(target), key, val_esc, formatted=formatted
                )
            stats[sec] += 1
            glossary_updates.setdefault(en_plain, {}).update(plain_by_canon)

        if not errors:
            planned_writes.update(xml_buffers)

    if errors:
        for err in errors:
            print(f'ERROR: {err}', file=sys.stderr)
        return 1

    if not args.dry_run:
        for path, content in sorted(planned_writes.items()):
            write_lf(path, content)
        if args.update_glossary and glossary_updates:
            entries = glossary.setdefault('entries', {})
            for en_text, trs in glossary_updates.items():
                entries.setdefault(en_text, {}).update(trs)
                entries[en_text] = dict(sorted(entries[en_text].items()))
            glossary['entries'] = dict(sorted(entries.items()))
            save_glossary(glossary, args.glossary)

    mode = '[dry-run] ' if args.dry_run else ''
    print(
        f'{mode}applied: host={stats["host"]}, desktop={stats["desktop"]}, '
        f'android={stats["android"]}, android_tv={stats["android_tv"]}, '
        f'cross_filled={stats["cross_filled"]}, files={len(planned_writes)}'
    )
    return 0


def cmd_rename(args):
    root = pathlib.Path(args.root).resolve()
    surface, old_key, new_key = args.surface, args.old_key, args.new_key
    if old_key == new_key:
        print('ERROR: old_key and new_key are identical', file=sys.stderr)
        return 1
    planned = {}

    if surface == 'host':
        host_dir = root / HOST_REL
        en_path = host_dir / 'en.json'
        en_doc = json.loads(en_path.read_text(encoding='utf-8'))
        _, exists = get_nested(en_doc, old_key)
        if not exists:
            print(f'ERROR: host/en.json does not contain {old_key!r}', file=sys.stderr)
            return 1
        for p in sorted(host_dir.glob('*.json')):
            doc = json.loads(p.read_text(encoding='utf-8'))
            val, found = get_nested(doc, old_key)
            if not found:
                continue
            _, collision = get_nested(doc, new_key)
            if collision:
                print(f'ERROR: host/{p.name} already contains {new_key!r}', file=sys.stderr)
                return 1
            delete_nested(doc, old_key)
            try:
                set_nested(doc, new_key, val)
            except ValueError as exc:
                print(f'ERROR: host/{p.name}: {exc}', file=sys.stderr)
                return 1
            planned[p] = json.dumps(doc, indent=2, ensure_ascii=False) + '\n'

    elif surface == 'desktop':
        for lang in DESKTOP_LOCALES:
            tables = desktop_table_texts(root, lang)
            if not tables:
                continue
            try:
                keyed = [(p, text, desktop_keys(text)) for p, text in tables]
            except ValueError as exc:
                print(f'ERROR: desktop/{lang}: {exc}', file=sys.stderr)
                return 1
            if any(new_key in keys for _, _, keys in keyed):
                print(f'ERROR: desktop/{lang}: target key already exists: {new_key!r}', file=sys.stderr)
                return 1
            holders = [(p, text) for p, text, keys in keyed if old_key in keys]
            if not holders:
                print(f'ERROR: desktop/{lang}: {old_key!r} is in neither table', file=sys.stderr)
                return 1
            for p, text in holders:
                planned[p] = rename_desktop_key(text, old_key, new_key)

    elif surface in ('android', 'android-tv'):
        res_dir = root / ANDROID_REL
        groups = [True] if surface == 'android-tv' else [False, True]
        dirs = []
        for tv in groups:
            dirs.append(res_dir / ('values-television' if tv else 'values'))
            dirs += [res_dir / (f'values-{loc}-television' if tv else f'values-{loc}') for loc in ANDROID_LOCALES]
        total = 0
        for d in dirs:
            if not d.is_dir():
                continue
            for p in sorted(d.glob('*.xml')):
                try:
                    updated, count = rename_android_key_xml(p.read_text(encoding='utf-8'), old_key, new_key)
                except ValueError as exc:
                    print(f'ERROR: android/{d.name}/{p.name}: {exc}', file=sys.stderr)
                    return 1
                if count:
                    planned[p] = updated
                    total += count
        if not total:
            print(f'ERROR: {surface}: key {old_key!r} not found', file=sys.stderr)
            return 1

    if not args.dry_run:
        for p, text in sorted(planned.items()):
            write_lf(p, text)
    mode = '[dry-run] ' if args.dry_run else ''
    print(f'{mode}renamed {surface}: {old_key!r} -> {new_key!r} ({len(planned)} files)')
    return 0


def cmd_prune(args):
    root = pathlib.Path(args.root).resolve()
    surface = args.surface
    keys = list(args.keys)
    if not keys and not args.stale:
        print('ERROR: provide one or more KEY arguments or --stale', file=sys.stderr)
        return 1
    planned = {}
    removed_total = 0

    if surface == 'host':
        host_dir = root / HOST_REL
        en_doc = json.loads((host_dir / 'en.json').read_text(encoding='utf-8'))
        en_keys = set(flatten_json(en_doc))
        for p in sorted(host_dir.glob('*.json')):
            doc = json.loads(p.read_text(encoding='utf-8'))
            targets = sorted(set(flatten_json(doc)) - en_keys) if (args.stale and p.stem != 'en') else keys
            changed = False
            for k in targets:
                if delete_nested(doc, k):
                    changed = True
                    removed_total += 1
            if changed:
                planned[p] = json.dumps(doc, indent=2, ensure_ascii=False) + '\n'

    elif surface == 'desktop':
        if args.stale and not keys:
            print('ERROR: desktop prune requires explicit KEY arguments', file=sys.stderr)
            return 1
        for lang in DESKTOP_LOCALES:
            for p, text in desktop_table_texts(root, lang):
                updated, removed = prune_desktop_keys(text, keys)
                if removed:
                    planned[p] = updated
                    removed_total += len(removed)

    elif surface in ('android', 'android-tv'):
        res_dir = root / ANDROID_REL
        groups = [True] if surface == 'android-tv' else [False, True]
        for tv in groups:
            def_dir = res_dir / ('values-television' if tv else 'values')
            if not def_dir.is_dir():
                continue
            def_entries, _ = load_android_dir(def_dir)
            dirs = [] if args.stale else [def_dir]
            dirs += [res_dir / (f'values-{loc}-television' if tv else f'values-{loc}') for loc in ANDROID_LOCALES]
            for d in dirs:
                if not d.is_dir():
                    continue
                if args.stale:
                    loaded, _ = load_android_dir(d)
                    targets = sorted(set(loaded) - set(def_entries))
                else:
                    targets = keys
                if not targets:
                    continue
                for p in sorted(d.glob('*.xml')):
                    updated, removed = prune_android_keys_xml(p.read_text(encoding='utf-8'), targets)
                    if removed:
                        planned[p] = updated
                        removed_total += len(removed)

    if not args.dry_run:
        for p, text in sorted(planned.items()):
            write_lf(p, text)
    mode = '[dry-run] ' if args.dry_run else ''
    print(f'{mode}pruned {surface}: {removed_total} entry(ies) across {len(planned)} file(s)')
    return 0


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--root', default=str(ROOT), help='repository root directory')
    parser.add_argument('--glossary', default=str(DEFAULT_GLOSSARY), help='path to i18n_glossary.json')
    sub = parser.add_subparsers(dest='command', required=True)

    p_check = sub.add_parser('check', help='verify completeness, placeholders, escaping, and structure')
    p_check.add_argument('--surface', choices=('all', 'host', 'desktop', 'android'), default='all')
    p_check.add_argument('--strict-host', action='store_true', help='fail on legacy baselined host missing keys too')
    p_check.add_argument('--json', action='store_true', help='emit machine-readable JSON report')
    p_check.set_defaults(func=cmd_check)

    p_apply = sub.add_parser('apply', help='apply a batch JSON/TOML translation spec with automatic cross-fill')
    p_apply.add_argument('spec', help='path to batch JSON/TOML file, or - for stdin')
    p_apply.add_argument('--dry-run', action='store_true', help='validate and print plan without writing files')
    p_apply.add_argument('--no-cross-fill', action='store_true', help='disable automatic glossary and Desktop<->Android cross-fill')
    p_apply.add_argument('--update-glossary', action='store_true', help='persist newly applied translations into glossary')
    p_apply.set_defaults(func=cmd_apply)

    p_rename = sub.add_parser('rename', help='rename a key across all locale files of a surface')
    p_rename.add_argument('surface', choices=('host', 'desktop', 'android', 'android-tv'))
    p_rename.add_argument('old_key')
    p_rename.add_argument('new_key')
    p_rename.add_argument('--dry-run', action='store_true')
    p_rename.set_defaults(func=cmd_rename)

    p_prune = sub.add_parser('prune', help='remove keys (or stale orphan keys) across all locale files of a surface')
    p_prune.add_argument('surface', choices=('host', 'desktop', 'android', 'android-tv'))
    p_prune.add_argument('keys', nargs='*')
    p_prune.add_argument('--stale', action='store_true', help='prune stale keys absent from canonical English source')
    p_prune.add_argument('--dry-run', action='store_true')
    p_prune.set_defaults(func=cmd_prune)
    return parser


def main(argv=None):
    if hasattr(sys.stdout, 'reconfigure'):
        sys.stdout.reconfigure(encoding='utf-8', errors='replace')
    if hasattr(sys.stderr, 'reconfigure'):
        sys.stderr.reconfigure(encoding='utf-8', errors='replace')
    args = build_parser().parse_args(argv)
    return args.func(args)


if __name__ == '__main__':
    sys.exit(main())
