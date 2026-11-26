"""Generated documentation and host settings catalog: regenerate, check, and merge without conflicts.

python tools/generated_docs.py            # regenerate every generated target from this worktree
python tools/generated_docs.py --check    # fail when one is stale (the file is left as it was)
python tools/generated_docs.py install    # register the merge driver in this clone's git config
python tools/generated_docs.py merge %O %A %B %P   # the merge driver git calls

Host settings live in rust/core/config/settings.toml (parsed with stdlib tomllib). From that
single source of truth this tool generates the Rust config catalog, retired tables, fallback
console tables, web console schema modules, and the All settings tables in docs/configuration.md.

Two branches that both change what a generated file or table is made from conflict in the
generated output, never in what it is made from. The merge driver (.gitattributes:
merge=rubylight-generated) merges the file normally and, when every conflict lies inside the
generated block (or the whole file for whole-file generated modules), takes this side's output
and reports success; run this tool afterwards to regenerate from the merged sources. A conflict
in hand-written text around a generated block stays a normal conflict.
"""
import argparse, json, pathlib, re, shutil, subprocess, sys, tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]
DRIVER = 'rubylight-generated'
SETTINGS_TOML = 'rust/core/config/settings.toml'
HOST_SETTINGS_INPUTS = (SETTINGS_TOML, 'tools/generated_docs.py')
NIGHTJAR_SETTINGS_TOML = 'client/protocol/policy/settings.toml'
NIGHTJAR_SETTINGS_INPUTS = (NIGHTJAR_SETTINGS_TOML, 'client/scripts/generate-settings.py')
SETTINGS_MARKERS = ('<!-- BEGIN GENERATED SETTINGS -->', '<!-- END GENERATED SETTINGS -->')


class Doc:
    def __init__(self, path, command=None, cwd=None, markers=None, inputs=(), render=None):
        self.path = path
        self.command = command
        self.cwd = cwd
        self.markers = markers
        self.inputs = inputs
        self.render = render


# ---------- Host settings catalog loader & helpers ----------

def load_catalog(root=ROOT):
    data = tomllib.loads((root / SETTINGS_TOML).read_text(encoding='utf-8'))
    by_key = {entry['key']: entry for entry in data['setting']}
    data['_by_key'] = by_key
    return data


def rs_str(value):
    return json.dumps(value, ensure_ascii=False)


def ts_str(value):
    # Prefer single quotes when there are no single quotes or newlines, matching existing TS style.
    if "'" not in value and '\n' not in value and '\r' not in value:
        escaped = value.replace('\\', '\\\\')
        return f"'{escaped}'"
    return json.dumps(value, ensure_ascii=False)


def ts_val(value):
    if isinstance(value, bool):
        return 'true' if value else 'false'
    if isinstance(value, (int, float)):
        return str(value)
    if isinstance(value, str):
        return ts_str(value)
    if isinstance(value, list):
        return '[' + ', '.join(ts_val(v) for v in value) + ']'
    if isinstance(value, dict):
        items = ', '.join(f'{ts_prop(k)}: {ts_val(v)}' for k, v in value.items())
        return '{ ' + items + ' }'
    raise TypeError(f'Unsupported TS value: {type(value)} {value!r}')


def ts_prop(key):
    if re.fullmatch(r'[A-Za-z_$][A-Za-z0-9_$]*', key):
        return key
    return ts_str(key)


def compile_pred(rule):
    """Compile a declarative TOML visibility/requirement rule into a TS expression over `values`."""
    if 'ref' in rule:
        return f'{rule["ref"]}(values)'
    if 'on' in rule:
        return f'on(values, {ts_str(rule["on"])}, {ts_val(rule["default"])})'
    if 'text_eq' in rule:
        return f'text(values, {ts_str(rule["text_eq"])}, {ts_str(rule["default"])}) === {ts_str(rule["value"])}'
    if 'text_neq' in rule:
        return f'text(values, {ts_str(rule["text_neq"])}, {ts_str(rule["default"])}) !== {ts_str(rule["value"])}'
    if 'read_eq' in rule:
        return f'read(values, {ts_str(rule["read_eq"])}, {ts_str(rule["default"])}) === {ts_str(rule["value"])}'
    if 'read_neq' in rule:
        return f'read(values, {ts_str(rule["read_neq"])}, {ts_str(rule["default"])}) !== {ts_str(rule["value"])}'
    if 'raw_eq' in rule:
        return f'values.{rule["raw_eq"]} === {ts_str(rule["value"])}'
    if 'raw_neq' in rule:
        return f'(values.{rule["raw_neq"]} ?? {ts_str(rule["default"])}) !== {ts_str(rule["value"])}'
    if 'str_trim_eq' in rule:
        return f'String(values.{rule["str_trim_eq"]} ?? {ts_str(rule["default"])}).trim() === {ts_str(rule["value"])}'
    if 'str_trim_neq' in rule:
        return f'String(values.{rule["str_trim_neq"]} ?? {ts_str(rule["default"])}).trim() !== {ts_str(rule["value"])}'
    if 'read_in' in rule:
        expr = f'read(values, {ts_str(rule["read_in"])}, {ts_str(rule["default"])})'
        if rule.get('lower'):
            expr += '.toLowerCase()'
        return f'{ts_val(rule["values"])}.includes({expr})'
    if 'read_not_in' in rule:
        expr = f'read(values, {ts_str(rule["read_not_in"])}, {ts_str(rule["default"])})'
        if rule.get('lower'):
            expr += '.toLowerCase()'
        return f'!{ts_val(rule["values"])}.includes({expr})'
    if 'str_trim_not_in' in rule:
        expr = f'String(values.{rule["str_trim_not_in"]} ?? \'\').trim()'
        if rule.get('lower'):
            expr += '.toLowerCase()'
        return f'!{ts_val(rule["values"])}.includes({expr})'
    if 'gt' in rule:
        return f'Number(values.{rule["gt"]} ?? {rule["default"]}) > {rule["value"]}'
    if 'all' in rule:
        return ' && '.join(compile_pred(sub) for sub in rule['all'])
    if 'any' in rule:
        return ' || '.join(compile_pred(sub) for sub in rule['any'])
    raise ValueError(f'Unknown predicate rule: {rule!r}')


def compile_pred_fn(rule, used_rules):
    collect_refs(rule, used_rules)
    if set(rule.keys()) == {'ref'}:
        return rule['ref']
    return f'(values) => {compile_pred(rule)}'


def compile_caution_fn(rule, used_rules):
    if 'ref' in rule:
        used_rules.add(rule['ref'])
        if rule['ref'] == 'amfUsageCaution':
            used_rules.add('read')
            return "(values) => amfUsageCaution(read(values, 'amd_usage', 'ultralowlatency'))"
        return rule['ref']
    collect_refs(rule['when'], used_rules)
    return f'(values) => ({compile_pred(rule["when"])} ? {ts_str(rule["message"])} : undefined)'


def collect_refs(rule, used):
    if 'ref' in rule:
        used.add(rule['ref'])
    elif 'on' in rule:
        used.add('on')
    elif 'text_eq' in rule or 'text_neq' in rule:
        used.add('text')
    elif any(k in rule for k in ('read_eq', 'read_neq', 'read_in', 'read_not_in')):
        used.add('read')
    elif 'all' in rule:
        for sub in rule['all']:
            collect_refs(sub, used)
    elif 'any' in rule:
        for sub in rule['any']:
            collect_refs(sub, used)


# ---------- Rust generators ----------

def render_rust_catalog(cat, _existing):
    by_key = cat['_by_key']
    rust = cat['rust']
    lines = [
        '//! Generated host settings catalog: overrides, 3.0 list, restart keys, typed accessors, and save validation.',
        '//! Generated from `rust/core/config/settings.toml` by `tools/generated_docs.py`; do not edit by hand.',
        '',
        'use super::{',
        '    BitrateAdaptation, Config, DisplayAdjust, MAX_STREAM_RESOLUTION, RemoveRemoteMonitor,',
        '    TouchMode, UpdatesMode, invalid, parse_boolean, parse_integer, parse_max_stream_resolution,',
        '    unquote,',
        '};',
        'use anyhow::{Result, bail};',
        'use std::path::PathBuf;',
        '',
        '/// Hardware metrics the host may report (`hardware_metrics_selection`).',
        'pub const HARDWARE_METRICS: &[&str] = &[',
    ]
    for metric in rust['hardware_metrics']:
        lines.append(f'    {rs_str(metric)},')
    lines.extend([
        '];',
        '',
        '/// Keys a device or an app may override (`override_allowed`).',
        'pub const OVERRIDE_ALLOWED: &[&str] = &[',
    ])
    for key in rust['override_order']:
        assert by_key[key].get('override_allowed'), f'{key} in override_order but override_allowed is false'
        lines.append(f'    {rs_str(key)},')
    lines.extend([
        '];',
        '',
        '/// Settings read only when the host starts (`restart_required`).',
        'pub const RESTART_KEYS: &[&str] = &[',
    ])
    for key in rust['restart_order']:
        assert by_key[key].get('restart'), f'{key} in restart_order but restart is false'
        lines.append(f'    {rs_str(key)},')
    lines.extend([
        '];',
        '',
        '/// Every 3.0 setting with its default, as `sunshine.conf` would spell it.',
        'pub const SETTINGS_30: &[(&str, &str)] = &[',
    ])
    for key in rust['settings_30_order']:
        entry = by_key[key]
        assert entry.get('settings_30'), f'{key} in settings_30_order but settings_30 is false'
        lines.append(f'    ({rs_str(key)}, {rs_str(entry["settings_30_default"])}),')
    lines.extend([
        '];',
        '',
        '/// Validate changed settings before saving (`Config::update_with`).',
        'pub(super) fn validate_changed(next: &Config, changed: &dyn Fn(&str) -> bool) -> Result<()> {',
    ])
    for entry in cat['setting']:
        v = entry.get('validate')
        if not v:
            continue
        key = entry['key']
        kind = v['kind']
        if kind == 'int_range':
            lo, hi = v['min'], v['max']
            cond = f'        && !parse_integer(&next.values[{rs_str(key)}]).is_some_and(|n| ({lo}..={hi}).contains(&n))'
            lines.append(f'    if changed({rs_str(key)})')
            if len(cond) <= 100:
                lines.append(cond)
            else:
                lines.append(f'        && !parse_integer(&next.values[{rs_str(key)}])')
                lines.append(f'            .is_some_and(|n| ({lo}..={hi}).contains(&n))')
            lines.extend([
                '    {',
                f'        bail!({rs_str(v["error"])});',
                '    }',
            ])
        elif kind == 'max_stream_resolution':
            lines.extend([
                '    if changed(MAX_STREAM_RESOLUTION) {',
                '        parse_max_stream_resolution(&next.values[MAX_STREAM_RESOLUTION])?;',
                '    }',
            ])
        elif kind == 'bool':
            line = f'    if changed({rs_str(key)}) && parse_boolean(&next.values[{rs_str(key)}]).is_none() {{'
            if len(line) <= 100:
                lines.append(line)
            else:
                lines.extend([
                    f'    if changed({rs_str(key)})',
                    f'        && parse_boolean(&next.values[{rs_str(key)}]).is_none()',
                    '    {',
                ])
            lines.extend([
                f'        bail!({rs_str(v["error"])});',
                '    }',
            ])
        elif kind == 'zero_or_range':
            lo, hi = v['min'], v['max']
            lines.extend([
                f'    if changed({rs_str(key)})',
                f'        && !parse_integer(&next.values[{rs_str(key)}])',
                f'            .is_some_and(|n| n == 0 || ({lo}..={hi}).contains(&n))',
                '    {',
                f'        bail!({rs_str(v["error"])});',
                '    }',
            ])
        elif kind == 'hardware_metrics_list':
            lines.extend([
                f'    if changed({rs_str(key)}) {{',
                f'        for metric in next.list({rs_str(key)}) {{',
                '            if !HARDWARE_METRICS.contains(&metric.as_str()) {',
                '                bail!("unknown hardware metric: {metric}");',
                '            }',
                '        }',
                '    }',
            ])
        else:
            raise ValueError(f'Unknown validation kind: {kind}')
    lines.extend([
        '    Ok(())',
        '}',
        '',
        'impl Config {',
        '    fn check_range(',
        '        &self,',
        '        key: &str,',
        '        value: i64,',
        '        default: i64,',
        '        range: std::ops::RangeInclusive<i64>,',
        '    ) -> i64 {',
        '        if range.contains(&value) {',
        '            value',
        '        } else {',
        '            invalid(key, self.get(key, ""));',
        '            default',
        '        }',
        '    }',
    ])
    for entry in cat['setting']:
        acc = entry.get('rust_accessor')
        if not acc:
            continue
        key = entry['key']
        method = acc['method']
        kind = acc['kind']
        doc = acc['doc']
        lines.append(f'    /// {doc}')
        if kind == 'bool':
            df = 'true' if entry['default'] else 'false'
            lines.extend([
                f'    pub fn {method}(&self) -> bool {{',
                f'        self.boolean({rs_str(key)}, {df})',
                '    }',
            ])
        elif kind == 'clamped_int':
            df = entry['default']
            lo, hi, ret = acc['min'], acc['max'], acc['ret']
            expr = f'        self.integer({rs_str(key)}, {df}).clamp({lo}, {hi}) as {ret}'
            lines.append(f'    pub fn {method}(&self) -> {ret} {{')
            if len(expr) <= 72:
                lines.append(expr)
            else:
                lines.append(f'        self.integer({rs_str(key)}, {df})')
                lines.append(f'            .clamp({lo}, {hi}) as {ret}')
            lines.append('    }')
        elif kind == 'ranged_int':
            df = entry['default']
            lo, hi, ret = acc['min'], acc['max'], acc['ret']
            lines.extend([
                f'    pub fn {method}(&self) -> {ret} {{',
                '        self.check_range(',
                f'            {rs_str(key)},',
                f'            self.integer({rs_str(key)}, {df}),',
                f'            {df},',
                f'            {lo}..={hi},',
                f'        ) as {ret}',
                '    }',
            ])
        elif kind == 'mib_bytes':
            df = entry['default']
            lo = acc['min']
            hi_expr = acc.get('max_expr', str(acc.get('max', '')))
            lines.extend([
                f'    pub fn {method}(&self) -> u64 {{',
                '        const MIB: i64 = 1024 * 1024;',
                '        let mib = self.check_range(',
                f'            {rs_str(key)},',
                f'            self.integer({rs_str(key)}, {df}),',
                f'            {df},',
                f'            {lo}..={hi_expr.replace("(1024 * 1024)", "MIB")},',
                '        );',
                '        (mib * MIB) as u64',
                '    }',
            ])
        elif kind == 'set_int':
            df = entry['default']
            ret = acc['ret']
            pat = ' | '.join(str(x) for x in acc['allowed'])
            lines.extend([
                f'    pub fn {method}(&self) -> {ret} {{',
                f'        let value = self.integer({rs_str(key)}, {df});',
                f'        if matches!(value, {pat}) {{',
                f'            value as {ret}',
                '        } else {',
                f'            invalid({rs_str(key)}, self.get({rs_str(key)}, ""));',
                f'            {df}',
                '        }',
                '    }',
            ])
        elif kind == 'opt_path':
            lines.extend([
                f'    pub fn {method}(&self) -> Option<PathBuf> {{',
                f'        let value = unquote(self.get({rs_str(key)}, "").trim()).trim();',
                '        (!value.is_empty()).then(|| PathBuf::from(value))',
                '    }',
            ])
        elif kind in ('enum_ci', 'enum_exact'):
            df = entry['default']
            ret = acc['ret']
            default_variant = next(v for name, v in acc['variants'] if name == df)
            lines.append(f'    pub fn {method}(&self) -> {ret} {{')
            if kind == 'enum_ci':
                lines.append(f'        let value = self.get({rs_str(key)}, {rs_str(df)});')
                lines.append('        match unquote(value.trim()).to_ascii_lowercase().as_str() {')
            else:
                lines.append(f'        let value = unquote(self.get({rs_str(key)}, {rs_str(df)}).trim()).trim();')
                lines.append('        match value {')
            for name, variant in acc['variants']:
                lines.append(f'            {rs_str(name)} => {variant},')
            lines.extend([
                '            _ => {',
                f'                invalid({rs_str(key)}, value);',
                f'                {default_variant}',
                '            }',
                '        }',
                '    }',
            ])
        else:
            raise ValueError(f'Unknown rust_accessor kind: {kind}')
    lines.append('}')
    return '\n'.join(lines) + '\n'


def render_rust_retired(cat, _existing):
    ret = cat['retired']
    lines = [
        '//! Generated tables of retired, renamed, removed, and folded host settings.',
        '//! Generated from `rust/core/config/settings.toml` by `tools/generated_docs.py`; do not edit by hand.',
        '',
        '/// Setting values Rubylight no longer offers, and what replaces each.',
        'pub const RETIRED_VALUES: &[(&str, &str, &str)] = &[',
    ]
    for item in ret['values']:
        lines.append(f'    ({rs_str(item["key"])}, {rs_str(item["old"])}, {rs_str(item["current"])}),')
    lines.extend([
        '];',
        '',
        '/// Settings saved under an earlier name and the name the host reads now.',
        'pub const RENAMED_KEYS: &[(&str, &str)] = &[',
    ])
    for item in ret['renamed']:
        pair = f'    ({rs_str(item["old"])}, {rs_str(item["current"])}),'
        if len(pair) <= 70:
            lines.append(pair)
        else:
            lines.extend([
                '    (',
                f'        {rs_str(item["old"])},',
                f'        {rs_str(item["current"])},',
                '    ),',
            ])
    lines.extend([
        '];',
        '',
        '/// Launchers whose sync options earlier releases kept under per-launcher keys.',
        'pub const LIBRARY_PREFIXES: &[&str] = &[',
    ])
    for p in ret['library_prefixes']:
        lines.append(f'    {rs_str(p)},')
    lines.extend([
        '];',
        '',
        '/// Launchers that record when a game was played.',
        'pub const LIBRARY_HISTORY_PREFIXES: &[&str] = &['
        + ', '.join(rs_str(p) for p in ret['library_history_prefixes']) + '];',
        '',
        '/// Shared Library sync options: `(name, default, prefixes)`.',
        'pub const LIBRARY_SYNC_OPTIONS: &[(&str, &str, &[&str])] = &[',
    ])
    for item in ret['library_sync_options']:
        ref = 'LIBRARY_PREFIXES' if item['scope'] == 'all' else 'LIBRARY_HISTORY_PREFIXES'
        lines.append(f'    ({rs_str(item["option"])}, {rs_str(item["default"])}, {ref}),')
    lines.extend([
        '];',
        '',
        '/// Settings of features Rubylight removed, dropped silently on load.',
        'pub const REMOVED_KEYS: &[&str] = &[',
    ])
    for k in ret['removed']:
        lines.append(f'    {rs_str(k)},')
    lines.extend([
        '];',
        '',
        '/// Prefixes of removed Quick Sync and software encoders.',
        'pub const REMOVED_PREFIXES: &[&str] = &['
        + ', '.join(rs_str(p) for p in ret['removed_prefixes']) + '];',
        '',
        '/// Settings folded into another setting.',
        'pub const FOLDED_KEYS: &[&str] = &[',
    ])
    for k in ret['folded']:
        lines.append(f'    {rs_str(k)},')
    lines.extend([
        '];',
        '',
        '/// Minimum QP keys and their old defaults when the retired switch was on.',
        'pub const MIN_QP: &[(&str, i64)] = &[',
    ])
    for item in ret['min_qp']:
        lines.append(f'    ({rs_str(item["key"])}, {item["default"]}),')
    lines.extend([
        '];',
        '',
        '/// App record keys Rubylight no longer reads.',
        'pub const REMOVED_APP_KEYS: &[&str] = &['
        + ', '.join(rs_str(k) for k in ret['removed_app_keys']) + '];',
        '',
        '/// App switches saved under an earlier name.',
    ])
    app_pairs = [f'({rs_str(item["old"])}, {rs_str(item["current"])})' for item in ret['renamed_app_switches']]
    single = 'pub const RENAMED_APP_SWITCHES: &[(&str, &str)] = &[' + ', '.join(app_pairs) + '];'
    if len(single) <= 100:
        lines.append(single)
    else:
        lines.append('pub const RENAMED_APP_SWITCHES: &[(&str, &str)] = &[')
        for p in app_pairs:
            lines.append(f'    {p},')
        lines.append('];')
    return '\n'.join(lines) + '\n'


def _format_console_ctrl(entry):
    ctrl = entry['control']
    if ctrl == 'bool':
        return 'Bool'
    if ctrl == 'text':
        return 'Text'
    if ctrl == 'number':
        return 'Number'
    if ctrl == 'json':
        return 'Json'
    if ctrl == 'grips':
        return 'Choice(GRIPS)'
    if ctrl == 'choice':
        return 'Choice(&[' + ', '.join(rs_str(c) for c in entry['choices']) + '])'
    raise ValueError(f'Unknown console control: {ctrl}')


def _emit_console_macro_entries(lines, rows):
    cur_group = None
    for row in rows:
        grp = row['group']
        if grp != cur_group:
            if cur_group is not None:
                lines.append('    ]')
            lines.append(f'    {rs_str(grp)} => [')
            cur_group = grp
        lines.append(f'        {rs_str(row["key"])}, {rs_str(row["label"])}, {_format_console_ctrl(row)};')
    if cur_group is not None:
        lines.append('    ]')


def render_rust_console_global(cat, _existing):
    by_key = cat['_by_key']
    con = cat['console']
    lines = [
        '//! Generated fallback-console global setting catalog (`Control`, `Setting`, `GRIPS`, `GLOBAL`).',
        '//! Generated from `rust/core/config/settings.toml` by `tools/generated_docs.py`; do not edit by hand.',
        '',
        '#[derive(Clone, Copy)]',
        'pub enum Control {',
        '    Bool,',
        '    Choice(&\'static [&\'static str]),',
        '    Text,',
        '    Number,',
        '    Json,',
        '}',
        '',
        'pub struct Setting {',
        '    pub key: &\'static str,',
        '    pub label: &\'static str,',
        '    pub group: &\'static str,',
        '    pub control: Control,',
        '}',
        '',
        'macro_rules! console_settings {',
        '    ($($group:literal => [$($key:literal, $label:literal, $control:expr;)*])*) => {',
        '        &[$($(Setting { key: $key, label: $label, group: $group, control: $control },)*)*]',
        '    };',
        '}',
        '',
        'use Control::*;',
        'pub(super) use console_settings;',
        '',
        '/// What a back grip can press (`back_grip_*`).',
        'pub const GRIPS: &[&str] = &[',
    ]
    for g in con['grips']:
        lines.append(f'    {rs_str(g)},')
    lines.extend([
        '];',
        '',
        'pub const GLOBAL: &[Setting] = console_settings! {',
    ])
    global_rows = []
    for key in con['global_order']:
        c = by_key[key]['console']
        global_rows.append({'key': key, **c})
    _emit_console_macro_entries(lines, global_rows)
    lines.append('};')
    return '\n'.join(lines) + '\n'


def render_rust_console_layers(cat, _existing):
    con = cat['console']
    lines = [
        '//! Generated fallback-console app/device setting catalogs and choice labels (`APP`, `CLIENT`, `choice_label`).',
        '//! Generated from `rust/core/config/settings.toml` by `tools/generated_docs.py`; do not edit by hand.',
        '',
        'use super::console_global_gen::{Control::*, Setting, console_settings};',
        '',
        'pub const APP: &[Setting] = console_settings! {',
    ]
    _emit_console_macro_entries(lines, con['app'])
    lines.extend([
        '};',
        '',
        'pub const CLIENT: &[Setting] = console_settings! {',
    ])
    _emit_console_macro_entries(lines, con['client'])
    lines.extend([
        '};',
        '',
        '/// Human-readable label for a fallback console select choice, if catalogued.',
        'pub fn choice_label(key: &str, choice: &str) -> Option<&\'static str> {',
        '    match (key, choice) {',
    ])
    for item in con['key_choice_labels']:
        arm = f'        ({rs_str(item["key"])}, {rs_str(item["choice"])}) => Some({rs_str(item["label"])}),'
        if len(arm) <= 100:
            lines.append(arm)
        else:
            lines.extend([
                f'        ({rs_str(item["key"])}, {rs_str(item["choice"])}) => {{',
                f'            Some({rs_str(item["label"])})',
                '        }',
            ])
    for item in con['choice_labels']:
        arm = f'        (_, {rs_str(item["choice"])}) => Some({rs_str(item["label"])}),'
        if len(arm) <= 100:
            lines.append(arm)
        else:
            lines.extend([
                f'        (_, {rs_str(item["choice"])}) => {{',
                f'            Some({rs_str(item["label"])})',
                '        }',
            ])
    lines.extend([
        '        _ => None,',
        '    }',
        '}',
    ])
    return '\n'.join(lines) + '\n'


# ---------- Web TypeScript generators ----------

def render_web_rules(cat, _existing):
    web = cat['web']
    lines = [
        '// Generated from rust/core/config/settings.toml by tools/generated_docs.py; do not edit by hand.',
        "import type { ConfigValue } from '../api';",
        "import { NATIVE_ENCODERS, VIRTUAL_DISPLAY_OS, offersEnhancement, type Aliases, type Option } from '../settings-types';",
        "import { toList } from '../../components/settings/values';",
        '',
        'export type Values = Record<string, ConfigValue>;',
        '',
        '/** A boolean read as the host reads it; anything unrecognised keeps the default. */',
        'export function on(values: Values, key: string, fallback: boolean): boolean {',
        '  const value = values[key];',
        "  if (typeof value === 'boolean') return value;",
        "  const word = String(value ?? '').trim().replace(/^\"(.*)\"$/, '$1').toLowerCase();",
        "  if (['true', 'yes', '1', 'enable', 'enabled', 'on'].includes(word)) return true;",
        "  if (['false', 'no', '0', 'disable', 'disabled', 'off'].includes(word)) return false;",
        '  return fallback;',
        '}',
        '',
        '/** A text setting read as basics.ts reads it: blank means the default. */',
        'export function text(values: Values, key: string, fallback: string): string {',
        '  const value = values[key];',
        "  const raw = value === null || value === undefined ? '' : String(value);",
        "  return raw.trim() === '' ? fallback : raw;",
        '}',
        '',
        '/** A text setting trimmed as video.ts/display.ts read it: unset or blank means the default. */',
        'export function read(values: Values, key: string, fallback: string): string {',
        '  const value = values[key];',
        '  if (value === undefined || value === null) return fallback;',
        '  const trimmed = String(value).trim();',
        "  return trimmed === '' ? fallback : trimmed;",
        '}',
        '',
        "export const streamsAudio = (values: Values): boolean => on(values, 'stream_audio', true);",
        "export const systemAudio = (values: Values): boolean => streamsAudio(values) && text(values, 'audio_capture_mode', 'system') === 'system';",
        "export const keyboardOn = (values: Values): boolean => on(values, 'keyboard', true);",
        "export const mouseOn = (values: Values): boolean => on(values, 'mouse', true);",
        "export const controllerOn = (values: Values): boolean => on(values, 'controller', true);",
        "export const couchMenuOn = (values: Values): boolean => on(values, 'couch_menu', true);",
        "export const rumbleOn = (values: Values): boolean => controllerOn(values) && on(values, 'forward_rumble', true);",
        "export const audioHapticsOn = (values: Values): boolean => rumbleOn(values) && on(values, 'audio_haptics', false);",
        '',
        'export const streamsWindow = (values: Values): boolean =>',
        "  ['window', 'app', 'true', 'on', 'yes', '1', 'enabled', 'window_focus', 'focus'].includes(read(values, 'capture_window', 'display').trim().toLowerCase());",
        "export const gridPacing = (values: Values): boolean => ['grid', 'fixed'].includes(read(values, 'frame_pacing', 'arrival').toLowerCase());",
        '',
        'function encoder(values: Values): string {',
        "  const name = read(values, 'encoder', 'auto');",
        '  switch (name) {',
        "    case 'amdvce': case 'amdvce_experimental': case 'amdvce_ffmpeg': case 'amdvce_legacy': return 'amf';",
        "    case 'nvenc_experimental': case 'nvenc_legacy': return 'nvenc';",
        "    case 'amf': case 'nvenc': return name;",
        "    default: return 'auto';",
        '  }',
        '}',
        '',
        'function automatic(values: Values, backend: string, unknown: boolean): boolean {',
        "  if (encoder(values) !== 'auto') return false;",
        '  const native = values[NATIVE_ENCODERS];',
        '  return Array.isArray(native) && native.length > 0 ? native.includes(backend) : unknown;',
        '}',
        '',
        "export const amf = (values: Values): boolean => encoder(values) === 'amf' || automatic(values, 'amf', true);",
        "export const nvenc = (values: Values): boolean => encoder(values) === 'nvenc' || automatic(values, 'nvenc', false);",
        "export const rtxVideo = (values: Values): boolean => offersEnhancement(values, 'rtx_video');",
        "export const rtxHdr = (values: Values): boolean => offersEnhancement(values, 'rtx_hdr');",
        'export const virtualDisplayOs = (values: Values): boolean => values[VIRTUAL_DISPLAY_OS] !== false;',
        "export const VIRTUAL_DISPLAY_OS_GATE = (): string => 'Needs Windows 11 23H2 or later';",
        '',
        'export function rtss(values: Values): boolean {',
        "  const name = read(values, 'frame_limiter_provider', 'auto').toLowerCase().replace(/[^a-z0-9]/g, '');",
        "  return !['none', 'disabled'].includes(name);",
        '}',
        '',
        'export function pyrowaveCustomFec(values: Values): boolean {',
        "  return on(values, 'pyrowave', true) && values.pyrowave_critical_fec_percentage != null && Number(values.pyrowave_critical_fec_percentage) !== Number(values.fec_percentage ?? 20);",
        '}',
        '',
        'export function keyframeLossPacing(values: Values): boolean {',
        "  const pacing = read(values, 'packet_pacing', 'auto');",
        "  return pacing !== 'always' && !(pacing !== 'off' && Number(pacing) > 0);",
        '}',
        '',
        'export function amfUsageCaution(usage: string): string | undefined {',
        '  switch (usage) {',
        "    case 'lowlatency_high_quality':",
        "      return 'Slower than Ultra low latency: about 0.4 ms more per HEVC frame and 0.9 ms per AV1 frame at 1440p120 (RX 7900 XT).';",
        "    case 'auto':",
        '      return "Adds delay: the driver\'s default is its transcoding profile, which leaves the low-latency modes off and queues frames at 4K.";',
        "    case 'transcoding': case 'lowlatency': case 'high_quality':",
        "      return 'Adds delay: this profile queues frames at 4K. Ultra low latency is the fastest.';",
        '    default:',
        '      return undefined;',
        '  }',
        '}',
        '',
        'export function rtssReflexCaution(values: Values): string | undefined {',
        '  const encoders = values[NATIVE_ENCODERS];',
        "  const reflex = read(values, 'rtss_frame_limit_type', 'async').toLowerCase().replace(/[^a-z0-9]/g, '') === 'nvidiareflex';",
        "  return reflex && Array.isArray(encoders) && encoders.length > 0 && !encoders.includes('nvenc')",
        "    ? 'This PC streams without an NVIDIA GPU, so NVIDIA Reflex limits with front edge sync instead.'",
        '    : undefined;',
        '}',
        '',
        "export const steam = (values: Values): boolean => on(values, 'steam_enabled', true);",
        "export const folders = (values: Values): boolean => toList(values.library_local_folders).some((folder) => folder.trim() !== '');",
        "export const playnite = (values: Values): boolean => on(values, 'playnite_enabled', true);",
        "export const sync = (values: Values): boolean => on(values, 'library_auto_sync', true);",
        "export const syncAll = (values: Values): boolean => on(values, 'library_sync_all_installed', true);",
        'export const recent = (values: Values): boolean => !syncAll(values);',
        'export const playniteSync = (values: Values): boolean => playnite(values) && sync(values);',
        'export const playniteRecent = (values: Values): boolean => playniteSync(values) && !syncAll(values);',
        '',
    ]
    for name, opts in web['shared_options'].items():
        lines.append(f'export const {name}: Option[] = [')
        for o in opts:
            lines.append(f'  {{ value: {ts_str(o["value"])}, label: {ts_str(o["label"])} }},')
        lines.append('];')
    lines.append('')
    for name, al in web['shared_aliases'].items():
        lines.append(f'export const {name}: Aliases = {ts_val(al)};')
    return '\n'.join(lines) + '\n'


def render_web_tiers(cat, _existing):
    web = cat['web']
    lines = [
        '// Generated from rust/core/config/settings.toml by tools/generated_docs.py; do not edit by hand.',
        "import type { Setting } from '../settings-types';",
        '',
        'export interface MainSection {',
        '  id: string;',
        '  title: string;',
        '  description: string;',
        '  keys: readonly string[];',
        '}',
        '',
        'export const MAIN_SECTIONS: readonly MainSection[] = [',
    ]
    for sec in web['main_sections']:
        lines.extend([
            '  {',
            f'    id: {ts_str(sec["id"])},',
            f'    title: {ts_str(sec["title"])},',
            f'    description: {ts_str(sec["description"])},',
            f'    keys: {ts_val(sec["keys"])},',
            '  },',
        ])
    lines.extend([
        '];',
        '',
        f'export const UPDATE_KEYS: readonly string[] = {ts_val(web["update_keys"])};',
        'export const MAIN_KEYS: readonly string[] = MAIN_SECTIONS.flatMap((section) => section.keys);',
        f'export const MAIN_BUDGET = {web["main_budget"]};',
        '',
        f'export const ALWAYS_ON: readonly string[] = {ts_val(web["always_on"])};',
        f'export const DIAGNOSTICS: readonly string[] = {ts_val(web["diagnostics"])};',
        f'export const DEBUG_GROUP = {ts_str(web["debug_group"])};',
        f'export const ORIGIN_CATEGORY: Readonly<Record<string, Setting[\'category\']>> = {ts_val(web["origin_category"])};',
        '',
        f'export const TRAILING_GROUPS: ReadonlySet<string> = new Set({ts_val(web["trailing_groups"])});',
        f'export const FOLDED_GROUPS: ReadonlySet<string> = new Set({ts_val(web["folded_groups"])});',
        f'export const GAMES_PAGE_KEYS: readonly string[] = {ts_val(web["games_page_keys"])};',
        'export const GAMES_PAGE_KEY_SET: ReadonlySet<string> = new Set(GAMES_PAGE_KEYS);',
    ])
    return '\n'.join(lines) + '\n'


def render_web_overrides(cat, _existing):
    by_key = cat['_by_key']
    lines = [
        '// Generated from rust/core/config/settings.toml by tools/generated_docs.py; do not edit by hand.',
        '',
        '/** Keys a device or an app may override (`override_allowed`), in the order the host lists them. */',
        'export const OVERRIDE_ALLOWED: readonly string[] = [',
    ]
    for key in cat['rust']['override_order']:
        assert by_key[key].get('override_allowed'), f'{key} in override_order but override_allowed is false'
        lines.append(f'  {ts_str(key)},')
    lines.extend(['];', ''])
    return '\n'.join(lines)


def _render_control_ts(ctrl, used_rules, uses_presets):
    parts = [f'kind: {ts_str(ctrl["kind"])}']
    if 'options_ref' in ctrl:
        ref = ctrl['options_ref']
        if ref == 'stream_presets':
            uses_presets.append(True)
            parts.append('options: presets.map((preset) => ({ value: preset.id, label: preset.label }))')
        else:
            used_rules.add(ref)
            parts.append(f'options: {ref}')
    elif 'options' in ctrl:
        parts.append(f'options: {ts_val(ctrl["options"])}')
    for field in ('fixed', 'min', 'max', 'step', 'unit', 'presets', 'daily', 'placeholder', 'mono', 'browse', 'time', 'pnp'):
        if field in ctrl:
            parts.append(f'{field}: {ts_val(ctrl[field])}')
    return '{ ' + ', '.join(parts) + ' }'


def render_web_part(part_name, export_launcher_groups=False):
    def _render(cat, _existing):
        items = [s for s in cat['setting'] if s['part'] == part_name]
        used_rules = set()
        used_types = set()
        uses_presets = []
        body = []

        for s in items:
            key = s['key']
            is_preset = key == 'stream_preset'
            if is_preset:
                uses_presets.append(True)
            body.append('  {')
            body.append(f'    key: {"SETTING" if is_preset else ts_str(key)},')
            body.append(f'    label: {ts_str(s["label"])},')
            body.append(f'    description: {ts_str(s["description"])},')
            if 'details' in s:
                body.append(f'    details: {ts_str(s["details"])},')
            body.append(f'    category: {ts_str(s["category"])},')
            if 'group' in s:
                body.append(f'    group: {ts_str(s["group"])},')
            ctrl_ts = _render_control_ts(s['control'], used_rules, uses_presets)
            body.append(f'    control: {ctrl_ts},')
            if 'aliases_ref' in s:
                used_rules.add(s['aliases_ref'])
                body.append(f'    aliases: {s["aliases_ref"]},')
            elif 'aliases' in s:
                body.append(f'    aliases: {ts_val(s["aliases"])},')
            body.append(f'    default: {"defaultPreset" if is_preset else ts_val(s["default"])},')
            if 'default_text' in s:
                body.append(f'    defaultText: {ts_str(s["default_text"])},')
            if 'part_of' in s:
                body.append(f'    partOf: {ts_str(s["part_of"])},')
            if 'inactive_hint' in s:
                body.append(f'    inactiveHint: {ts_str(s["inactive_hint"])},')
            if 'visible_when' in s:
                body.append(f'    visibleWhen: {compile_pred_fn(s["visible_when"], used_rules)},')
            if 'required_when' in s:
                body.append(f'    requiredWhen: {compile_pred_fn(s["required_when"], used_rules)},')
            if 'caution_when' in s:
                body.append(f'    cautionWhen: {compile_caution_fn(s["caution_when"], used_rules)},')
            if 'host_gated' in s:
                if s['host_gated'] == 'virtual_display_os':
                    used_rules.add('VIRTUAL_DISPLAY_OS_GATE')
                    body.append('    hostGated: VIRTUAL_DISPLAY_OS_GATE,')
                elif s['host_gated'] == 'nvidia':
                    used_types.add('nvidiaGate')
                    body.append('    hostGated: nvidiaGate,')
                else:
                    raise ValueError(f'Unknown host_gated: {s["host_gated"]}')
            if 'host_reason' in s:
                used_types.add('nvidiaReason')
                body.append(f'    hostReason: nvidiaReason({ts_str(s["host_reason"])}),')
            if s.get('restart'):
                body.append('    restart: true,')
            if s.get('advanced'):
                body.append('    advanced: true,')
            if s.get('pending'):
                body.append('    pending: true,')
            body.append('  },')

        header = ['// Generated from rust/core/config/settings.toml by tools/generated_docs.py; do not edit by hand.']
        type_imports = [*sorted(used_types), 'type Setting']
        header.append(f"import {{ {', '.join(type_imports)} }} from '../settings-types';")
        if uses_presets:
            header.append("import { SETTING, defaultPreset, presets } from '../stream-presets';")
        if used_rules:
            header.append(f"import {{ {', '.join(sorted(used_rules))} }} from './rules.gen';")
        header.append('')
        if export_launcher_groups:
            header.append(f'export const LAUNCHER_GROUPS: Record<string, string> = {ts_val(cat["web"]["launcher_groups"])};')
            header.append('')
        header.append('export const settings: Setting[] = [')
        return '\n'.join(header + body + ['];']) + '\n'
    return _render


# ---------- Docs generator (docs/configuration.md) ----------

TAB_LABELS = {
    'pc': 'This PC',
    'picture': 'Picture',
    'sound': 'Sound & controls',
    'games': 'Games',
    'network': 'Network',
    'advanced': 'Advanced',
}


def _topic_tab(category):
    if category == 'general':
        return 'pc'
    if category in ('video', 'encoders', 'frame-limiting', 'display', 'hdr'):
        return 'picture'
    if category in ('audio', 'input'):
        return 'sound'
    if category in ('library', 'commands'):
        return 'games'
    if category == 'network':
        return 'network'
    return 'advanced'


def _auto_doc_default(s, shared_options):
    d = s['default']
    ctrl = s['control']
    kind = ctrl['kind']
    if isinstance(d, bool):
        return '`true`' if d else '`false`'
    if isinstance(d, list):
        return 'none' if len(d) == 0 else f'`{json.dumps(d)}`'
    if isinstance(d, dict):
        return f'`{json.dumps(d)}`'
    if kind == 'select':
        ds = str(d)
        if ctrl.get('options_ref') == 'stream_presets':
            opts = [
                {'value': 'lowest_latency', 'label': 'Lowest latency'},
                {'value': 'balanced', 'label': 'Balanced'},
                {'value': 'reference', 'label': 'Reference quality'},
            ]
        elif 'options_ref' in ctrl:
            opts = shared_options[ctrl['options_ref']]
        else:
            opts = ctrl.get('options', [])
        opt = next((o['label'] for o in opts if o['value'] == ds), None)
        if ds == '':
            return f'empty ({opt})' if opt else 'empty'
        return f'`{ds}` ({opt})' if opt else f'`{ds}`'
    if kind == 'number':
        unit = ctrl.get('unit')
        return f'`{d}` {unit}' if unit else f'`{d}`'
    if d == '':
        return 'empty'
    return f'`{d}`'


def render_config_doc(cat, existing):
    start_marker, end_marker = SETTINGS_MARKERS
    if existing is None or start_marker not in existing or end_marker not in existing:
        raise RuntimeError(f'docs/configuration.md is missing {start_marker} / {end_marker} markers')
    by_key = cat['_by_key']
    web = cat['web']
    main_keys = {k for sec in web['main_sections'] for k in sec['keys']}
    origin_cat = web['origin_category']
    shared_opts = web['shared_options']

    block = []
    for idx, sec in enumerate(cat['docs']['sections']):
        if idx > 0:
            block.append('')
        block.append(sec['heading'])
        block.append('')
        if 'before_note' in sec:
            block.append(sec['before_note'])
            block.append('')
        block.append('| Setting | Key | Default | What it does |')
        block.append('| --- | --- | --- | --- |')
        is_debug = sec['heading'] == '#### Debug switches'
        for key in sec['keys']:
            s = by_key[key]
            if is_debug:
                orig_c = origin_cat.get(key, s['category'])
                col_setting = f'{TAB_LABELS[_topic_tab(orig_c)]} › {s["group"]}: {s["label"]}'
            else:
                base = f'{s["group"]}: {s["label"]}'
                col_setting = f'**{base}**' if key in main_keys else base
            col_default = s.get('doc_default', _auto_doc_default(s, shared_opts))
            col_desc = s.get('doc_summary', s['description'])
            block.append(f'| {col_setting} | `{key}` | {col_default} | {col_desc} |')
        if 'after_note' in sec:
            block.append('')
            block.append(sec['after_note'])

    before, rest = existing.split(start_marker, 1)
    _, after = rest.split(end_marker, 1)
    return f'{before}{start_marker}\n' + '\n'.join(block) + f'\n{end_marker}{after}'


DOCS = [
    Doc('client/docs/settings.md', [sys.executable, 'scripts/generate-settings.py', '--target', 'docs/settings.md'], 'client',
        SETTINGS_MARKERS,
        (NIGHTJAR_SETTINGS_TOML, 'client/scripts/generate-settings.py', 'client/app/src/main/res/', 'client/app/src/main/java/')),
    Doc('client/docs/devices.md', ['node', 'scripts/device-docs.mjs'], 'client',
        ('<!-- device-results:start -->', '<!-- device-results:end -->'),
        ('client/scripts/device-docs.mjs', 'client/site/data/device-results.json')),
    # Nightjar settings generated from client/protocol/policy/settings.toml (standalone client/scripts/generate-settings.py)
    Doc('client/protocol/policy/src/settings_catalog_gen.rs',
        [sys.executable, 'scripts/generate-settings.py', '--target', 'protocol/policy/src/settings_catalog_gen.rs'], 'client',
        inputs=NIGHTJAR_SETTINGS_INPUTS),
    Doc('client/protocol/desktop-config/src/config/defaults_gen.rs',
        [sys.executable, 'scripts/generate-settings.py', '--target', 'protocol/desktop-config/src/config/defaults_gen.rs'], 'client',
        inputs=NIGHTJAR_SETTINGS_INPUTS),
    Doc('client/app/src/main/java/com/limelight/preferences/SettingsCatalogGen.java',
        [sys.executable, 'scripts/generate-settings.py', '--target', 'app/src/main/java/com/limelight/preferences/SettingsCatalogGen.java'], 'client',
        inputs=NIGHTJAR_SETTINGS_INPUTS),
    # Host settings generated from rust/core/config/settings.toml
    Doc('rust/core/config/src/config/catalog_gen.rs', inputs=HOST_SETTINGS_INPUTS, render=render_rust_catalog),
    Doc('rust/core/config/src/config/retired_gen.rs', inputs=HOST_SETTINGS_INPUTS, render=render_rust_retired),
    Doc('rust/core/config/src/config/console_global_gen.rs', inputs=HOST_SETTINGS_INPUTS, render=render_rust_console_global),
    Doc('rust/core/config/src/config/console_layers_gen.rs', inputs=HOST_SETTINGS_INPUTS, render=render_rust_console_layers),
    Doc('rust/web/src/lib/schema/rules.gen.ts', inputs=HOST_SETTINGS_INPUTS, render=render_web_rules),
    Doc('rust/web/src/lib/schema/tiers.gen.ts', inputs=HOST_SETTINGS_INPUTS, render=render_web_tiers),
    Doc('rust/web/src/lib/schema/overrides.gen.ts', inputs=HOST_SETTINGS_INPUTS, render=render_web_overrides),
    Doc('rust/web/src/lib/schema/basics_core.gen.ts', inputs=HOST_SETTINGS_INPUTS, render=render_web_part('basics_core')),
    Doc('rust/web/src/lib/schema/basics_io.gen.ts', inputs=HOST_SETTINGS_INPUTS, render=render_web_part('basics_io')),
    Doc('rust/web/src/lib/schema/video_stream.gen.ts', inputs=HOST_SETTINGS_INPUTS, render=render_web_part('video_stream')),
    Doc('rust/web/src/lib/schema/video_encoders.gen.ts', inputs=HOST_SETTINGS_INPUTS, render=render_web_part('video_encoders')),
    Doc('rust/web/src/lib/schema/display_screen.gen.ts', inputs=HOST_SETTINGS_INPUTS, render=render_web_part('display_screen')),
    Doc('rust/web/src/lib/schema/display_extras.gen.ts', inputs=HOST_SETTINGS_INPUTS, render=render_web_part('display_extras')),
    Doc('rust/web/src/lib/schema/library.gen.ts', inputs=HOST_SETTINGS_INPUTS, render=render_web_part('library', export_launcher_groups=True)),
    Doc('rust/web/src/lib/schema/extensions.gen.ts', inputs=HOST_SETTINGS_INPUTS, render=render_web_part('extensions')),
    Doc('docs/configuration.md', markers=SETTINGS_MARKERS, inputs=HOST_SETTINGS_INPUTS, render=render_config_doc),
]


def affected(paths):
    """The generated docs whose file, generator or sources are among paths."""
    found = []
    for doc in DOCS:
        if any(p == doc.path or any(p == i or (i.endswith('/') and p.startswith(i)) for i in doc.inputs) for p in paths):
            found.append(doc)
    return found


def regenerate(doc, root=ROOT, catalog=None):
    if doc.render is not None:
        cat = catalog if catalog is not None else load_catalog(root)
        path = root / doc.path
        existing = path.read_text(encoding='utf-8') if path.exists() else None
        rendered = doc.render(cat, existing).encode('utf-8')
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(rendered)
        return
    program = shutil.which(doc.command[0])
    if not program:
        raise RuntimeError(f'{doc.command[0]} is not on PATH (needed for {doc.path})')
    out = subprocess.run([program, *doc.command[1:]], cwd=root / doc.cwd, capture_output=True, text=True)
    if out.returncode:
        raise RuntimeError(f'{" ".join(doc.command)} failed for {doc.path}: {(out.stderr or out.stdout).strip()}')


def run_all(check, docs=None, root=ROOT):
    stale = []
    targets = docs or DOCS
    catalog = load_catalog(root) if any(d.render is not None for d in targets) else None
    for doc in targets:
        path = root / doc.path
        if doc.render is not None:
            before = path.read_bytes() if path.exists() else None
            existing = before.decode('utf-8') if before is not None else None
            rendered = doc.render(catalog, existing).encode('utf-8')
            if rendered != before:
                stale.append(doc.path)
                if not check:
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(rendered)
            continue
        before = path.read_bytes() if path.exists() else None
        try:
            regenerate(doc, root, catalog=catalog)
            if path.read_bytes() != before:
                stale.append(doc.path)
        finally:
            if check:
                if before is not None:
                    path.write_bytes(before)
                elif path.exists():
                    path.unlink()
    return stale


def conflicts_inside(text, markers):
    """Whether every conflict in git merge-file output lies inside the generated block."""
    if markers is None:
        return '<<<<<<< ' in text
    start, end = markers
    inside, in_conflict, any_conflict = False, False, False
    for line in text.splitlines():
        if line.startswith(start):
            inside = True
        elif line.startswith(end):
            if in_conflict:
                return False
            inside = False
        elif line.startswith('<<<<<<< '):
            any_conflict = True
            if not inside:
                return False
            in_conflict = True
        elif line.startswith('>>>>>>> '):
            in_conflict = False
    return any_conflict and not in_conflict


def merge(base, ours, theirs, name):
    """git's merge driver: 0 and the merged file in ours, or 1 and a conflicted file."""
    doc = next((d for d in DOCS if d.path == name.replace('\\', '/')), None)
    labels = ['-L', 'ours', '-L', 'base', '-L', 'theirs']
    marked = subprocess.run(['git', 'merge-file', '-p', *labels, ours, base, theirs], capture_output=True)
    if marked.returncode < 0 or marked.returncode > 127:
        return 1  # merge-file failed: git keeps this side's file as a conflict
    if marked.returncode == 0:
        pathlib.Path(ours).write_bytes(marked.stdout)
        return 0
    if doc is None or not conflicts_inside(marked.stdout.decode('utf-8', 'replace'), doc.markers):
        pathlib.Path(ours).write_bytes(marked.stdout)
        return 1
    resolved = subprocess.run(['git', 'merge-file', '-p', '--ours', ours, base, theirs], capture_output=True)
    pathlib.Path(ours).write_bytes(resolved.stdout)
    print(f'{name}: generated table merged as this side\'s; run `python tools/generated_docs.py` '
          'after the merge to regenerate it from the merged sources.', file=sys.stderr)
    return 0


def install(root=ROOT):
    # The config is shared by every worktree of the clone: one on a branch from before this
    # tool falls back to git's own merge, conflict markers included.
    # This Python, else python on PATH; neither: git's own merge, markers included.
    script = 'tools/generated_docs.py'
    python = pathlib.Path(sys.executable).as_posix()
    driver = (f'PY="{python}"; command -v "$PY" >/dev/null 2>&1 || PY=python; '
              f'if test -f {script} && command -v "$PY" >/dev/null 2>&1; then "$PY" {script} merge %O %A %B %P; '
              'else git merge-file -L ours -L base -L theirs %A %O %B; fi')
    for key, value in ((f'merge.{DRIVER}.name', 'Rubylight generated docs (regenerate after merging)'),
                       (f'merge.{DRIVER}.driver', driver)):
        subprocess.run(['git', '-C', str(root), 'config', key, value], check=True)
    print(f'merge driver {DRIVER} registered for this clone and its worktrees')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('action', nargs='?', default='regen', choices=['regen', 'install', 'merge'])
    parser.add_argument('files', nargs='*')
    parser.add_argument('--check', action='store_true', help='fail when a generated doc is stale; change nothing')
    argv = list(sys.argv[1:] if argv is None else argv)
    if not any(a in ('regen', 'install', 'merge') for a in argv):
        argv.insert(0, 'regen')  # `generated_docs.py --check client/docs/settings.md`
    args = parser.parse_args(argv)
    if args.action == 'install':
        install()
        return 0
    if args.action == 'merge':
        if len(args.files) != 4:
            parser.error('merge takes %O %A %B %P')
        return merge(*args.files)
    docs = [d for d in DOCS if not args.files or d.path in {f.replace('\\', '/') for f in args.files}]
    if not docs:
        parser.error('no generated doc among ' + ', '.join(args.files))
    try:
        stale = run_all(args.check, docs)
    except RuntimeError as error:
        print(error, file=sys.stderr)
        return 1
    if args.check:
        if stale:
            print('Stale: ' + ', '.join(stale) + '. Run `python tools/generated_docs.py` and commit the result.')
            return 1
        print('Generated docs are current.')
        return 0
    print('Regenerated: ' + (', '.join(stale) if stale else 'nothing changed'))
    return 0


if __name__ == '__main__':
    sys.exit(main())
