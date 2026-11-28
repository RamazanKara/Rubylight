"""Architecture guardrails for Rubylight and Nightjar.

Checks tracked non-foreign, non-generated source files for:
1. Rust `//!` and Java/Kotlin `/**` module headers on new modules.
2. Rule lints (baselined via tools/arch_baselines.json):
   - raw-config-read: raw Config reads in rust/host/src/stream(.rs|/**) outside stream/config.rs.
   - rubylight-family-gating: Family::Rubylight / Family.RUBYLIGHT feature gating in client/.
   - unannotated-cfg-unix: cfg(...unix...) in client/protocol/desktop{,-*}/ without OS-intent // comment.

Usage:
    python tools/arch_check.py [--root PATH] [--write-baselines]
"""
import argparse
import json
import os
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
EXTENSIONS = ('.rs', '.java', '.kt', '.c', '.cpp', '.h', '.hpp', '.ts', '.svelte')
FOREIGN = re.compile(r'(^|/)(vendor|third-party|node_modules|include|openssl|moonlight-common-c|adrenotools)/')
GENERATED = re.compile(r'(^|/)shaders_spv\.h$')
SKIP_WALK_DIRS = frozenset({'.git', 'target', 'node_modules', '.gradle', 'build', 'dist', '__pycache__', '.venv', 'venv'})

TYPE_RE = re.compile(
    r'^[ \t]*(?:@\w+(?:\([^)]*\))?[ \t]+)*'
    r'(?:(?:public|final|abstract|sealed|non-sealed|data|internal|private|protected|open|annotation|value|enum|inner|actual|expect)[ \t]+)*'
    r'(?:class|interface|enum|object|@interface|record)\b',
    re.M,
)
KT_TOP_LEVEL_RE = re.compile(
    r'^[ \t]*(?:@\w+(?:\([^)]*\))?[ \t]+)*'
    r'(?:(?:public|internal|private|inline|suspend|override|operator|infix|tailrec|external|const|lateinit|actual|expect)[ \t]+)*'
    r'(?:fun|val|var|const|typealias)\b',
    re.M,
)

RAW_METHOD_RE = re.compile(r'\.\s*(?:boolean|integer|path)\s*\(')
RAW_LIST_RE = re.compile(r'\.\s*list\s*\(')
RAW_GET_RE = re.compile(r'\.\s*get\s*\(')
RAW_VALUES_RE = re.compile(
    r'\.\s*values\s*(?:\.\s*(?:get|contains_key|iter|keys|values|len|is_empty|first_key_value|last_key_value|range|into_iter|clone)\b|\[)'
)
NON_CONFIG_GET_RECEIVERS = frozenset({'peers', 'ciphers'})

RUST_FAMILY_RE = re.compile(r'\bFamily\s*::\s*Rubylight\b')
JAVA_FAMILY_RE = re.compile(r'\bFamily\s*\.\s*RUBYLIGHT\b|\bcase\s+RUBYLIGHT\s*:')

CFG_UNIX_RE = re.compile(r'#\[\s*cfg(?:_attr)?\s*\(|cfg!\s*\(')
UNIX_WORD_RE = re.compile(r'\bunix\b')
OS_INTENT_RE = re.compile(
    r'\b(?:linux|macos|mac|apple|unix|posix|darwin|platform|wayland|x11|both)\b',
    re.I,
)


def count_loc(text):
    """Count lines matching tools/code_map.py:120."""
    return text.count('\n') + (not text.endswith('\n') and bool(text))


def is_checked_source(rel_path):
    return rel_path.endswith(EXTENSIONS) and not FOREIGN.search(rel_path) and not GENERATED.search(rel_path)


def discover_files(root):
    """Discover tracked/unignored non-foreign, non-generated source files under root."""
    root = pathlib.Path(root)
    paths = None
    if (root / '.git').exists():
        res = subprocess.run(
            ['git', '-c', 'core.quotepath=off', 'ls-files', '--cached', '--others', '--exclude-standard'],
            cwd=root,
            capture_output=True,
            text=True,
            encoding='utf-8',
            errors='replace',
        )
        if res.returncode == 0:
            paths = res.stdout.splitlines()
    if paths is None:
        paths = []
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = sorted(d for d in dirnames if d not in SKIP_WALK_DIRS)
            for fname in sorted(filenames):
                rel = pathlib.Path(dirpath, fname).relative_to(root).as_posix()
                paths.append(rel)
    found = []
    seen = set()
    for rel in paths:
        rel = rel.replace('\\', '/')
        if rel in seen or not is_checked_source(rel):
            continue
        if (root / rel).is_file():
            seen.add(rel)
            found.append(rel)
    return sorted(found)


def is_rust_test_file(rel_path):
    return (
        rel_path.endswith(('_tests.rs', '_test.rs', '/tests.rs'))
        or rel_path == 'tests.rs'
        or '/tests/' in rel_path
        or '/reconnect_tests/' in rel_path
    )


def is_client_test_file(rel_path):
    if rel_path.endswith('.rs'):
        return is_rust_test_file(rel_path)
    return (
        rel_path.startswith((
            'client/app/src/test/',
            'client/app/src/testDebug/',
            'client/app/src/testRoot/',
            'client/app/src/androidTest/',
        ))
        or '/src/test/' in rel_path
        or '/src/testDebug/' in rel_path
        or '/src/testRoot/' in rel_path
        or '/src/androidTest/' in rel_path
    )


def has_rust_module_header(text):
    """Return True if leading lines (past blanks, #![...] attrs, // non-doc comments) contain non-empty //!."""
    in_attr = False
    bracket_depth = 0
    for raw in text.splitlines()[:60]:
        line = raw.strip()
        if in_attr:
            bracket_depth += _attr_bracket_delta(line)
            if bracket_depth <= 0:
                in_attr = False
                bracket_depth = 0
            continue
        if not line:
            continue
        if line.startswith('//!'):
            if line[3:].strip():
                return True
            continue
        if line.startswith('#!['):
            bracket_depth = _attr_bracket_delta(line)
            if bracket_depth > 0:
                in_attr = True
            continue
        if line.startswith('//') and not line.startswith('///'):
            continue
        break
    return False


_CHAR_LIT_RE = re.compile(
    r"'(?:[^'\\\n\r]|\\(?:[^xu\n\r]|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]{1,6}\}))'"
)
_BYTE_CHAR_LIT_RE = re.compile(
    r"b'(?:[^'\\\n\r]|\\(?:[^xu\n\r]|x[0-9a-fA-F]{2}))'"
)
_RAW_STR_RE = re.compile(r'([bc]?r)(#*)"')


def _attr_bracket_delta(line):
    delta = 0
    i = 0
    n = len(line)
    in_str = False
    escaped = False
    while i < n:
        ch = line[i]
        if in_str:
            if escaped:
                escaped = False
            elif ch == '\\':
                escaped = True
            elif ch == '"':
                in_str = False
            i += 1
            continue
        if ch == '/' and i + 1 < n and line[i + 1] == '/':
            break
        if ch in ('r', 'b', 'c') and (i == 0 or not (line[i - 1].isalnum() or line[i - 1] == '_')):
            rm = _RAW_STR_RE.match(line, i)
            if rm:
                end_tok = '"' + rm.group(2)
                end_pos = line.find(end_tok, rm.end())
                if end_pos == -1:
                    break
                i = end_pos + len(end_tok)
                continue
        if ch == 'b' and i + 1 < n and line[i + 1] == "'" and (i == 0 or not (line[i - 1].isalnum() or line[i - 1] == '_')):
            cm = _BYTE_CHAR_LIT_RE.match(line, i)
            if cm:
                i = cm.end()
                continue
        if ch == "'" and (i == 0 or not (line[i - 1].isalnum() or line[i - 1] == '_')):
            cm = _CHAR_LIT_RE.match(line, i)
            if cm:
                i = cm.end()
                continue
        if ch == '"':
            in_str = True
        elif ch == '[':
            delta += 1
        elif ch == ']':
            delta -= 1
        i += 1
    return delta


_JAVA_TOKEN_RE = re.compile(
    r'//[^\n]*'
    r'|/\*[\s\S]*?\*/'
    r'|"""(?:\\[\s\S]|[^\\])*?"""'
    r'|"(?:\\[\s\S]|[^"\\])*"'
    r"|'(?:\\.|[^'\\\n])*'"
)


def _blank_keep_newlines(text):
    return re.sub(r'[^\n]', ' ', text)


def strip_java_kt(text, suffix='.java', strip_comments=True, strip_strings=True):
    """Strip comments and/or strings from Java/Kotlin source while preserving line numbers and byte length."""
    if suffix == '.java':
        def _replace(m):
            tok = m.group(0)
            if tok.startswith(('//', '/*')):
                return _blank_keep_newlines(tok) if strip_comments else tok
            if not strip_strings:
                return tok
            if tok.startswith('"""') and tok.endswith('"""') and len(tok) >= 6:
                return '"""' + _blank_keep_newlines(tok[3:-3]) + '"""'
            if len(tok) >= 2 and tok[0] in ('"', "'") and tok[-1] == tok[0]:
                return tok[0] + _blank_keep_newlines(tok[1:-1]) + tok[-1]
            return tok

        return _JAVA_TOKEN_RE.sub(_replace, text)

    out = []
    i = 0
    n = len(text)
    nested_comments = suffix == '.kt'
    while i < n:
        ch = text[i]
        # Line comment //
        if ch == '/' and i + 1 < n and text[i + 1] == '/':
            j = text.find('\n', i)
            end = n if j == -1 else j
            if strip_comments:
                out.append(' ' * (end - i))
            else:
                out.append(text[i:end])
            i = end
            continue
        # Block comment /* ... */
        if ch == '/' and i + 1 < n and text[i + 1] == '*':
            start = i
            depth = 1
            i += 2
            while i < n and depth > 0:
                if nested_comments and text[i] == '/' and i + 1 < n and text[i + 1] == '*':
                    depth += 1
                    i += 2
                elif text[i] == '*' and i + 1 < n and text[i + 1] == '/':
                    depth -= 1
                    i += 2
                else:
                    i += 1
            chunk = text[start:i]
            if strip_comments:
                out.append(''.join('\n' if c == '\n' else ' ' for c in chunk))
            else:
                out.append(chunk)
            continue
        # Triple-quoted string """..."""
        if text.startswith('"""', i):
            start = i
            i += 3
            while i < n:
                if suffix == '.java' and text[i] == '\\' and i + 1 < n:
                    i += 2
                elif text.startswith('"""', i):
                    i += 3
                    break
                else:
                    i += 1
            if strip_strings:
                inner = text[start + 3:i - 3] if text.endswith('"""', start, i) else text[start + 3:i]
                out.append('"""')
                out.append(''.join('\n' if c == '\n' else ' ' for c in inner))
                if text.endswith('"""', start, i) and i - start >= 6:
                    out.append('"""')
            else:
                out.append(text[start:i])
            continue
        # Normal string "..."
        if ch == '"':
            start = i
            i += 1
            while i < n:
                if text[i] == '\\' and i + 1 < n:
                    i += 2
                elif text[i] == '"':
                    i += 1
                    break
                else:
                    i += 1
            if strip_strings:
                closed = i - start >= 2 and text[i - 1] == '"'
                inner = text[start + 1:i - 1] if closed else text[start + 1:i]
                out.append('"')
                out.append(''.join('\n' if c == '\n' else ' ' for c in inner))
                if closed:
                    out.append('"')
            else:
                out.append(text[start:i])
            continue
        # Char literal '...'
        if ch == "'":
            start = i
            i += 1
            while i < n:
                if text[i] == '\\' and i + 1 < n:
                    i += 2
                elif text[i] == "'":
                    i += 1
                    break
                elif text[i] == '\n':
                    break
                else:
                    i += 1
            if strip_strings:
                closed = i - start >= 2 and text[i - 1] == "'"
                inner = text[start + 1:i - 1] if closed else text[start + 1:i]
                out.append("'")
                out.append(''.join('\n' if c == '\n' else ' ' for c in inner))
                if closed:
                    out.append("'")
            else:
                out.append(text[start:i])
            continue
        out.append(ch)
        i += 1
    return ''.join(out)


def _extract_java_kt_doc_blocks(text, suffix):
    """Extract top-level /** ... */ block interiors while ignoring //, /* ... */, and string/char literals."""
    blocks = []
    i = 0
    n = len(text)
    nested_comments = suffix == '.kt'
    while i < n:
        ch = text[i]
        if ch == '/' and i + 1 < n and text[i + 1] == '/':
            j = text.find('\n', i)
            i = n if j == -1 else j + 1
            continue
        if ch == '/' and i + 1 < n and text[i + 1] == '*':
            start = i
            depth = 1
            i += 2
            while i < n and depth > 0:
                if nested_comments and text[i] == '/' and i + 1 < n and text[i + 1] == '*':
                    depth += 1
                    i += 2
                elif text[i] == '*' and i + 1 < n and text[i + 1] == '/':
                    depth -= 1
                    i += 2
                else:
                    i += 1
            chunk = text[start:i]
            if chunk.startswith('/**') and len(chunk) >= 5 and chunk.endswith('*/'):
                blocks.append(chunk[3:-2])
            continue
        if text.startswith('"""', i):
            i += 3
            while i < n:
                if suffix == '.java' and text[i] == '\\' and i + 1 < n:
                    i += 2
                elif text.startswith('"""', i):
                    i += 3
                    break
                else:
                    i += 1
            continue
        if ch == '"':
            i += 1
            while i < n:
                if text[i] == '\\' and i + 1 < n:
                    i += 2
                elif text[i] == '"':
                    i += 1
                    break
                else:
                    i += 1
            continue
        if ch == "'":
            i += 1
            while i < n:
                if text[i] == '\\' and i + 1 < n:
                    i += 2
                elif text[i] == "'":
                    i += 1
                    break
                elif text[i] == '\n':
                    break
                else:
                    i += 1
            continue
        i += 1
    return blocks


def has_java_kt_module_header(text, suffix):
    """Return True if a non-empty /** ... */ block precedes the first top-level declaration."""
    comment_stripped = strip_java_kt(text, suffix=suffix, strip_comments=True, strip_strings=False)
    matches = [m for m in (TYPE_RE.search(comment_stripped),) if m]
    if suffix == '.kt':
        m_kt = KT_TOP_LEVEL_RE.search(comment_stripped)
        if m_kt:
            matches.append(m_kt)
    if not matches:
        return False
    first = min(matches, key=lambda mm: mm.start())
    blocks = _extract_java_kt_doc_blocks(text[:first.start()], suffix)
    return any(re.sub(r'^\s*\*?', '', line).strip() for block in blocks for line in block.splitlines())


def strip_rust_comments_and_strings(text):
    """Strip Rust comments and string/char literal interiors while preserving newlines and outer delimiters."""
    out = []
    i = 0
    n = len(text)
    while i < n:
        ch = text[i]
        # Line comment //
        if ch == '/' and i + 1 < n and text[i + 1] == '/':
            j = text.find('\n', i)
            if j == -1:
                out.append(' ' * (n - i))
                break
            out.append(' ' * (j - i))
            out.append('\n')
            i = j + 1
            continue
        # Nested block comment /* ... */
        if ch == '/' and i + 1 < n and text[i + 1] == '*':
            depth = 1
            out.append('  ')
            i += 2
            while i < n and depth > 0:
                if text[i] == '/' and i + 1 < n and text[i + 1] == '*':
                    depth += 1
                    out.append('  ')
                    i += 2
                elif text[i] == '*' and i + 1 < n and text[i + 1] == '/':
                    depth -= 1
                    out.append('  ')
                    i += 2
                else:
                    out.append('\n' if text[i] == '\n' else ' ')
                    i += 1
            continue
        # Raw strings r#"..."#, br#"..."#, cr#"..."#
        if ch in ('r', 'b', 'c') and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == '_')):
            rm = _RAW_STR_RE.match(text, i)
            if rm:
                hashes = rm.group(2)
                out.append(rm.group(0))
                i = rm.end()
                end_tok = '"' + hashes
                while i < n:
                    if text.startswith(end_tok, i):
                        out.append(end_tok)
                        i += len(end_tok)
                        break
                    out.append('\n' if text[i] == '\n' else ' ')
                    i += 1
                continue
        # Byte/C string prefix b"..." or c"..."
        if ch in ('b', 'c') and i + 1 < n and text[i + 1] == '"' and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == '_')):
            out.append(ch + '"')
            i += 2
            i = _consume_rust_quoted(text, i, out)
            continue
        # Normal string "..."
        if ch == '"':
            out.append('"')
            i += 1
            i = _consume_rust_quoted(text, i, out)
            continue
        # Byte char literal b'...' (do not confuse with identifier ending in b like foo_b'a)
        if ch == 'b' and i + 1 < n and text[i + 1] == "'" and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == '_')):
            cm = _BYTE_CHAR_LIT_RE.match(text, i)
            if cm:
                out.append("b'" + ' ' * (len(cm.group(0)) - 3) + "'")
                i = cm.end()
                continue
        # Char literal '...' (do not confuse with lifetimes like 'a or 'static)
        if ch == "'" and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == '_')):
            cm = _CHAR_LIT_RE.match(text, i)
            if cm:
                out.append("'" + ' ' * (len(cm.group(0)) - 2) + "'")
                i = cm.end()
                continue
        out.append(ch)
        i += 1
    return ''.join(out)


def _consume_rust_quoted(text, i, out):
    n = len(text)
    while i < n:
        c = text[i]
        if c == '\\' and i + 1 < n:
            out.append(' ')
            out.append('\n' if text[i + 1] == '\n' else ' ')
            i += 2
        elif c == '"':
            out.append('"')
            return i + 1
        else:
            out.append('\n' if c == '\n' else ' ')
            i += 1
    return i


def _is_cfg_test_predicate(pred):
    pred = pred.strip()
    if pred == 'test':
        return True
    m = re.match(r'^all\s*\(', pred)
    if m:
        open_idx = m.end() - 1
        close_idx = _find_matching_delim(pred, open_idx, '(', ')')
        if close_idx == len(pred) - 1:
            args = _split_top_level_args(pred[open_idx + 1:close_idx])
            return any(_is_cfg_test_predicate(arg) for arg in args)
    return False


def _is_cfg_test_attr(inner):
    inner = inner.strip()
    m = re.match(r'^cfg\s*\(', inner)
    if not m:
        return False
    open_idx = m.end() - 1
    close_idx = _find_matching_delim(inner, open_idx, '(', ')')
    if close_idx != len(inner) - 1:
        return False
    return _is_cfg_test_predicate(inner[open_idx + 1:close_idx])


def _find_matching_delim(code, open_idx, open_ch, close_ch):
    depth = 0
    n = len(code)
    i = open_idx
    while i < n:
        c = code[i]
        if c == open_ch:
            depth += 1
        elif c == close_ch:
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return -1


def strip_cfg_test_blocks(code):
    """Blank out #[cfg(test)] items/statements/blocks in stripped Rust code while preserving newlines."""
    chars = list(code)
    n = len(chars)
    b_depth = 0
    p_depth = 0
    k_depth = 0
    i = 0

    def blank_range(start, end):
        for idx in range(start, end):
            if chars[idx] != '\n':
                chars[idx] = ' '

    while i < n:
        c = chars[i]
        if c == '#' and i + 1 < n and chars[i + 1] == '[':
            end_br = _find_matching_delim(code, i + 1, '[', ']')
            if end_br != -1:
                inner = code[i + 2:end_br]
                if _is_cfg_test_attr(inner):
                    b0, p0, k0 = b_depth, p_depth, k_depth
                    blank_range(i, end_br + 1)
                    j = end_br + 1
                    # Skip whitespace and any additional outer attributes #[...] attached to the same item.
                    while j < n:
                        while j < n and chars[j].isspace():
                            j += 1
                        if j + 1 < n and chars[j] == '#' and chars[j + 1] == '[':
                            next_end = _find_matching_delim(code, j + 1, '[', ']')
                            if next_end == -1:
                                break
                            blank_range(j, next_end + 1)
                            j = next_end + 1
                        else:
                            break
                    kw_m = re.match(r'(?:pub(?:\s*\([^)]*\))?\s+)?([A-Za-z_]\w*)\b', code[j:])
                    kw = kw_m.group(1) if kw_m else ''
                    is_stmt_semicolon = kw in ('let', 'type', 'use') or (
                        kw in ('const', 'static') and not re.match(r'(?:pub(?:\s*\([^)]*\))?\s+)?(?:const|static)\s+(?:unsafe\s+|extern\s+(?:"[^"]*"\s+)?)?fn\b', code[j:])
                    )
                    b, p, k = b0, p0, k0
                    saw_brace = False
                    while j < n:
                        ch = chars[j]
                        if not is_stmt_semicolon:
                            if (b, p, k) == (b0, p0, k0):
                                if ch == '{':
                                    saw_brace = True
                                elif ch in (';', ',') and not saw_brace:
                                    if ch != '\n':
                                        chars[j] = ' '
                                    j += 1
                                    break
                                elif ch in (')', ']', '}') and not saw_brace:
                                    # Closing delimiter of enclosing construct (e.g. last struct field without comma).
                                    break
                        else:
                            if (b, p, k) == (b0, p0, k0):
                                if ch == ';':
                                    if ch != '\n':
                                        chars[j] = ' '
                                    j += 1
                                    break
                                if ch in (')', ']', '}'):
                                    break

                        if ch == '{':
                            b += 1
                        elif ch == '}':
                            b -= 1
                        elif ch == '(':
                            p += 1
                        elif ch == ')':
                            p -= 1
                        elif ch == '[':
                            k += 1
                        elif ch == ']':
                            k -= 1

                        if ch != '\n':
                            chars[j] = ' '
                        j += 1

                        if not is_stmt_semicolon and saw_brace and ch == '}' and (b, p, k) == (b0, p0, k0):
                            # Handle `if ... { ... } else { ... }`.
                            peek = j
                            while peek < n and chars[peek].isspace():
                                peek += 1
                            if re.match(r'else\b', code[peek:]):
                                saw_brace = False
                                continue
                            break
                    i = j
                    continue
        if c == '{':
            b_depth += 1
        elif c == '}':
            b_depth -= 1
        elif c == '(':
            p_depth += 1
        elif c == ')':
            p_depth -= 1
        elif c == '[':
            k_depth += 1
        elif c == ']':
            k_depth -= 1
        i += 1
    return ''.join(chars)


def _split_top_level_args(arg_text):
    if not arg_text.strip():
        return []
    args = []
    start = 0
    p = b = k = 0
    for idx, ch in enumerate(arg_text):
        if ch == '(':
            p += 1
        elif ch == ')':
            p -= 1
        elif ch == '[':
            b += 1
        elif ch == ']':
            b -= 1
        elif ch == '{':
            k += 1
        elif ch == '}':
            k -= 1
        elif ch == ',' and p == 0 and b == 0 and k == 0:
            args.append(arg_text[start:idx])
            start = idx + 1
    args.append(arg_text[start:])
    return args


def _receiver_root_ident(code, dot_idx):
    prefix = code[:dot_idx].rstrip()
    changed = True
    while changed:
        changed = False
        if prefix.endswith(('?',)):
            prefix = prefix[:-1].rstrip()
            changed = True
            continue
        if prefix.endswith(')'):
            # Strip trailing .lock() / .unwrap() / .read() / .borrow() / .borrow_mut() / .as_ref()
            m = re.search(r'\.\s*(?:lock|read|write|unwrap|borrow|borrow_mut|as_ref|expect)\s*\([^()]*\)\s*$', prefix)
            if m:
                prefix = prefix[:m.start()].rstrip()
                changed = True
                continue
        m_await = re.search(r'\.\s*await\s*$', prefix)
        if m_await:
            prefix = prefix[:m_await.start()].rstrip()
            changed = True
    m_id = re.search(r'([A-Za-z_]\w*)\s*$', prefix)
    return m_id.group(1) if m_id else ''


def find_raw_config_reads(text):
    """Return sorted 1-based line numbers of raw Config reads in non-test Rust code."""
    stripped = strip_cfg_test_blocks(strip_rust_comments_and_strings(text))
    hits = set()

    for m in RAW_METHOD_RE.finditer(stripped):
        hits.add(stripped.count('\n', 0, m.start()) + 1)

    for m in RAW_LIST_RE.finditer(stripped):
        open_idx = m.end() - 1
        close_idx = _find_matching_delim(stripped, open_idx, '(', ')')
        if close_idx != -1:
            args = _split_top_level_args(stripped[open_idx + 1:close_idx])
            if len(args) == 1 and args[0].strip():
                hits.add(stripped.count('\n', 0, m.start()) + 1)

    for m in RAW_GET_RE.finditer(stripped):
        open_idx = m.end() - 1
        close_idx = _find_matching_delim(stripped, open_idx, '(', ')')
        if close_idx == -1:
            continue
        args = _split_top_level_args(stripped[open_idx + 1:close_idx])
        if len(args) != 2:
            continue
        recv = _receiver_root_ident(stripped, m.start())
        if recv in NON_CONFIG_GET_RECEIVERS or recv.endswith(('_peers', '_ciphers')):
            continue
        arg2 = args[1].strip()
        if arg2 in ('true', 'false'):
            continue
        if (
            '"' not in arg2
            and not re.search(r'(?<!#)\b(?:if|match)\b|\{', arg2)
            and re.search(r'(?:!=|==|&&|\|\|)', arg2)
        ):
            continue
        hits.add(stripped.count('\n', 0, m.start()) + 1)

    for m in RAW_VALUES_RE.finditer(stripped):
        hits.add(stripped.count('\n', 0, m.start()) + 1)

    return sorted(hits)


def find_rubylight_family_gating(rel_path, text):
    """Return sorted 1-based line numbers of Family::Rubylight / Family.RUBYLIGHT in non-test code."""
    if 'Rubylight' not in text and 'RUBYLIGHT' not in text:
        return []
    if rel_path.endswith('.rs'):
        stripped = strip_cfg_test_blocks(strip_rust_comments_and_strings(text))
        pattern = RUST_FAMILY_RE
    else:
        suffix = pathlib.PurePosixPath(rel_path).suffix
        stripped = strip_java_kt(text, suffix=suffix, strip_comments=True, strip_strings=True)
        pattern = JAVA_FAMILY_RE
    return sorted({stripped.count('\n', 0, m.start()) + 1 for m in pattern.finditer(stripped)})


def _skip_preceding_rust_attrs(code, pos):
    """Walk backward from pos across whitespace and outer #[...] attributes, returning the earliest offset."""
    while True:
        j = pos - 1
        while j >= 0 and code[j].isspace():
            j -= 1
        if j < 0 or code[j] != ']':
            return pos
        # Find matching '[' backwards.
        depth = 1
        k = j - 1
        while k >= 0 and depth > 0:
            if code[k] == ']':
                depth += 1
            elif code[k] == '[':
                depth -= 1
            k -= 1
        # Check for '#' immediately before '[' (and not '#!' inner attr).
        if depth != 0 or k < 0 or code[k] != '#' or (k > 0 and code[k - 1] == '#'):
            return pos
        pos = k


def find_unannotated_cfg_unix(text):
    """Return sorted 1-based line numbers of cfg(...unix...) lacking an OS-intent // comment."""
    if 'unix' not in text:
        return []
    code = strip_rust_comments_and_strings(text)
    raw_lines = text.splitlines()
    unannotated = []

    for m in CFG_UNIX_RE.finditer(code):
        open_idx = m.end() - 1
        close_idx = _find_matching_delim(code, open_idx, '(', ')')
        if close_idx == -1:
            continue
        inner = code[open_idx + 1:close_idx]
        if not UNIX_WORD_RE.search(inner):
            continue

        cfg_line = code.count('\n', 0, m.start()) + 1
        is_macro = code[m.start():m.end()].lstrip().startswith('cfg!')

        top_offset = _skip_preceding_rust_attrs(code, m.start())
        top_line = code.count('\n', 0, top_offset) + 1

        comments = []
        idx = top_line - 2
        while idx >= 0:
            s = raw_lines[idx].strip()
            if s.startswith('///'):
                idx -= 1
            elif s.startswith('//') and not s.startswith('//!'):
                comments.append(s[2:].strip())
                idx -= 1
            else:
                break
        comment_text = ' '.join(reversed(comments))
        annotated = bool(comment_text and OS_INTENT_RE.search(comment_text))

        if not annotated and is_macro and 1 <= cfg_line <= len(raw_lines):
            # For inline cfg!(unix), also allow a trailing non-doc // comment on the same line.
            line_start = text.rfind('\n', 0, m.start()) + 1
            line_end = text.find('\n', m.start())
            if line_end == -1:
                line_end = len(text)
            raw_line = text[line_start:line_end]
            code_line = code[line_start:line_end]
            # Find where real code differs from raw line trailing comment.
            for pos in range(len(raw_line) - 1):
                if raw_line[pos:pos + 2] == '//' and code_line[pos:pos + 2] == '  ':
                    trailing = raw_line[pos + 2:]
                    if not trailing.startswith(('/', '!')) and OS_INTENT_RE.search(trailing):
                        annotated = True
                    break

        if not annotated:
            unannotated.append(cfg_line)

    return unannotated


def compute_baselines(root):
    """Compute exact arch_baselines payload from the current tree."""
    files = discover_files(root)
    missing_rust = []
    missing_java_kt = []
    raw_config = {}
    family_allowed = {}
    family_legacy = {}
    cfg_unix = {}

    allowed_family_files = {
        'client/protocol/policy/src/http/capabilities.rs',
        'client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java',
        'client/protocol/desktop-ui/src/host_page.rs',
    }

    for rel in files:
        text = (root / rel).read_text(encoding='utf-8', errors='replace')

        if rel.endswith('.rs'):
            if not has_rust_module_header(text):
                missing_rust.append(rel)
        elif rel.endswith(('.java', '.kt')):
            suffix = pathlib.PurePosixPath(rel).suffix
            if not has_java_kt_module_header(text, suffix):
                missing_java_kt.append(rel)

        if (rel == 'rust/host/stream/src/stream.rs' or rel.startswith('rust/host/stream/src/stream/')) \
                and rel != 'rust/host/stream/src/stream/config.rs' and rel.endswith('.rs') \
                and not is_rust_test_file(rel):
            hits = find_raw_config_reads(text)
            if hits:
                raw_config[rel] = len(hits)

        if rel.startswith('client/') and rel.endswith(('.rs', '.java', '.kt')) and not is_client_test_file(rel):
            hits = find_rubylight_family_gating(rel, text)
            if hits:
                if rel in allowed_family_files:
                    family_allowed[rel] = len(hits)
                else:
                    family_legacy[rel] = len(hits)

        if rel.startswith(('client/protocol/desktop/', 'client/protocol/desktop-config/', 'client/protocol/desktop-ui/', 'client/protocol/desktop-media/')) and rel.endswith('.rs'):
            hits = find_unannotated_cfg_unix(text)
            if hits:
                cfg_unix[rel] = len(hits)

    arch_baselines = {
        'missing_headers': {
            'rust': sorted(missing_rust),
            'java_kt': sorted(missing_java_kt),
        },
        'raw_config_read': dict(sorted(raw_config.items())),
        'rubylight_family_gating': {
            'allowed': dict(sorted(family_allowed.items())),
            'legacy': dict(sorted(family_legacy.items())),
        },
        'unannotated_cfg_unix': dict(sorted(cfg_unix.items())),
    }
    return arch_baselines


def load_json(path, default):
    if not path.is_file():
        return default
    return json.loads(path.read_text(encoding='utf-8'))


def check_repo(root):
    """Run all architecture checks under root; return sorted list of (rel_path, line, rule_id, message)."""
    root = pathlib.Path(root)
    baselines = load_json(root / 'tools/arch_baselines.json', {})

    missing_hdr = baselines.get('missing_headers', {})
    missing_rust_allow = set(missing_hdr.get('rust', []))
    missing_java_allow = set(missing_hdr.get('java_kt', []))

    raw_config_allow = baselines.get('raw_config_read', {})
    family_cfg = baselines.get('rubylight_family_gating', {})
    family_allow = {}
    family_allow.update(family_cfg.get('allowed', {}))
    family_allow.update(family_cfg.get('legacy', {}))
    cfg_unix_allow = baselines.get('unannotated_cfg_unix', {})

    violations = []
    for rel in discover_files(root):
        text = (root / rel).read_text(encoding='utf-8', errors='replace')

        # 1. Module headers
        if rel.endswith('.rs'):
            if rel not in missing_rust_allow and not has_rust_module_header(text):
                violations.append((rel, 1, 'missing-module-header', 'missing //! module header'))
        elif rel.endswith(('.java', '.kt')):
            suffix = pathlib.PurePosixPath(rel).suffix
            if rel not in missing_java_allow and not has_java_kt_module_header(text, suffix):
                violations.append((rel, 1, 'missing-module-header', 'missing /** module Javadoc header'))

        # 2. Rule 1: raw-config-read
        if (rel == 'rust/host/stream/src/stream.rs' or rel.startswith('rust/host/stream/src/stream/')) \
                and rel != 'rust/host/stream/src/stream/config.rs' and rel.endswith('.rs') \
                and not is_rust_test_file(rel):
            hits = find_raw_config_reads(text)
            limit = int(raw_config_allow.get(rel, 0))
            if len(hits) > limit:
                for line_no in hits[limit:]:
                    violations.append((
                        rel,
                        line_no,
                        'raw-config-read',
                        f'raw Config read outside stream/config.rs ({len(hits)} > baseline {limit})',
                    ))

        # 3. Rule 2: rubylight-family-gating
        if rel.startswith('client/') and rel.endswith(('.rs', '.java', '.kt')) and not is_client_test_file(rel):
            hits = find_rubylight_family_gating(rel, text)
            limit = int(family_allow.get(rel, 0))
            if len(hits) > limit:
                for line_no in hits[limit:]:
                    violations.append((
                        rel,
                        line_no,
                        'rubylight-family-gating',
                        f'gate on HostCapabilities instead of Family::Rubylight ({len(hits)} > baseline {limit})',
                    ))

        # 4. Rule 3: unannotated-cfg-unix
        if rel.startswith(('client/protocol/desktop/', 'client/protocol/desktop-config/', 'client/protocol/desktop-ui/', 'client/protocol/desktop-media/')) and rel.endswith('.rs'):
            hits = find_unannotated_cfg_unix(text)
            limit = int(cfg_unix_allow.get(rel, 0))
            if len(hits) > limit:
                for line_no in hits[limit:]:
                    violations.append((
                        rel,
                        line_no,
                        'unannotated-cfg-unix',
                        f'cfg(unix) requires a preceding // comment with OS intent ({len(hits)} > baseline {limit})',
                    ))

    return sorted(violations, key=lambda item: (item[0], item[1], item[2], item[3]))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('--root', type=pathlib.Path, default=ROOT, help='Repository root to check')
    parser.add_argument('--write-baselines', action='store_true',
                        help='Regenerate tools/arch_baselines.json from --root')
    args = parser.parse_args(argv)

    root = args.root.resolve()
    if args.write_baselines:
        arch_baselines = compute_baselines(root)
        (root / 'tools/arch_baselines.json').write_text(
            json.dumps(arch_baselines, indent=2) + '\n', encoding='utf-8', newline='\n'
        )
        print(
            f'Wrote baselines: {len(arch_baselines["missing_headers"]["rust"])} rust + '
            f'{len(arch_baselines["missing_headers"]["java_kt"])} java/kt missing headers'
        )
        return 0

    violations = check_repo(root)
    for rel, line_no, rule_id, message in violations:
        print(f'{rel}:{line_no}: {rule_id}: {message}')
    if violations:
        print(f'ERROR: arch_check failed ({len(violations)} violation(s))')
        return 1
    print('arch_check passed (0 violations)')
    return 0


if __name__ == '__main__':
    sys.exit(main())
