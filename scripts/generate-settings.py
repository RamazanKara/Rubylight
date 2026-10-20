"""Regenerate the source-backed tables in docs/settings.md using only the standard library."""

from pathlib import Path
import re
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
RES = ROOT / "app/src/main/res"
JAVA = ROOT / "app/src/main/java/com/limelight"
ANDROID = "{http://schemas.android.com/apk/res/android}"
SEEKBAR = "{http://schemas.moonlight-stream.com/apk/res/seekbar}"

# Usage advice is editorial; labels, options, ranges, dependencies, sections and preset writes come from source.
ADVICE = {
    "checkbox_always_dark": "Enable for the dark grey theme on app screens, menus and dialogs even when the system uses a light theme, as most TVs do. OLED black takes precedence. Applies immediately and stays separate from stream profiles.",
    "checkbox_oled_black": "Enable for pure black app, menu and dialog backgrounds, including when the system uses a light theme. Applies immediately and stays separate from stream profiles.",
    "list_resolution": "Choose Custom to enter a fixed width and height (64–16384 pixels each) for every stream without a resolution override. Named profiles, PC and game overrides retain their priority. Custom dimensions stay fixed when the screen changes. Raise for detail; lower for less network and decoding load. The picker also includes screen and standard sizes; larger fixed presets stay listed while chosen. Changing resolution preserves an explicitly chosen bitrate.",
    "list_fps": "Raise for smoother motion on a fast screen. The screen's own refresh rate is added as Screen refresh rate. Changing resolution or FPS preserves an explicitly chosen bitrate.",
    "stream_profiles": "Open to create, rename, duplicate or delete named profiles (sets of stream values such as Home 4K120 or Hotel), pick the profile in use, and optionally let the network pick one when a stream starts. A profile sets only the values chosen in it. See [named profiles](#named-profiles-and-game-settings).",
    "list_mobile_bitrate_kbps": "Choose a cap for streams that start on mobile data or on Wi-Fi that Android marks as metered, such as a phone's hotspot. It caps the bitrate from every layer, including profiles, PC and game values; Off uses the same bitrate on every network. Read once at stream start. See [named profiles](#named-profiles-and-game-settings).",
    "seekbar_bitrate_kbps": "Raise for image quality on a stable link; lower for network loss. The slider is logarithmic; the buttons make fine adjustments.",
    "video_format": "Start with Automatic. Choose a specific codec to compare decoder performance; Prefer AV1 and Prefer HEVC fall back when the PC or device lacks them. PyroWave requires a host that supports it (Rubylight, or Vibepollo with PyroWave available) and Vulkan capabilities; on a PC whose host has none, that PC uses the setting for all PCs or Automatic instead.",
    "adaptive_quality_mode": "Off by default. Motion preserves FPS first, Detail preserves resolution first, and Battery also reduces work on healthy links. Requires the host's reconfigure capability and an adaptive decoder; PyroWave is unsupported. The stream menu can override the mode per PC.",
    "adaptive_quality_min_resolution": "Sets the smallest short edge for automatic quality, with the aspect ratio preserved. The selected starting resolution is the ceiling; a minimum above that ceiling is clamped down. Available while Automatic stream quality is on.",
    "adaptive_quality_min_fps": "Sets the lowest FPS automatic quality may request, including Battery mode. Recovery increases one step after 30 healthy seconds; reductions are at least 15 seconds apart. Available while Automatic stream quality is on.",
    "upscaling_mode": "Choose a stream resolution below the screen size for SDR upscaling. PyroWave uses SGSR for both FSR and SGSR selections. HDR and 10-bit streams use direct output; the row says so.",
    "upscaling_sharpness": "Reduce for halos or ringing; raise for soft edges. Applies on the next stream. At 0%, FSR skips extra sharpening; SGSR still reconstructs edges. Available with FSR 1.0 or SGSR 1.",
    "checkbox_enable_hdr": "Enable with an HDR10 screen, a compatible decoder and HDR configured on the host display. Availability is device-filtered.",
    "checkbox_vrr": "Enable Android's automatic ARR policy on supported Android 16 displays; otherwise request maximum refresh. Hosts that support VRR are asked for it too. Overrides pacing to Lowest latency and disables the lower refresh rate.",
    "checkbox_host_frame_times": "Enable for even motion when the network or decoder adds jitter, especially with Adaptive refresh on a screen with ARR. Each MediaCodec frame is released at its host timestamp plus a delay covering about 97% of recent frames; later frames are shown at once. Adds that delay (usually a few milliseconds) and replaces Frame pacing. Hosts that announce clock sync (Rubylight) add the host-to-present latency to the performance overlay. PyroWave streams are not affected.",
    "checkbox_virtual_display": "Enable to request a host display matching the stream. Requires host support and a ready display driver.",
    "host_display_location": "Explains where the mode is set: open the PC's menu, choose a mode and confirm Save display mode. Keep PC display primary preserves the monitor's primary role; make virtual display primary moves it to the stream. Saved separately per PC.",
    "seekbar_virtual_display_scale": "Change the host render pixel count relative to the stream. Desktop text size stays as set on the host.",
    "checkbox_yuv444": "Enable for colored text and desktop detail when both ends support it. Otherwise 4:2:0 is used; HDR takes priority.",
    "checkbox_stretch_video": "How the stream fills a screen of another shape: Fit shows all of it with bars, Fill (crop) fills the screen and cuts off the edges that do not fit, Stretch fills it and distorts shapes, 1:1 pixels shows the stream at its own size, centred. The key is the older Stretch switch, still written (on only for Stretch) so older versions behave sensibly; the mode itself is stored as `video_scaling`. Also in the stream menu. See [video scaling](video-and-latency.md#video-scaling).",
    "video_alignment": "Top places a Fit or 1:1 picture at the top of the screen, leaving the space below for the soft keyboard or the lower half of a half-folded phone. No effect on Fill and Stretch, which cover the whole screen.",
    "stream_orientation": "Automatic keeps the earlier behaviour: phones in landscape; tablets, squarish screens, windows and external displays in any orientation. Landscape and Portrait lock the screen; Rotate with device follows it within the system rotation lock. A host streaming from its virtual display starts the stream in the screen's shape, and with Rotate with device asks for the turned size while the stream runs where the host takes in-stream reconfigure. Other hosts keep the stream's size and Video scaling fits it into the turned screen. Hidden on TVs. See [orientation](video-and-latency.md#orientation).",
    "seekbar_deadzone": "Raise to suppress stick drift; lower for small movements. Games can add their own deadzone on top.",
    "checkbox_multi_controller": "Leave on so each controller is its own player and the PC sees controllers as they connect. Turn off to make every controller player 1 and keep one controller present on the PC, for games that need one before any connects.",
    "controller_button_mapping": "Open Controller buttons, press a digital button and choose its output or Nothing. Mappings default to the original button, are saved per controller model and apply next stream.",
    "controller_test": "Open Controller test to see buttons, sticks, triggers and the D-pad live and to try rumble, lights, motion sensors and the touchpad where the controller has them. Works with a D-pad or controller alone; hold B or Back for one second to leave. Leaving stops rumble and restores the lights. See [controller test](controls.md#controller-test).",
    "checkbox_flip_face_buttons": "Enable to swap A/B and X/Y on physical and on-screen gamepads.",
    "checkbox_gamepad_motion_sensors": "Enable for controller gyro/accelerometer input on Android 12+. Fresh Android 12 (API 31) installations override this to Off because of an OS sensor bug.",
    "checkbox_gamepad_motion_fallback": "Enable to use this device's motion sensors when a controller lacks them. Hidden if the device has neither gyro nor accelerometer.",
    "checkbox_vibrate_fallback": "Enable to feel host rumble on this device when the controller has no rumble motors.",
    "seekbar_vibrate_fallback_strength": "Adjust device rumble strength; 100% is normal strength.",
    "checkbox_gamepad_touchpad_as_mouse": "Enable to always move the host pointer with the controller touchpad instead of forwarding native touchpad input.",
    "checkbox_disable_rumble": "Enable to stop every vibration the host asks for: controller and trigger rumble, USB DualSense waveform haptics and Vibrate device for rumble. Checked once at stream start.",
    "checkbox_mouse_emulation": "Leave on to toggle controller mouse mode by holding Start.",
    "analog_scrolling": "Choose which stick scrolls in controller mouse mode. The other moves the pointer; with None, both move it.",
    "checkbox_mouse_emulation_fixed_step": "Enable for precise pointing in controller mouse mode: hold X while moving the stick to move a fixed step; Y doubles the step from 1 to 128 pixels, then wraps to 1.",
    "checkbox_touchscreen_trackpad": "Use Trackpad for relative cursor movement, Direct mouse to point at a screen location, Multi-touch for host touch input, Local cursor for a pointer drawn on this device with no round trip, or Touch off to ignore fingers while playing with a controller (Back still opens the stream menu). Unsupported native touch falls back to Direct mouse. The picker's Help explains each mode. See [touch modes](controls.md#touch-modes).",
    "seekbar_trackpad_speed": "Adjust Trackpad movement; 100% is normal speed.",
    "seekbar_local_cursor_speed": "Adjust Local cursor movement; at 100% the drawn pointer moves as far as your finger.",
    "checkbox_trackpad_acceleration": "Enable so slow finger movement aims precisely and fast swipes cover more distance. Trackpad and Local cursor; applies from the next stream.",
    "list_trackpad_acceleration_curve": "Choose how strongly fast swipes are sped up.",
    "checkbox_trackpad_momentum": "Enable so the pointer keeps gliding after a quick flick. A new touch, a mode change, the stream menu, picture-in-picture and disconnecting stop it at once. Trackpad and Local cursor.",
    "list_trackpad_momentum_glide": "Choose how far the pointer glides.",
    "list_trackpad_area": "Limit where Trackpad and Local cursor gestures may start, for example to rest a thumb on one side; touches that start elsewhere are ignored.",
    "list_three_finger_tap": "Choose Phone keyboard (default), Keyboard & mouse, Middle click or Off. Works in Trackpad, Direct mouse and Local cursor modes.",
    "list_four_finger_tap": "Choose Keyboard & mouse, Phone keyboard, Middle click or Off (default). Four fingers never also trigger the three-finger action.",
    "seekbar_mouse_speed": "Adjust physical relative mouse movement; 100% is normal speed.",
    "checkbox_mouse_nav_buttons": "Enable to send mouse Back/Forward buttons. Disable if it interferes with right-click.",
    "checkbox_absolute_mouse_mode": "Enable for desktop pointer acceleration. Leave off for game mouse look. Available on Android 8+ except devices using the separate raw pointer path.",
    "checkbox_input_only_any_host": "Enable to add Input-only to a game's menu on PCs whose host has no Input-only mode of its own (Sunshine, GameStream, Apollo with its input-only mode off). The PC still streams video and audio at 640x360, 30 FPS and 500 kbps, about 0.7 Mbit/s with overhead; Nightjar discards both. Hosts with their own mode keep using their Input-only tile. See [Input-only on any PC](controls.md#input-only-on-any-pc).",
    "keyboard_shortcuts_editor": "Create, edit or delete up to 32 named keyboard chords for the extra keyboard toolbar. Import/export JSON files to transfer them; importing asks before replacing the saved list. See [custom keyboard shortcuts](controls.md#custom-keyboard-shortcuts).",
    "checkbox_back_as_meta": "Enable to send the Windows key from a physical keyboard's Back key. Android's Back gesture and TV remotes keep their local navigation; the controller menu chord remains available. See [keyboard options](controls.md#keyboard-options).",
    "checkbox_back_opens_menu": "Enable for a handheld's dedicated physical Back key, such as on AYN Thor, to open the stream menu instead of being forwarded as a controller button. Off preserves its existing mapping. Select/Start, mouse right-click and Keyboard Back sends Windows key keep their behavior. Applies from the next stream.",
    "checkbox_right_alt_as_meta": "Enable on a Mac host so Right Alt acts as Command (the Windows key). Left Alt is unchanged; leave off if you type with AltGr.",
    "checkbox_show_onscreen_controls": "Enable a touchscreen gamepad. The group is hidden on devices without a touchscreen.",
    "checkbox_vibrate_osc": "Enable feedback when pressing virtual buttons.",
    "checkbox_only_show_L3R3": "Enable when another controller supplies everything except stick clicks.",
    "checkbox_show_guide_button": "Enable to expose the virtual Guide/Home button.",
    "seekbar_osc_opacity": "Lower to see more of the game beneath the controls.",
    "reset_osc": "Confirm to restore virtual button sizes and positions. This layout is kept separately from a global settings reset.",
    "checkbox_enable_perf_overlay": "Enable while diagnosing frame rate, network loss or delay. Can also be toggled during a stream.",
    "performance_overlay_mode": "Compact is one line: displayed FPS, network round trip (RTT) and frame loss behind a green, amber or red dot. Statistics summarizes the stream with a status line of lasting facts. Debug shows detailed measured stages and is always available. Installs that kept the old Statistics default move to Compact once. Switch in stream menu → Display → Overlay and diagnostics; Copy stats includes Debug details.",
    "gpu_driver": "Import and select a compatible Snapdragon Vulkan driver for PyroWave. Shown on arm64 Adreno devices only. The system driver remains the default; changes apply to the next stream.",
    "list_audio_config": "Choose 5.1 or 7.1 for a matching audio output and host configuration; use stereo for headphones or two speakers.",
    "checkbox_host_audio": "Enable to hear audio on the PC as well as Android.",
    "checkbox_enable_mic": "Enable to talk in games and chat apps on the PC. Asks for microphone access; if it is not allowed, the setting stays off. The PC hears it as its streaming microphone. Mute it from the stream menu; the overlay shows whether it is sending.",
    "checkbox_enable_audiofx": "Enable for Android equalizers or effects. Can add delay and changes the audio output path.",
    "checkbox_enable_post_stream_toast": "Enable to see the average decode time after a session ends; it is not end-to-end latency.",
    "checkbox_enable_pip": "Enable to keep a small video window when leaving the app. Input and overlay are hidden there; requires device support.",
    "checkbox_auto_reconnect": "Resume the same running game after transport failures or returning from screen lock, up to five attempts within one minute. Back or leaving during a retry cancels it. Graceful termination, pairing, permissions and decoder failures stay manual; recovery never launches or replaces a game.",
    "list_network_grace": "Off by default: the stream ends after 10 seconds without contact, as before. 20, 30 or 60 seconds keeps it open through a Wi-Fi roam, a short dropout or a VPN moving between Wi-Fi and mobile data; the picture freezes until the network returns. Longer waits only help if the host waits too, so raise its own client timeout. When the wait runs out the stream ends with a message saying so, and Recover interrupted streams, if on, then reconnects. See [network drops](video-and-latency.md#network-drops).",
    "list_languages": "Choose the interface language. Android 13+ uses the system per-app language setting.",
    "checkbox_small_icon_mode": "Enable to fit more games in the library grid.",
    "checkbox_quick_resume": "Enable to go straight back into the running game when you tap its PC, skipping the library. Use the PC's menu for the library while a game runs. Off keeps the tap opening the library.",
    "help": "Open the bundled help: connecting a PC, pairing, controls and the stream menu, common fixes and problem reports. Works offline; each topic links to these documents for detail.",
    "latency_benchmark": "Run a 10-second local frame-callback, timer and input-dispatch benchmark without a host. Use Measure input latency in the stream menu for end-to-end timing.",
    "report_problem": "Preview the explanation and share a redacted event/crash report through Android's share sheet.",
    "checkbox_check_updates": "Leave on to look for a newer release on GitHub when the PC list opens, at most every 12 hours; you choose whether to install it. Hidden when the app was installed from Google Play, which updates it.",
    "check_updates_now": "Look for a newer release now, even if the automatic check ran recently. A release you skipped is shown again. Hidden when the app was installed from Google Play.",
    "about_app": "View the installed app version and open Source and licenses.",
    "reset_all": "Confirm to reset global preferences. Keeps pairing, PC profiles, controller mappings, control layout and language.",
    "frame_pacing": "Use Lowest latency for response, Balanced for cadence, Balanced with FPS limit to cap pacing, or Smoothest video to retain frames at the cost of delay.",
    "checkbox_codec_low_latency": "Leave on for supported Android 11+ decoders. Disable when comparing a decoder freeze or artifact.",
    "checkbox_vendor_low_latency": "Leave on for supported chipset decoder keys. Disable to isolate vendor-specific instability.",
    "checkbox_disable_hevc_low_latency": "Off by default. Enable if HEVC video stutters or freezes to omit Android and vendor low-latency flags on any device. H.264, AV1, reference-frame invalidation and decoder performance hints keep their own settings. Applies to the next stream and decoder benchmark; changing it invalidates previous benchmark results.",
    "checkbox_codec_performance": "Leave on for decoder priority/rate hints on Android 6+. Disable to compare heat or battery use.",
    "checkbox_phone_performance_hints": "Enable for a measured comparison on Android 12+; compare decode time with it on and off.",
    "checkbox_gpu_max_clocks": "Off by default. Optionally request higher Adreno GPU clocks during PyroWave streaming to reduce decode time; this can increase power use and heat. Thermal protection remains active. Installs that already stored a choice keep it. Shown on Adreno devices only.",
    "checkbox_drop_late_frames": "Enable with Balanced pacing to discard older queued outputs. Favors response over smoothness. Available with Balanced frame pacing.",
    "checkbox_unbatched_input": "Enable to bypass display-rate input batching. High-rate input can increase CPU and network load.",
    "checkbox_network_priority": "Enable to compare scheduling under load. Requests streaming thread priority on this device.",
    "checkbox_texture_view": "Try for black or flickering video or misplaced output. Adds composition work and is disabled for HDR and PyroWave.",
    "checkbox_unlock_fps": "Enable to expose high rates otherwise filtered by display capability; compare shown FPS in the overlay.",
    "checkbox_reduce_refresh_rate": "Enable to allow a lower screen refresh for power savings.",
    "checkbox_full_range": "Use when the host and display both send full-range color, for full shadow and highlight detail.",
    "checkbox_usb_driver": "Leave on for built-in Xbox USB support. Android requests USB access when needed.",
    "checkbox_usb_dualsense": "Enable for direct USB DualSense/Edge input, feedback and adaptive triggers. Accept the explanation and Android USB permission during a stream; Bluetooth retains Android's input path.",
    "checkbox_usb_dualsense_haptics": "Enable alongside the USB DualSense driver for native waveform feedback. Hosts that support it (currently Rubylight) also forward controller speaker, headset and microphone audio. Microphone streaming needs its separate setting and Android permission; the controller mute button follows the stream mute. Reconnect after changing this option. USB only.",
    "checkbox_usb_switch_pro": "Experimental and untested on hardware. Enable to read a wired Nintendo Switch Pro Controller directly: calibrated sticks, Home and Capture, motion, rumble, player lights and battery. Accept Android's USB permission during a stream; Bluetooth and Joy-Con keep Android's input path. See [Switch Pro over USB](controls.md#switch-pro-over-usb).",
    "checkbox_usb_bind_all": "Enable to prefer the app's built-in Xbox USB driver even when Android already recognizes the controller.",
    "checkbox_enable_sops": "Leave on to request game optimization from hosts that implement it; the host decides whether it has an effect.",
    "checkbox_disable_warnings": "Enable to hide stream connection warnings once your connection is tuned.",
    "checkbox_rust_engine": "Off by default. Enable to stream with Nightjar's new engine (written in Rust) instead of the current one. Streams it does not handle yet (PyroWave, remote monitor sessions, hosts older than GFE 3.22) keep the current engine automatically. Applies from the next stream; turn it off if a stream fails to start or misbehaves, and report the problem.",
    "export_latency_csv": "Choose a destination for the latest local frame-timing CSV. Each row holds the measured stage timings.",
    "low_latency_decoding": "Leave on. Sets Android low-latency mode, Chipset low-latency mode and Phone performance hints together; turn off only to test a decoding problem. The individual switches are also in Advanced; the row says when one is off.",
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
    categories = {category.get(ANDROID + "key"): category for category in preferences}
    # Sections: the overview rows (preferences_root.xml) and the groups each one shows (StreamSettings.SECTIONS).
    overview = ET.parse(RES / "xml/preferences_root.xml").getroot()
    section_titles = {node.get(ANDROID + "key"): resolve(node.get(ANDROID + "title"))
                      for node in overview if not node.tag.endswith("PresetPreference")}
    settings_java = (JAVA / "preferences/StreamSettings.java").read_text(encoding="utf-8")
    sections = {}
    for name, members in re.findall(r'SECTIONS\.put\("(\w+)", new String\[\] \{([^}]*)\}\);', settings_java):
        sections[name] = re.findall(r'"(\w+)"', members)
    if list(sections) != list(section_titles) or sorted(c for group in sections.values() for c in group) != sorted(categories):
        raise ValueError("Review the changed settings sections")
    config = (JAVA / "preferences/PreferenceConfiguration.java").read_text(encoding="utf-8")
    constants = dict(re.findall(r"static final String (\w+) = \"([^\"]+)\";", config))
    seekbar = (JAVA / "preferences/SeekBarPreference.java").read_text(encoding="utf-8")
    slider_defaults = dict(re.findall(r'getAttributeIntValue\(\w+, "(min|max|step|keyStep|divisor)", (\d+)\)', seekbar))
    policy = (JAVA / "binding/video/DisplayFrameRatePolicy.java").read_text(encoding="utf-8")
    bounds = re.search(r"Math.max\((\d+), Math.min\((\d+), Math.round\(panelMaxHz\)\)\)", policy)
    if not bounds:
        raise ValueError("Review the changed preset refresh-rate policy")
    native_defaults = config.split("static boolean migrateToNativeDefaults(", 1)[1].split("return true;", 1)[0]
    for write in ("DisplayFrameRatePolicy.streamFrameRate(panelMaxHz)",
                  ".putString(RESOLUTION_PREF_STRING, nativeResolution)",
                  ".putString(FPS_PREF_STRING, fps)",
                  ".putInt(BITRATE_PREF_STRING, getDefaultBitrate(nativeResolution, fps))"):
        if write not in native_defaults:
            raise ValueError("Review the changed native installation defaults")
    merged = re.search(r"LOW_LATENCY_DECODING_KEYS = \{([^}]*)\}", config)[1]
    merged_titles = [resolve(nodes[constants[name]].get(ANDROID + "title")) for name in re.findall(r"(\w+_PREF_STRING)", merged)]

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
        elif "SwitchPreference" in node.tag or "CheckboxPreference" in node.tag:
            choices = "On; Off"
        if key == "list_resolution":
            default = "This screen's native resolution; follows the screen used for each stream"
        elif key == "list_fps":
            default = f"Screen maximum (rounded, {bounds[1]}–{bounds[2]} FPS)"
        elif key == "seekbar_bitrate_kbps":
            default = "Calculated for the native resolution and screen maximum FPS; follows changes until bitrate is chosen explicitly"
        elif key == "checkbox_small_icon_mode":
            threshold = re.search(r"smallestScreenWidthDp < (\d+)", config)[1]
            default = f"On below {threshold} dp smallest screen width; Off on TVs and larger screens"
        elif key == "checkbox_touchscreen_trackpad":
            trackpad = re.search(r"DEFAULT_TOUCHSCREEN_TRACKPAD = (true|false)", config)[1] == "true"
            default = array("@array/stream_touch_modes")[0 if trackpad else 1]
            choices = "; ".join(array("@array/stream_touch_modes"))
        elif key == "checkbox_stretch_video":
            default = array("@array/video_scaling_modes")[0]
            choices = "; ".join(array("@array/video_scaling_modes"))
        elif key == "low_latency_decoding":
            default = "On (not stored: on while " + ", ".join(merged_titles) + " are all on)"
        else:
            default = display_value(key, default) if default is not None else "No stored default"
        if key == "checkbox_gamepad_motion_sensors":
            default += "; Off on Android 12 (API 31)"
        advice = ADVICE[key]
        dependency = node.get(ANDROID + "dependency")
        if dependency:
            title = resolve(nodes[dependency].get(ANDROID + "title"))
            if nodes[dependency].get(ANDROID + "disableDependentsState") == "true":
                advice += f" Unavailable while {title} is On."
            else:
                advice += f" Requires {title} On."
        return "| " + " | ".join(map(cell, [resolve(node.get(ANDROID + "title")) + f"<br>`{key}`", default, choices, advice])) + " |"

    lines = ["## Global settings", "",
             "These tables include every preference, including actions, grouped as the Settings screens show them. On/Off choices have no units; action rows have no stored default. Device capability checks can hide or disable a row.", ""]
    for section, members in sections.items():
        lines += ["### " + section_titles[section], ""]
        for member in members:
            category = categories[member]
            if category.get(ANDROID + "title"):
                lines += ["#### " + resolve(category.get(ANDROID + "title")), ""]
            lines += ["| Setting / preference key | Default | Choices and units | When to change or use |",
                      "| --- | --- | --- | --- |"]
            lines += [row(node) for node in category]
            lines.append("")

    source = (JAVA / "preferences/StreamPreset.java").read_text(encoding="utf-8")
    presets = re.findall(r"^    ([A-Z_]+)[,;]$", source, re.M)
    names = array("@array/stream_preset_names")
    if presets != ["NATIVE", "BATTERY_SAVER"] or len(names) != len(presets):
        raise ValueError("Review the changed presets")
    block = source.split("LATENCY_SWITCHES = {", 1)[1].split("};", 1)[0]
    switches = re.findall(r'^\s+(?:"([a-z_]+)"|PreferenceConfiguration\.([A-Z_]+)),$', block, re.M)
    switch_titles = [resolve(nodes[literal or constants[constant]].get(ANDROID + "title")) for literal, constant in switches]
    if constants["GPU_MAX_CLOCKS_PREF_STRING"] in [literal or constants[constant] for literal, constant in switches]:
        raise ValueError("Review the presets table: Native now changes the GPU performance boost")
    lines += ["## Presets", "",
              "Both presets stream at this screen's own resolution and keep the selected codec, HDR and 4:4:4 choices. Named profiles and saved PC and game values take priority. The selected chip says Custom when values do not match a preset.", "",
              "| Setting | " + " | ".join(names) + " |",
              "| --- | --- | --- |",
              "| Video resolution | This screen's resolution | This screen's resolution |",
              f"| Video frame rate | Screen maximum (rounded, {bounds[1]}–{bounds[2]} FPS) | 30 FPS |",
              "| Video bitrate | Default for that resolution and frame rate | Default for that resolution and frame rate |",
              "| Adaptive refresh (ARR / VRR) | Off | Off |",
              "| Client-side upscaling | Off | Off |",
              "| Frame pacing | Lowest latency | Balanced with FPS limit |",
              "| Allow lower refresh rate | Off | On |",
              "| Decoder performance hints | On | Off |",
              f"| {resolve('@string/title_gpu_max_clocks')} | Unchanged | Off |",
              "", "Native also turns these back on: " + ", ".join(switch_titles) + ".", ""]
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
