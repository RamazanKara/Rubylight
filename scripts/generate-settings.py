"""Regenerate the source-backed tables in docs/settings.md using only the standard library."""

import math
from pathlib import Path
import re
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
RES = ROOT / "app/src/main/res"
JAVA = ROOT / "app/src/main/java/com/limelight"
ANDROID = "{http://schemas.android.com/apk/res/android}"
SEEKBAR = "{http://schemas.moonlight-stream.com/apk/res/seekbar}"

# Usage advice is editorial; labels, options, ranges, dependencies and preset writes come from source.
ADVICE = {
    "list_resolution": "Raise for fine detail; lower when decode time or network load is high. Native sizes are added and unsupported sizes filtered on the device.",
    "list_fps": "Raise for smoother motion on a fast screen. Native refresh is added on the device; unsupported rates are filtered unless unlocked. Changing resolution or FPS recalculates bitrate.",
    "seekbar_bitrate_kbps": "Raise for image quality on a stable link; lower for network loss. The slider is logarithmic; the buttons make fine adjustments.",
    "video_format": "Start with Automatic. Choose a specific codec to compare decoder performance; PyroWave requires its own host and Vulkan capabilities.",
    "upscaling_mode": "Use for lower-resolution SDR video on a larger screen. HDR, 10-bit, PyroWave and unsupported or overloaded GPUs use direct output.",
    "upscaling_sharpness": "Reduce for halos or ringing; raise for soft edges. Applies on the next stream. At 0%, FSR skips extra sharpening; SGSR still reconstructs edges.",
    "checkbox_enable_hdr": "Enable with an HDR10 screen, a compatible decoder and HDR configured on the host display. Availability is device-filtered.",
    "checkbox_vrr": "Enable to follow game cadence. Uses Android adaptive refresh when confirmed, otherwise the panel maximum. Overrides pacing to Lowest latency and disables lower refresh.",
    "checkbox_virtual_display": "Enable to request a host display matching the stream. Requires host support and a ready display driver.",
    "seekbar_virtual_display_scale": "Request host rendering at a percentage of the stream size. This does not set desktop text size or Android density.",
    "checkbox_yuv444": "Enable for colored text and desktop detail when both ends support it. Otherwise 4:2:0 is used; HDR takes priority.",
    "checkbox_stretch_video": "Enable to fill a screen with a different aspect ratio; shapes will stretch.",
    "seekbar_deadzone": "Raise to suppress stick drift; lower for small movements. Games may impose an additional deadzone.",
    "checkbox_multi_controller": "Leave on to detect connected controllers and assign players. Turn off when a game needs a controller present before one connects.",
    "controller_button_mapping": "Open Controller buttons, press a digital button and choose its output or Nothing. Mappings default to the original button, are saved per controller model and apply next stream.",
    "checkbox_flip_face_buttons": "Enable to swap A/B and X/Y on physical and on-screen gamepads.",
    "checkbox_gamepad_motion_sensors": "Enable for controller gyro/accelerometer input on Android 12+. Fresh Android 12 (API 31) installations override this to Off because of an OS sensor bug.",
    "checkbox_gamepad_motion_fallback": "Enable to use this device's motion sensors when a controller lacks them. Hidden if the device has neither gyro nor accelerometer.",
    "checkbox_vibrate_fallback": "Enable to feel host rumble on this device when the controller cannot rumble.",
    "seekbar_vibrate_fallback_strength": "Adjust device rumble strength; 100% is normal strength.",
    "checkbox_gamepad_touchpad_as_mouse": "Enable to always move the host pointer with the controller touchpad instead of forwarding native touchpad input.",
    "checkbox_mouse_emulation": "Leave on to toggle controller mouse mode by holding Start.",
    "analog_scrolling": "Choose which stick scrolls in controller mouse mode. The other moves the pointer; Both sticks move the mouse disables stick scrolling.",
    "checkbox_touchscreen_trackpad": "Use Trackpad for relative cursor movement, Direct mouse to point at a screen location, or Multi-touch for host touch input. Unsupported native touch falls back to Direct mouse.",
    "seekbar_trackpad_speed": "Adjust relative touchscreen movement; 100% is normal speed.",
    "seekbar_mouse_speed": "Adjust physical relative mouse movement; 100% is normal speed.",
    "checkbox_mouse_nav_buttons": "Enable to send mouse Back/Forward buttons. Disable if it interferes with right-click.",
    "checkbox_absolute_mouse_mode": "Enable for desktop pointer acceleration. Leave off for game mouse look. Available on Android 8+ except devices using the separate raw pointer path.",
    "checkbox_show_onscreen_controls": "Enable a touchscreen gamepad. The category is hidden on devices without a touchscreen.",
    "checkbox_vibrate_osc": "Enable feedback when pressing virtual buttons.",
    "checkbox_only_show_L3R3": "Enable when another controller supplies everything except stick clicks.",
    "checkbox_show_guide_button": "Enable to expose the virtual Guide/Home button.",
    "seekbar_osc_opacity": "Lower to see more of the game beneath the controls.",
    "reset_osc": "Confirm to restore virtual button sizes and positions. Global settings reset does not reset this layout.",
    "checkbox_enable_perf_overlay": "Enable while diagnosing frame rate, network loss or delay. Can also be toggled during a stream.",
    "performance_overlay_mode": "Use Compact for a corner summary or Advanced for the measured stages. Hold the overlay to switch; Copy stats includes Advanced details.",
    "list_audio_config": "Choose 5.1 or 7.1 for a matching audio output and host configuration; use stereo for headphones or two speakers.",
    "checkbox_host_audio": "Enable to hear audio on the PC as well as Android.",
    "checkbox_enable_audiofx": "Enable for Android equalizers or effects. Can add delay and changes the audio output path.",
    "checkbox_enable_post_stream_toast": "Enable to see average decode time after a session ends.",
    "checkbox_enable_pip": "Enable to keep a small video window when leaving the app. Input and overlay are hidden there; requires device support.",
    "list_languages": "Choose the interface language. Android 13+ uses the system per-app language setting.",
    "checkbox_small_icon_mode": "Enable to fit more games in the library grid.",
    "help": "Open setup and troubleshooting help.",
    "latency_benchmark": "Run a 10-second local frame-callback, timer and input-dispatch benchmark without a host. It does not measure stream or physical end-to-end latency.",
    "report_problem": "Preview the explanation and share a redacted event/crash report through Android's share sheet.",
    "about_app": "View the installed app version and open Source and licenses.",
    "reset_all": "Confirm to reset global preferences. Keeps pairing, PC profiles, controller mappings, control layout and language.",
    "frame_pacing": "Use Lowest latency for response, Balanced for cadence, Balanced with FPS limit to cap pacing, or Smoothest video to retain frames at the cost of delay. VRR overrides this.",
    "checkbox_codec_low_latency": "Leave on for supported Android 11+ decoders. Disable when comparing a decoder freeze or artifact.",
    "checkbox_vendor_low_latency": "Leave on for supported chipset decoder keys. Disable to isolate vendor-specific instability.",
    "checkbox_codec_performance": "Leave on for decoder priority/rate hints on Android 6+. Disable to compare heat or battery use.",
    "checkbox_phone_performance_hints": "Enable only for a measured comparison on Android 12+. CPU scheduling hints can also increase delay on some devices.",
    "checkbox_drop_late_frames": "Enable with Balanced pacing to discard older queued outputs. Can trade smoothness for response.",
    "checkbox_unbatched_input": "Enable to bypass display-rate input batching. High-rate input can increase CPU and network load.",
    "checkbox_network_priority": "Enable to compare scheduling under load. Requests streaming thread priority; it is not a router QoS setting.",
    "checkbox_texture_view": "Try for black or flickering video or misplaced output. Adds composition work and is disabled for HDR and PyroWave.",
    "checkbox_unlock_fps": "Enable to expose high rates otherwise filtered by display capability; the screen may not show every frame.",
    "checkbox_reduce_refresh_rate": "Enable to allow a lower screen refresh for power savings. VRR disables it.",
    "checkbox_full_range": "Use only when testing a matching host/display color range; incorrect range loses shadow or highlight detail.",
    "checkbox_usb_driver": "Leave on for built-in Xbox USB support. Requires USB host support; Android requests USB access when needed.",
    "checkbox_usb_dualsense": "Enable for direct USB DualSense/Edge input, feedback and adaptive triggers. Accept the explanation and Android USB permission during a stream; Bluetooth retains Android's input path.",
    "checkbox_usb_bind_all": "Enable to prefer Rubylight's Xbox USB driver even when Android already recognizes the controller.",
    "checkbox_enable_sops": "Leave on to request game optimization from hosts that implement it; the host decides whether it has an effect.",
    "checkbox_disable_warnings": "Enable only to hide stream connection warnings after diagnosing the connection.",
    "export_latency_csv": "Choose a destination for the latest local frame-timing CSV. Missing measurements remain blank.",
}


def generate():
    resources = {}
    for file in sorted((RES / "values").glob("*.xml")):
        for element in ET.parse(file).getroot():
            if "name" in element.attrib:
                resources[element.get("name")] = element

    def resolve(value):
        if value.startswith("@string/"):
            value = "".join(resources[value.split("/", 1)[1]].itertext())
        return re.sub(r"\s+", " ", value.replace("\\n", " ").replace("\\'", "'").replace('\\"', '"')).strip()

    def array(value):
        return [resolve("".join(item.itertext())) for item in resources[value.split("/", 1)[1]]]

    preferences = ET.parse(RES / "xml/preferences.xml").getroot()
    nodes = {node.get(ANDROID + "key"): node for category in preferences for node in category}
    if set(nodes) != set(ADVICE):
        raise ValueError(f"Update usage advice: missing={set(nodes) - set(ADVICE)}, removed={set(ADVICE) - set(nodes)}")
    config = (JAVA / "preferences/PreferenceConfiguration.java").read_text(encoding="utf-8")
    constants = dict(re.findall(r"static final String (\w+) = \"([^\"]+)\";", config))
    defaults = dict(re.findall(r'static final (?:String|boolean|int) (\w+) = ("[^"]*"|true|false|\d+);', config))
    for key, value in re.findall(
            r'prefs\.get(?:String|Boolean|Int)\(\s*("[^"]+"|\w+),\s*("[^"]*"|[\w.]+)\s*\)', config):
        key = constants.get(key, key.strip('"'))
        value = defaults.get(value.split(".")[-1], value).strip('"')
        if key in nodes and nodes[key].get(ANDROID + "defaultValue") not in (None, value):
            raise ValueError(f"XML and runtime defaults differ for {key}: {nodes[key].get(ANDROID + 'defaultValue')} != {value}")
    seekbar = (JAVA / "preferences/SeekBarPreference.java").read_text(encoding="utf-8")
    slider_defaults = dict(re.findall(r'getAttributeIntValue\(\w+, "(min|max|step|keyStep|divisor)", (\d+)\)', seekbar))

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

    lines = ["## Global settings", "", "These tables include every preference, including actions. On/Off choices have no units; action rows have no stored default. Device capability checks can hide or disable a row.", ""]
    for category in preferences:
        lines += ["### " + resolve(category.get(ANDROID + "title")), "",
                  "| Setting / preference key | Default | Choices and units | When to change or use |",
                  "| --- | --- | --- | --- |"]
        for node in category:
            key = node.get(ANDROID + "key")
            default = node.get(ANDROID + "defaultValue")
            choices = "Action"
            if node.get(ANDROID + "entries"):
                choices = "; ".join(display_value(key, value) for value in array(node.get(ANDROID + "entryValues")))
            elif node.tag.endswith("SeekBarPreference"):
                minimum = node.get(SEEKBAR + "min", slider_defaults["min"])
                maximum = node.get(ANDROID + "max", slider_defaults["max"])
                step = node.get(SEEKBAR + "step", slider_defaults["step"])
                choices = f"{display_value(key, minimum)}–{display_value(key, maximum)}"
                if node.get(SEEKBAR + "logScale") == "true":
                    method = seekbar.split("static int logStep(int value)", 1)[1].split("static int clamp", 1)[0]
                    bands = re.findall(r"if \(value < (\d+)\) \{\s*return (\d+);", method)
                    steps = re.findall(r"return (\d+);", method)
                    if not bands or len(steps) != len(bands) + 1:
                        raise ValueError("Review the changed logarithmic slider steps")
                    choices += "; slider/± steps: " + ", ".join(
                        f"{display_value(key, size)} below {display_value(key, limit)}" for limit, size in bands)
                    choices += f", {display_value(key, steps[-1])} from {display_value(key, bands[-1][0])}"
                else:
                    choices += f"; step {display_value(key, step)}"
                    key_step = node.get(SEEKBAR + "keyStep")
                    if key_step:
                        choices += f"; keyboard step {display_value(key, key_step)}"
                if node.get(SEEKBAR + "presets"):
                    choices += "; presets " + ", ".join(display_value(key, item) for item in array(node.get(SEEKBAR + "presets")))
            elif "SwitchPreference" in node.tag or "CheckboxPreference" in node.tag:
                choices = "On; Off"
            if key == "seekbar_bitrate_kbps":
                # Fail if the runtime formula changes instead of silently retaining a stale default.
                formula = "(fps <= 60 ? fps : (Math.sqrt(fps / 60.f) * 60.f)) / 30.f"
                if formula not in config or "Math.round(resolutionFactor * frameRateFactor) * 1000" not in config:
                    raise ValueError("Review the changed runtime bitrate formula")
                pixels = [int(w) * int(h) for w, h in re.findall(r"(\d+)\s*\*\s*(\d+)", re.search(r"int\[\] pixelVals = \{(.*?)\};", config, re.S)[1])]
                factors = [int(n) for n in re.findall(r"\d+", re.search(r"int\[\] factorVals = \{(.*?)\};", config, re.S)[1])][:-1]
                width, height = map(int, nodes["list_resolution"].get(ANDROID + "defaultValue").split("x"))
                fps = int(nodes["list_fps"].get(ANDROID + "defaultValue"))
                factor = factors[pixels.index(width * height)]
                rate = (fps if fps <= 60 else math.sqrt(fps / 60) * 60) / 30
                default = display_value(key, str(math.floor(factor * rate + 0.5) * 1000)) + " at the default resolution/FPS; recalculated when either changes"
            elif key == "checkbox_small_icon_mode":
                threshold = re.search(r"smallestScreenWidthDp < (\d+)", config)[1]
                default = f"On below {threshold} dp smallest screen width; Off on TVs and larger screens"
            elif key == "checkbox_touchscreen_trackpad":
                trackpad = re.search(r"DEFAULT_TOUCHSCREEN_TRACKPAD = (true|false)", config)[1] == "true"
                default = array("@array/stream_touch_modes")[0 if trackpad else 1]
                choices = "; ".join(array("@array/stream_touch_modes"))
            else:
                default = display_value(key, default) if default is not None else "No stored default"
            if key == "checkbox_gamepad_motion_sensors":
                default += "; Off on Android 12 (API 31)"
            advice = ADVICE[key]
            dependency = node.get(ANDROID + "dependency")
            if dependency:
                advice += " Requires " + resolve(nodes[dependency].get(ANDROID + "title")) + " On."
            lines.append("| " + " | ".join(map(cell, [resolve(node.get(ANDROID + "title")) + f"<br>`{key}`", default, choices, advice])) + " |")
        lines.append("")

    source = (JAVA / "preferences/StreamPreset.java").read_text(encoding="utf-8")
    presets = re.findall(r'^\s+([A-Z_]+)\((null|"[^"]+"), (\d+), (true|false), "([^"]+)", "([^"]+)"\)', source, re.M)
    names = array("@array/stream_preset_names")
    if len(presets) != len(names):
        raise ValueError("Preset names and definitions differ")
    apply = source.split("void apply(", 1)[1]
    writes = re.findall(r"\.put(?:String|Boolean|Int)\(([^,]+), (.+)\)\s*;?\s*$", apply, re.M)
    if not writes or "this == BATTERY_SAVER ? 30 : DisplayFrameRatePolicy.streamFrameRate(panelMaxHz)" not in source:
        raise ValueError("Review the changed preset implementation")
    policy = (JAVA / "binding/video/DisplayFrameRatePolicy.java").read_text(encoding="utf-8")
    bounds = re.search(r"Math.max\((\d+), Math.min\((\d+), Math.round\(panelMaxHz\)\)\)", policy)
    if not bounds:
        raise ValueError("Review the changed preset refresh-rate policy")
    fps_label = f"Screen maximum (rounded, {bounds[1]}–{bounds[2]} FPS)"
    lines += ["## Presets", "", "Presets change only the following global values. Other settings, including resolution, HDR and sharpening, stay as selected. Saved PC profiles take priority. The selected chip says Custom when values do not match a preset.", "",
              "| Setting | " + " | ".join(names) + " |",
              "| --- | " + " | ".join("---" for _ in names) + " |"]
    for key_expression, expression in writes:
        key = constants[key_expression.split(".")[-1]] if key_expression.startswith("PreferenceConfiguration.") else key_expression.strip('"')
        values = []
        for enum, codec, bitrate, vrr, pacing, upscaling in presets:
            fields = {"codec": codec.strip('"'), "bitrate": bitrate, "vrr": vrr, "pacing": pacing, "upscaling": upscaling}
            if expression == "Integer.toString(fps(panelMaxHz))":
                values.append("30 FPS" if enum == "BATTERY_SAVER" else fps_label)
                continue
            if expression in fields:
                value = fields[expression]
            elif expression in ("true", "false"):
                value = expression
            elif expression in ("this == BATTERY_SAVER", "this != BATTERY_SAVER"):
                value = str((enum == "BATTERY_SAVER") == ("==" in expression)).lower()
            else:
                raise ValueError(f"Review preset expression: {expression}")
            values.append("Keep selected codec" if value == "null" else display_value(key, value))
        lines.append("| " + resolve(nodes[key].get(ANDROID + "title")) + " | " + " | ".join(values) + " |")
    lines += ["", "Choose Balanced for mixed play, Low latency for fast response, Best quality for a stable high-bandwidth connection, or Battery saver for a 30 FPS target with reduced decoder performance hints.", ""]
    return "\n".join(lines)


if __name__ == "__main__":
    path = ROOT / "docs/settings.md"
    text = path.read_text(encoding="utf-8")
    start = "<!-- BEGIN GENERATED SETTINGS -->"
    end = "<!-- END GENERATED SETTINGS -->"
    before, rest = text.split(start)
    _, after = rest.split(end)
    path.write_text(before + start + "\n\n" + generate() + "\n" + end + after, encoding="utf-8", newline="\n")
    print("Generated docs/settings.md from preference XML and Java defaults/presets.")
