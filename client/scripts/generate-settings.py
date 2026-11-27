"""Generate Nightjar's settings artifacts and docs/settings.md from protocol/policy/settings.toml.

Uses only the Python standard library (`tomllib`, `xml.etree.ElementTree`, etc.), keeping `client/`
self-contained for standalone Nightjar CI and `RamazanKara/nightjar` mirroring.

Usage:
    python3 scripts/generate-settings.py                 # regenerate all Nightjar settings targets
    python3 scripts/generate-settings.py --check         # fail if any generated target is stale
    python3 scripts/generate-settings.py --target PATH   # regenerate or check one client-relative path
"""

import argparse
import json
from pathlib import Path
import re
import sys
import tomllib
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
SETTINGS_TOML = ROOT / "protocol/policy/settings.toml"
RES = ROOT / "app/src/main/res"
JAVA = ROOT / "app/src/main/java/com/limelight"
ANDROID = "{http://schemas.android.com/apk/res/android}"
SEEKBAR = "{http://schemas.moonlight-stream.com/apk/res/seekbar}"

TARGET_POLICY_RS = "protocol/policy/src/settings_catalog_gen.rs"
TARGET_DESKTOP_RS = "protocol/desktop-config/src/config/defaults_gen.rs"
TARGET_ANDROID_JAVA = "app/src/main/java/com/limelight/preferences/SettingsCatalogGen.java"
TARGET_SETTINGS_MD = "docs/settings.md"
ALL_TARGETS = (
    TARGET_POLICY_RS,
    TARGET_DESKTOP_RS,
    TARGET_ANDROID_JAVA,
    TARGET_SETTINGS_MD,
)


def load_catalog(root=ROOT):
    data = tomllib.loads((root / "protocol/policy/settings.toml").read_text(encoding="utf-8"))
    android_keys = [s["key"] for s in data["android_setting"]]
    desktop_keys = [s["key"] for s in data["desktop_setting"]]
    const_names = [c["name"] for c in data["android_const"]]
    if len(android_keys) != len(set(android_keys)):
        dups = sorted({k for k in android_keys if android_keys.count(k) > 1})
        raise ValueError(f"Duplicate android_setting keys in settings.toml: {dups}")
    if len(desktop_keys) != len(set(desktop_keys)):
        dups = sorted({k for k in desktop_keys if desktop_keys.count(k) > 1})
        raise ValueError(f"Duplicate desktop_setting keys in settings.toml: {dups}")
    if len(const_names) != len(set(const_names)):
        dups = sorted({k for k in const_names if const_names.count(k) > 1})
        raise ValueError(f"Duplicate android_const names in settings.toml: {dups}")
    data["_android_by_key"] = {s["key"]: s for s in data["android_setting"]}
    data["_desktop_by_key"] = {s["key"]: s for s in data["desktop_setting"]}
    for s in data["android_setting"]:
        if "desktop_key" in s and s["desktop_key"] not in data["_desktop_by_key"]:
            raise ValueError(f"Unknown desktop_key {s['desktop_key']!r} on android_setting {s['key']!r}")
    for s in data["desktop_setting"]:
        if "android_key" in s and s["android_key"] not in data["_android_by_key"]:
            raise ValueError(f"Unknown android_key {s['android_key']!r} on desktop_setting {s['key']!r}")
    return data


def rs_str(value):
    return json.dumps(value, ensure_ascii=False)


def java_str(value):
    return json.dumps(value, ensure_ascii=False)


def render_policy_rs(cat):
    c = cat["constants"]
    android_retired = [r for r in cat["retired"] if r["platform"] == "android"]
    desktop_retired = [r for r in cat["retired"] if r["platform"] == "desktop"]
    android_renamed = [r for r in cat["renamed"] if r["platform"] == "android"]
    shared = [s for s in cat["android_setting"] if "desktop_key" in s]

    def fmt_u32_array(name, values, doc):
        items = ", ".join(str(v) for v in values)
        decl = f"pub const {name}: [u32; {len(values)}] = [{items}];"
        if len(decl) <= 100:
            return [f"/// {doc}", decl]
        out = [f"/// {doc}", f"pub const {name}: [u32; {len(values)}] = ["]
        for v in values:
            out.append(f"    {v},")
        out.append("];")
        return out

    lines = [
        "//! Generated canonical Nightjar settings catalog shared by desktop and Android.",
        "//! Generated from `client/protocol/policy/settings.toml` by `client/scripts/generate-settings.py`; do not edit by hand.",
        "",
        "use std::ops::RangeInclusive;",
        "",
        "/// Total Android settings rows documented in `settings.toml`.",
        f"pub const ANDROID_SETTING_COUNT: usize = {len(cat['android_setting'])};",
        "/// Total Desktop `Settings` fields declared in `settings.toml`.",
        f"pub const DESKTOP_SETTING_COUNT: usize = {len(cat['desktop_setting'])};",
        "",
        "/// Default Hold Guide to open the Control Center duration in milliseconds.",
        f"pub const DEFAULT_GUIDE_MENU_HOLD_MS: u32 = {c['default_guide_menu_hold_ms']};",
        *fmt_u32_array(
            "GUIDE_HOLD_MS",
            c["guide_hold_ms"],
            "The choices for holding Guide to open the Control Center, in milliseconds; 0 is off.",
        ),
        *fmt_u32_array(
            "DISPLAY_SCALES",
            c["display_scales"],
            "Windows virtual display scales offered in percent; 0 keeps the PC's own setting.",
        ),
        *fmt_u32_array(
            "UI_ZOOM_STEPS",
            c["ui_zoom_steps"],
            "Desktop launcher UI zoom steps in percent (0 is Auto).",
        ),
        "/// Fixed video FEC percentages offered in settings.",
        f"pub const ERROR_CORRECTION_PERCENTAGES: [u8; {len(c['error_correction_percentages'])}] = "
        f"[{', '.join(str(v) for v in c['error_correction_percentages'])}];",
        "/// Default decode unit queue bound in frames.",
        f"pub const DECODE_QUEUE_FRAMES_DEFAULT: u32 = {c['decode_queue_frames_default']};",
        *fmt_u32_array(
            "QUALITY_MIN_SHORT_EDGES",
            c["quality_min_short_edges"],
            "Shared adaptive quality minimum short-edge resolution choices.",
        ),
        *fmt_u32_array(
            "QUALITY_MIN_FPS",
            c["quality_min_fps"],
            "Shared adaptive quality minimum FPS choices.",
        ),
        *fmt_u32_array(
            "NETWORK_GRACE_CHOICES_S",
            c["network_grace_choices_s"],
            "Shared network dropout grace period choices in seconds.",
        ),
        "/// The PC render resolution range in percent.",
        f"pub const RENDER_SCALE: RangeInclusive<u32> = {c['render_scale_min']}..={c['render_scale_max']};",
        "/// The global stick dead zone range in percent.",
        f"pub const DEADZONE: RangeInclusive<u32> = {c['deadzone_min']}..={c['deadzone_max']};",
        "/// The microphone gain range in percent.",
        f"pub const MIC_GAIN: RangeInclusive<u32> = {c['mic_gain_min']}..={c['mic_gain_max']};",
        "/// The microphone gain slider step in percent.",
        f"pub const MIC_GAIN_STEP: u32 = {c['mic_gain_step']};",
        "/// The desktop keys holding Sharper colored text.",
        f"pub const SHARP_COLOR_DESKTOP_KEYS: [&str; {len(c['sharp_color_desktop_keys'])}] = "
        f"[{', '.join(rs_str(k) for k in c['sharp_color_desktop_keys'])}];",
        "/// The Android keys holding Sharper colored text.",
        f"pub const SHARP_COLOR_ANDROID_KEYS: [&str; {len(c['sharp_color_android_keys'])}] = "
        f"[{', '.join(rs_str(k) for k in c['sharp_color_android_keys'])}];",
        "",
        "/// A retired preference/config key that must never be reused with another meaning.",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]",
        "pub struct RetiredKey {",
        "    pub key: &'static str,",
        "    pub replacement: &'static str,",
        "}",
        "",
        "/// A setting present on both Android and Desktop clients.",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]",
        "pub struct SharedSetting {",
        "    pub android_key: &'static str,",
        "    pub desktop_key: &'static str,",
        "    pub tier: &'static str,",
        "    pub scope: &'static str,",
        "}",
        "",
        "/// Retired Android preference keys.",
        "pub const ANDROID_RETIRED_KEYS: &[RetiredKey] = &[",
    ]
    for r in android_retired:
        lines.append(f"    RetiredKey {{ key: {rs_str(r['key'])}, replacement: {rs_str(r['replacement'])} }},")
    lines.extend([
        "];",
        "",
        "/// Retired Desktop JSON setting keys.",
        "pub const DESKTOP_RETIRED_KEYS: &[RetiredKey] = &[",
    ])
    for r in desktop_retired:
        lines.append(f"    RetiredKey {{ key: {rs_str(r['key'])}, replacement: {rs_str(r['replacement'])} }},")
    lines.extend([
        "];",
        "",
        "/// Renamed Android preference keys `(old, new)`.",
        "pub const ANDROID_RENAMED_KEYS: &[(&str, &str)] = &[",
    ])
    for r in android_renamed:
        lines.append(f"    ({rs_str(r['old'])}, {rs_str(r['new'])}),")
    lines.extend([
        "];",
        "",
        "/// Cross-platform settings shared between Android and Desktop.",
        "pub const SHARED_SETTINGS: &[SharedSetting] = &[",
    ])
    for s in shared:
        entry = (
            f"    SharedSetting {{ android_key: {rs_str(s['key'])}, "
            f"desktop_key: {rs_str(s['desktop_key'])}, "
            f"tier: {rs_str(s['tier'])}, scope: {rs_str(s['scope'])} }},"
        )
        if len(entry) <= 100:
            lines.append(entry)
        else:
            lines.extend([
                "    SharedSetting {",
                f"        android_key: {rs_str(s['key'])},",
                f"        desktop_key: {rs_str(s['desktop_key'])},",
                f"        tier: {rs_str(s['tier'])},",
                f"        scope: {rs_str(s['scope'])},",
                "    },",
            ])
    lines.extend([
        "];",
        "",
        "/// Returns whether `key` is a retired Android preference key.",
        "pub fn is_android_retired(key: &str) -> bool {",
        "    ANDROID_RETIRED_KEYS.iter().any(|r| r.key == key)",
        "}",
        "",
        "/// Returns whether `key` is a retired Desktop JSON setting key.",
        "pub fn is_desktop_retired(key: &str) -> bool {",
        "    DESKTOP_RETIRED_KEYS.iter().any(|r| r.key == key)",
        "}",
        "",
    ])
    return "\n".join(lines)


def render_desktop_defaults_rs(cat):
    desktop = cat["desktop_setting"]
    override_keys = [s["key"] for s in desktop if s.get("override_allowed")]
    lines = [
        "//! Generated desktop `Settings::default()` and key catalogs.",
        "//! Generated from `client/protocol/policy/settings.toml` by `client/scripts/generate-settings.py`; do not edit by hand.",
        "",
        "use super::{",
        "    AnalogScrolling, AudioChannels, CardSize, CodecPreference, DecoderPreference, DisplayOutput,",
        "    ErrorCorrection, FramePacing, GamepadQuitCombo, GraphicsCard, HomeGroup, HomeLayout,",
        "    HomeOrder, HomeSort, HostDisplay, Language, MicEcho, MouseMode, OpenOnStart, OverlayCorner,",
        "    OverlayMode, PyroWaveDecoder, QualityMode, ReduceMotion, Renderer, Settings, Theme, Upscaling,",
        "    VideoScaling, VrrWait, WindowMode,",
        "};",
        "",
        "pub use nightjar_policy::settings_catalog_gen::{",
        "    DEADZONE, DISPLAY_SCALES, GUIDE_HOLD_MS, MIC_GAIN, MIC_GAIN_STEP, RENDER_SCALE,",
        "    SHARP_COLOR_DESKTOP_KEYS as SHARP_COLOR_KEYS, UI_ZOOM_STEPS,",
        "};",
        "",
        "/// All Desktop `Settings` field names in canonical catalog order.",
        "pub const DESKTOP_SETTING_KEYS: &[&str] = &[",
    ]
    for s in desktop:
        lines.append(f"    {rs_str(s['key'])},")
    lines.extend([
        "];",
        "",
        "/// Desktop `Settings` keys permitted in per-PC, per-game, or named profile overrides.",
        "pub const DESKTOP_OVERRIDE_KEYS: &[&str] = &[",
    ])
    for key in override_keys:
        lines.append(f"    {rs_str(key)},")
    lines.extend([
        "];",
        "",
        "/// Generated default values for [`Settings`].",
        "pub(super) fn default_settings() -> Settings {",
        "    Settings {",
    ])
    for s in desktop:
        lines.append(f"        {s['key']}: {s['default_expr']},")
    lines.extend([
        "    }",
        "}",
        "",
    ])
    return "\n".join(lines)


def render_android_java(cat):
    c = cat["constants"]
    android_retired_test = [
        r["key"] for r in cat["retired"] if r["platform"] == "android" and r.get("in_android_retired_set")
    ]
    android_renamed = [r for r in cat["renamed"] if r["platform"] == "android"]

    def java_lit(jtype, val):
        if jtype == "String":
            return java_str(val)
        if jtype == "boolean":
            return "true" if val else "false"
        return str(val)

    lines = [
        "package com.limelight.preferences;",
        "",
        "/**",
        " * Generated canonical Android preference keys, scalar defaults, value lists, and retired/renamed tables.",
        " * Generated from {@code client/protocol/policy/settings.toml} by {@code client/scripts/generate-settings.py}; do not edit by hand.",
        " */",
        "public class SettingsCatalogGen {",
        "    SettingsCatalogGen() {}",
        "",
    ]
    for const in cat["android_const"]:
        prefix = "public static final " if const["vis"] == "public" else "static final "
        lines.append(f"    {prefix}{const['type']} {const['name']} = {java_lit(const['type'], const['value'])};")

    lines.extend([
        "",
        "    static final String[] REMOVED_AUTO_BITRATE_LIMIT_PREFS = {"
        + ", ".join(java_str(k) for k in c["removed_auto_bitrate_limit_android_keys"])
        + "};",
        "    static final int[] VIRTUAL_DISPLAY_DPI_VALUES = {"
        + ", ".join(str(v) for v in c["display_scales"])
        + "};",
        "    static final int[] QUALITY_MIN_SHORT_EDGES = {"
        + ", ".join(str(v) for v in c["quality_min_short_edges"])
        + "};",
        "    static final int[] QUALITY_MIN_FPS = {"
        + ", ".join(str(v) for v in c["quality_min_fps"])
        + "};",
        "    static final String[] LOW_LATENCY_DECODING_KEYS = {",
        "            " + ", ".join(java_str(k) for k in c["low_latency_decoding_android_keys"]) + ",",
        "    };",
        "    static final String[][] RENAMED_PREFS = {",
    ])
    for r in android_renamed:
        lines.append(f"            {{{java_str(r['old'])}, {java_str(r['new'])}}},")
    lines.extend([
        "    };",
        "    static final String[] RETIRED_KEYS = {",
    ])
    for key in android_retired_test:
        lines.append(f"            {java_str(key)},")
    lines.extend([
        "    };",
        "}",
        "",
    ])
    return "\n".join(lines)


def generate_settings_md_body(cat, root=ROOT):
    res = root / "app/src/main/res"
    java = root / "app/src/main/java/com/limelight"
    by_key = cat["_android_by_key"]
    const_map = {c["name"]: c["value"] for c in cat["android_const"] if c["type"] == "String"}

    resources = {}
    for file in sorted((res / "values").glob("*.xml")):
        for element in ET.parse(file).getroot():
            if "name" in element.attrib:
                resources[element.get("name")] = element

    def resolve(value):
        if value.startswith("@string/"):
            value = "".join(resources[value.split("/", 1)[1]].itertext())
        return re.sub(r"\s+", " ", value.replace("\\n", " ").replace("\\'", "'").replace('\\"', '"')).strip()

    def array(value):
        return [resolve("".join(item.itertext())) for item in resources[value.split("/", 1)[1]]]

    preferences = ET.parse(res / "xml/preferences.xml").getroot()
    nodes = {node.get(ANDROID + "key"): node for category in preferences for node in category}
    overview = ET.parse(res / "xml/preferences_root.xml").getroot()
    profiles = next(node for node in overview if node.get(ANDROID + "key") == "stream_profiles")

    shown_while = {
        s["key"]: (s["shown_while"]["parent"], s["shown_while"]["on"])
        for s in cat["android_setting"]
        if "shown_while" in s
    }
    always_listed = {s["key"]: s["always_listed"] for s in cat["android_setting"] if "always_listed" in s}
    shown_only = {s["key"]: s["shown_only"] for s in cat["android_setting"] if "shown_only" in s}
    pickers = {
        s["key"]: (s["picker"]["array"], s["picker"]["default_index"])
        for s in cat["android_setting"]
        if "picker" in s
    }
    picker_order = {s["key"]: s["picker_order"] for s in cat["android_setting"] if "picker_order" in s}

    if set(shown_while) - set(nodes) or {parent for parent, _ in shown_while.values()} - set(nodes):
        raise ValueError("Review shown_while in settings.toml: a row it names is gone")
    if set(nodes) | {"stream_profiles"} != set(by_key):
        raise ValueError(
            f"Update settings.toml android_setting: missing={set(nodes) - set(by_key)}, "
            f"removed={set(by_key) - set(nodes) - {'stream_profiles'}}"
        )

    categories = {category.get(ANDROID + "key"): category for category in preferences}
    tiers = ET.parse(res / "values/settings_tiers.xml").getroot()
    main = {item.text for tier in tiers for item in tier}
    for tier in tiers:
        section = tier.get("name").removeprefix("settings_tier_")
        keys = [item.text for item in tier]
        if len(keys) > 8 or set(keys) - set(nodes):
            raise ValueError(f"Review the main rows of {section}: at most 8, all in preferences.xml")

    section_titles = {
        node.get(ANDROID + "key"): resolve(node.get(ANDROID + "title"))
        for node in overview
        if not node.tag.endswith("PresetPreference")
        and node is not profiles
        and node.get(ANDROID + "key") != "more_sections"
    }
    settings_java = (java / "preferences/StreamSettings.java").read_text(encoding="utf-8")
    sections = {}
    for name, members in re.findall(r'SECTIONS\.put\("(\w+)", new String\[\] \{([^}]*)\}\);', settings_java):
        sections[name] = re.findall(r'"(\w+)"', members)
    if set(sections) == set(section_titles):
        sections = {name: sections[name] for name in section_titles}
    if list(sections) != list(section_titles) or sorted(c for group in sections.values() for c in group) != sorted(categories):
        raise ValueError("Review the changed settings sections")

    seekbar = (java / "preferences/SeekBarPreference.java").read_text(encoding="utf-8")
    slider_defaults = dict(re.findall(r'getAttributeIntValue\(\w+, "(min|max|step|keyStep|divisor)", (\d+)\)', seekbar))
    policy = (root / "protocol/policy/src/video_pacing.rs").read_text(encoding="utf-8")
    bounds = re.search(r"java_round_f32\(panel_max_hz\)\.clamp\((\d+),\s*(\d+)\)", policy)
    if not bounds:
        raise ValueError("Review the changed preset refresh-rate policy")

    # Cross-check native installation defaults in PreferenceMigrations.java (after split) or PreferenceConfiguration.java.
    mig_path = java / "preferences/PreferenceMigrations.java"
    cfg_path = java / "preferences/PreferenceConfiguration.java"
    mig_src = mig_path.read_text(encoding="utf-8") if mig_path.exists() else cfg_path.read_text(encoding="utf-8")
    if "static boolean migrateToNativeDefaults(" not in mig_src:
        mig_src = cfg_path.read_text(encoding="utf-8")
    native_defaults = mig_src.split("static boolean migrateToNativeDefaults(", 1)[1].split("return true;", 1)[0]
    for write in (
        "DisplayFrameRatePolicy.streamFrameRate(panelMaxHz)",
        "RESOLUTION_PREF_STRING, nativeResolution",
        "FPS_PREF_STRING, fps",
        "BITRATE_PREF_STRING, ",
    ):
        if write not in native_defaults:
            raise ValueError(f"Review the changed native installation defaults (missing {write!r})")

    # Cross-check shared value lists against nightjar-policy quality.rs and recovery.rs.
    quality_rs = (root / "protocol/policy/src/quality.rs").read_text(encoding="utf-8")
    recovery_rs = (root / "protocol/policy/src/recovery.rs").read_text(encoding="utf-8")
    for src, sym, expected in (
        (quality_rs, "MIN_SHORT_EDGE_CHOICES", cat["constants"]["quality_min_short_edges"]),
        (quality_rs, "MIN_FPS_CHOICES", cat["constants"]["quality_min_fps"]),
        (recovery_rs, "NETWORK_GRACE_CHOICES_S", cat["constants"]["network_grace_choices_s"]),
    ):
        m = re.search(rf"const {sym}:\s*\[u32;\s*\d+\]\s*=\s*\[([^\]]*)\]", src)
        if not m or [int(x.strip()) for x in m.group(1).split(",")] != expected:
            raise ValueError(f"settings.toml constants out of sync with nightjar-policy {sym}")

    merged_titles = [
        resolve(nodes[key].get(ANDROID + "title"))
        for key in cat["constants"]["low_latency_decoding_android_keys"]
    ]

    def display_value(key, value):
        node = nodes[key]
        if value in ("true", "false"):
            return "On" if value == "true" else "Off"
        if node.get(ANDROID + "entryValues"):
            options = dict(zip(array(node.get(ANDROID + "entryValues")), array(node.get(ANDROID + "entries"))))
            if key == "list_resolution":
                return options[value] + " (" + value.replace("x", " × ") + " pixels)"
            return options[value]
        if node.tag.endswith("SeekBarPreference"):
            divisor = int(node.get(SEEKBAR + "divisor", slider_defaults["divisor"]))
            unit = resolve(node.get(ANDROID + "text", ""))
            return f"{int(value) / divisor:g}{' ' if unit and unit != '%' else ''}{unit}"
        return value

    def cell(value):
        return str(value).replace("|", "\\|").replace("\n", "<br>")

    def row(node):
        key = node.get(ANDROID + "key")
        default = node.get(ANDROID + "defaultValue")
        choices = "Action"
        if node.get(ANDROID + "entries"):
            choices = "; ".join(display_value(key, value) for value in array(node.get(ANDROID + "entryValues")))
        elif node.tag.endswith("SeekBarPreference"):
            minimum = node.get(SEEKBAR + "min", slider_defaults["min"])
            maximum = node.get(ANDROID + "max", slider_defaults["max"])
            step = node.get(SEEKBAR + "step", slider_defaults["step"])
            choices = f"{display_value(key, minimum)}–{display_value(key, maximum)}; step {display_value(key, step)}"
            key_step = node.get(SEEKBAR + "keyStep")
            if key_step:
                choices += f"; buttons {display_value(key, key_step)}"
            if node.get(SEEKBAR + "presets"):
                choices += "; presets " + ", ".join(display_value(key, item) for item in array(node.get(SEEKBAR + "presets")))
        elif "SwitchPreference" in node.tag or "CheckboxPreference" in node.tag or "ConsoleModePreference" in node.tag:
            choices = "On; Off"
        if key == "list_resolution":
            default = "This screen's native resolution; follows the screen used for each stream"
        elif key == "list_fps":
            default = f"Screen maximum (rounded, {bounds[1]}–{bounds[2]} FPS)"
        elif key == "seekbar_bitrate_kbps":
            default = (
                "Calculated for the native resolution and screen maximum FPS; with PyroWave, PyroWave's own "
                "(about 400 Mbps at 1080p60, at most 900 Mbps or 90% of a known wired link, never below its minimum); "
                "follows changes until bitrate is chosen explicitly"
            )
        elif key == "checkbox_small_icon_mode":
            threshold = cat["android"]["small_icon_min_sw_dp"]
            default = f"On below {threshold} dp smallest screen width; Off on TVs and larger screens"
        elif key == "touch_mode":
            trackpad = cat["android"]["default_touchscreen_trackpad"]
            default = array("@array/stream_touch_modes")[0 if trackpad else 1]
            choices = "; ".join(array("@array/stream_touch_modes"))
        elif key == "video_scaling":
            default = array("@array/video_scaling_modes")[0]
            choices = "; ".join(array("@array/video_scaling_modes"))
        elif key == "low_latency_decoding":
            default = "On (not stored: on while " + ", ".join(merged_titles) + " are all on)"
        elif key == "theme":
            default = array("@array/theme_choice_names")[0]
            choices = "; ".join(array("@array/theme_choice_names"))
        elif key in pickers:
            names, default_index = pickers[key]
            default = array(names)[default_index]
            order = picker_order.get(key, range(len(array(names))))
            choices = "; ".join(array(names)[i] for i in order)
        elif key == "checkbox_enable_perf_overlay":
            default = array("@array/overlay_choice_names")[0]
            choices = "; ".join(array("@array/overlay_choice_names"))
        elif key == "simple_hdr":
            default = "Off on a fresh install"
            choices = "; ".join(array("@array/simple_hdr_names"))
        elif key in ("simple_smoothness", "simple_picture"):
            names = {"simple_smoothness": "simple_smoothness_names", "simple_picture": "simple_picture_names"}[key]
            default = array("@array/" + names)[0]
            choices = "; ".join(array("@array/" + names))
        elif key == "low_latency_customize" or key.startswith("more_"):
            default = "Closed"
        else:
            default = display_value(key, default) if default is not None else "No stored default"
        if key == "checkbox_gamepad_motion_sensors":
            default += "; Off on Android 12 (API 31)"
        advice = by_key[key]["advice"]
        if key in shown_while and key not in shown_only:
            parent, on = shown_while[key]
            advice += f" Shown while {resolve(nodes[parent].get(ANDROID + 'title'))} is {'On' if on else 'Off'}."
        if key in shown_only:
            advice += f" In the list only {shown_only[key]}, or while it holds another value than its default; search finds it either way."
        if key in always_listed:
            advice += (
                " Always in the list, since the Control Center or a PC's, game's or profile's settings can make it act;"
                f" it says why while {always_listed[key]} in the main settings."
            )
        tier = "Main" if key in main or key.startswith("more_") else "More"
        return "| " + " | ".join(map(cell, [resolve(node.get(ANDROID + "title")) + f"<br>`{key}`", tier, default, choices, advice])) + " |"

    lines = [
        "## Global settings",
        "",
        "These tables include every preference, including actions, grouped as the Settings screens show them. On/Off choices have no units; action rows have no stored default. Device capability checks can hide or disable a row.",
        "",
        "Tier: Main rows show when a section opens; More rows fold behind the section's last row, More … settings, and search still finds them (`res/values/settings_tiers.xml`).",
        "",
    ]
    for section, members in sections.items():
        lines += ["### " + section_titles[section], ""]
        titled = False
        for index, member in enumerate(members):
            category = categories[member]
            if category.get(ANDROID + "title"):
                lines += ["#### " + resolve(category.get(ANDROID + "title")), ""]
                titled = True
            elif index and titled:
                lines += ["#### Other " + section_titles[section] + " settings", ""]
            elif index:
                lines.pop()
                lines += [row(node) for node in category]
                lines.append("")
                continue
            lines += [
                "| Setting / preference key | Tier | Default | Choices and units | When to change or use |",
                "| --- | --- | --- | --- | --- |",
            ]
            lines += [row(node) for node in category]
            lines.append("")

    source = (java / "preferences/StreamPreset.java").read_text(encoding="utf-8")
    presets = re.findall(r"^    ([A-Z_]+)[,;]$", source, re.M)
    names = array("@array/stream_preset_names")
    if presets != ["NATIVE", "SMOOTH_TV", "WIFI_AWAY", "BATTERY_SAVER"] or len(names) != len(presets):
        raise ValueError("Review the changed presets")
    block = source.split("LATENCY_SWITCHES = {", 1)[1].split("};", 1)[0]
    switches = re.findall(r'^\s+(?:"([a-z_]+)"|PreferenceConfiguration\.([A-Z_]+)),$', block, re.M)
    no_row = {"checkbox_drop_late_frames": "Keep only the newest frame (" + resolve("@string/pacing_balanced") + " in Frame pacing)"}
    resolved_switches = [literal or const_map[constant] for literal, constant in switches]
    switch_titles = [
        no_row[key] if key in no_row else resolve(nodes[key].get(ANDROID + "title"))
        for key in resolved_switches
    ]
    if const_map["GPU_MAX_CLOCKS_PREF_STRING"] in resolved_switches:
        raise ValueError("Review the presets table: Native now changes the GPU performance boost")
    lines += [
        "## Presets",
        "",
        "Every preset keeps the selected codec, HDR and Sharper colored text choices. Named profiles and saved PC and game values take priority. The selected chip says Custom when values do not match a preset.",
        "",
        "| Setting | " + " | ".join(names) + " |",
        "| --- | --- | --- | --- | --- |",
        "| Resolution | This screen's resolution | This screen's resolution | Two thirds of this screen's resolution (fixed, 1080p gives 720p) | This screen's resolution |",
        f"| Frame rate | Screen maximum (rounded, {bounds[1]}–{bounds[2]} FPS) | Screen maximum | 60 FPS, or the screen maximum below that | 30 FPS |",
        "| Bitrate | Default for that resolution, frame rate and codec | Default for that resolution, frame rate and codec | Default for that resolution, frame rate and codec | Default for that resolution, frame rate and codec |",
        "| Variable refresh rate | Off | Off | Off | Off |",
        "| Upscaling | Off | Off | Off | Off |",
        "| Frame pacing | Lowest latency | Smoothest video | Balanced | Balanced with FPS limit |",
        f"| {resolve('@string/title_lower_refresh')} | Off | Off | Off | On, or On, matched to the game, where that was chosen |",
        "| Decoder performance hints | On | On | On | Off |",
        f"| {resolve('@string/title_gpu_max_clocks')} | Unchanged | Unchanged | Unchanged | Off |",
        "",
        "Native also turns these back on; the other presets leave them as they are: " + ", ".join(switch_titles) + ".",
        "",
        "## Profiles",
        "",
        f"{resolve(profiles.get(ANDROID + 'title'))} (`stream_profiles`), under the presets on the Settings overview: "
        + by_key["stream_profiles"]["advice"],
        "",
    ]
    return "\n".join(lines)


def render_settings_md(cat, existing, root=ROOT):
    start = "<!-- BEGIN GENERATED SETTINGS -->"
    end = "<!-- END GENERATED SETTINGS -->"
    before, rest = existing.split(start, 1)
    _, after = rest.split(end, 1)
    return before + start + "\n\n" + generate_settings_md_body(cat, root=root) + "\n" + end + after


def render_target(target, cat, root=ROOT):
    if target == TARGET_POLICY_RS:
        return render_policy_rs(cat)
    if target == TARGET_DESKTOP_RS:
        return render_desktop_defaults_rs(cat)
    if target == TARGET_ANDROID_JAVA:
        return render_android_java(cat)
    if target == TARGET_SETTINGS_MD:
        existing = (root / target).read_text(encoding="utf-8")
        return render_settings_md(cat, existing, root=root)
    raise ValueError(f"Unknown target: {target}")


def run(targets=ALL_TARGETS, check=False, root=ROOT):
    cat = load_catalog(root)
    stale = []
    for rel in targets:
        path = root / rel
        before = path.read_bytes() if path.exists() else None
        rendered = render_target(rel, cat, root=root).encode("utf-8")
        if rendered != before:
            stale.append(rel)
            if not check:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(rendered)
    return stale


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="fail when a generated target is stale without modifying files")
    parser.add_argument("--target", action="append", choices=ALL_TARGETS, help="limit generation/check to specific client-relative target(s)")
    args = parser.parse_args(argv)
    targets = tuple(args.target) if args.target else ALL_TARGETS
    stale = run(targets=targets, check=args.check)
    if args.check:
        if stale:
            print("Stale Nightjar settings targets: " + ", ".join(stale) + ". Run `python3 scripts/generate-settings.py`.", file=sys.stderr)
            return 1
        print("Nightjar settings targets are current.")
        return 0
    print("Generated Nightjar settings targets: " + (", ".join(stale) if stale else "nothing changed") + ".")
    return 0


if __name__ == "__main__":
    sys.exit(main())
