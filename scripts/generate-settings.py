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
    "checkbox_always_dark": "Choose System to follow the system theme, Dark for the dark grey theme on app screens, menus and dialogs even when the system uses a light theme (as most TVs do), or OLED black for pure black backgrounds. Applies immediately and stays separate from stream profiles. Stored as two switches, `checkbox_always_dark` and `checkbox_oled_black`; OLED black sets both, System clears both. High contrast keeps the app black whatever is chosen here.",
    "list_ui_scale": "Raise for large, readable menus on a TV or a small phone; lower to fit more on screen. Scales the whole app layout, not the stream, and the system font size still applies on top. Applies immediately and stays separate from stream profiles.",
    "list_ui_mode": "Automatic follows the device. Choose TV on a handheld or phone that is connected to a TV (a Retroid Pocket docked over HDMI, for example) to get the TV layouts with focus, a remote or gamepad and larger rows; choose Handheld to get the touch layouts on a TV. The stream is not changed. Applies immediately and stays separate from stream profiles.",
    "checkbox_high_contrast": "Enable for a black background, body-coloured secondary text and outlines (7:1 against the background) and a 4 dp focus ring on every app screen. The stream is unchanged. Applies immediately and stays separate from stream profiles.",
    "list_reduce_motion": "Follow system reduces motion when Android's animation scale is 0 or animators are off. Reduce always swaps the focus zoom and sheet slide for a plain ring; Full always animates. Applies immediately and stays separate from stream profiles.",
    "list_resolution": "Choose Custom to enter a fixed width and height (64–16384 pixels each) for every stream without a resolution override. Named profiles, PC and game overrides retain their priority. Custom dimensions stay fixed when the screen changes. Raise for detail; lower for less network and decoding load. The picker also includes screen and standard sizes; larger fixed presets stay listed while chosen. Changing resolution preserves an explicitly chosen bitrate.",
    "list_fps": "Raise for smoother motion on a fast screen. Open the list and choose Show all frame rates to include rates the screen may not support (possible stutter; compare rendered FPS in the overlay), or Show supported frame rates only to go back. Stored as `checkbox_unlock_fps`. The screen's own refresh rate is added as Screen refresh rate. Changing resolution or FPS preserves an explicitly chosen bitrate.",
    "stream_profiles": "Open to create, rename, duplicate or delete named profiles (sets of stream values such as Home 4K120 or Hotel), pick the profile in use, and optionally let the network pick one when a stream starts. A profile sets only the values chosen in it. See [named profiles](#named-profiles-and-game-settings).",
    "list_mobile_bitrate_kbps": "Choose a cap for streams that start on mobile data or on Wi-Fi that Android marks as metered, such as a phone's hotspot. It caps the bitrate from every layer, including profiles, PC and game values; Off uses the same bitrate on every network. Read once at stream start. See [named profiles](#named-profiles-and-game-settings).",
    "checkbox_auto_bitrate": "Off by default. Lowers the bitrate when the connection, decoder or device heat can't keep up and raises it again when things recover, never above Video bitrate and never below a built-in floor (2 Mbps, or more for large streams). The host's own adjustment stands down while it runs. This is the default for every PC; the stream menu's Automatic bitrate saves a choice for one PC, and changing this switch makes every PC follow it again. Needs a host that supports runtime bitrate changes (currently Rubylight).",
    "checkbox_vulkan_render": "Experimental. Presents H.264, HEVC and AV1 through Vulkan with its own frame pacing, for frame-generation overlays that hook Vulkan. It adds a render pass, so some latency, and needs Android 10 and a GPU with Vulkan 1.1. If it cannot be used (PyroWave, Android 9 or older, no Vulkan support, 4:4:4 video, or it fails to start) the stream uses direct output and a message says why. While it is on, client-side upscaling is off. If it stops working during a stream, the stream ends with a message that names it; turn this switch off to go back. Leave it off unless you need such an overlay.",
    "seekbar_bitrate_kbps": "Raise for image quality on a stable link; lower for network loss. The slider is logarithmic; the buttons make fine adjustments.",
    "video_format": "Start with Automatic. Choose a specific codec to compare decoder performance; Prefer AV1 and Prefer HEVC fall back when the PC or device lacks them. PyroWave requires a host that supports it (Rubylight, or Vibepollo with PyroWave available) and Vulkan capabilities; on a PC whose host has none, that PC uses the setting for all PCs or Automatic instead.",
    "adaptive_quality_mode": "Off by default. Motion preserves FPS first, Detail preserves resolution first, and Battery also reduces work on healthy links. Lowers resolution or frame rate when the network, slow decoding or heat keep the stream from running well; not available with PyroWave. The stream menu can override the mode per PC.",
    "adaptive_quality_min_resolution": "Sets the smallest short edge for automatic quality, with the aspect ratio preserved. The selected starting resolution is the ceiling; a minimum above that ceiling is clamped down. Shown while Lower resolution or frame rate when needed is not Off.",
    "adaptive_quality_min_fps": "Sets the lowest FPS automatic quality may request, including Battery mode. Recovery increases one step after 30 healthy seconds; reductions are at least 15 seconds apart. Shown while Lower resolution or frame rate when needed is not Off.",
    "upscaling_mode": "Choose a stream resolution below the screen size for SDR upscaling. Smooth is bilinear; Sharp (FSR) and Sharp, Snapdragon (SGSR) add edge sharpening. PyroWave uses SGSR for both FSR and SGSR selections. HDR and 10-bit streams use direct output; the row says so.",
    "upscaling_sharpness": "Reduce for halos or ringing; raise for soft edges. Applies on the next stream. At 0%, FSR skips extra sharpening; SGSR still reconstructs edges. Shown only with Sharp (FSR) or Sharp, Snapdragon (SGSR).",
    "checkbox_enable_hdr": "Enable with an HDR10 screen, a compatible decoder and HDR configured on the host display. Availability is device-filtered.",
    "checkbox_enable_sdr_10bit": "Enable if you see banding in gradients and the screen has no HDR, or HDR is off. Streams SDR in 10-bit HEVC or AV1 and needs a Main10 decoder on this device and a PC that offers 10-bit; otherwise the stream is 8-bit and the app says so. The PC's display must be in SDR: with HDR on it, the stream is HDR. Not used with PyroWave. Takes effect on the next stream.",
    "checkbox_vrr": "Enable Android's automatic ARR policy on supported Android 16 displays; otherwise request maximum refresh. Hosts that support VRR are asked for it too. Overrides pacing to Lowest latency and disables the lower refresh rate.",
    "checkbox_host_frame_times": "Enable for even motion when the network or decoder adds jitter, especially with Adaptive refresh on a screen with ARR. Each MediaCodec frame is released at its host timestamp plus a delay covering about 97% of recent frames; later frames are shown at once. Adds that delay (usually a few milliseconds) and replaces Frame pacing. Hosts that announce clock sync (Rubylight) add the host-to-present latency to the performance overlay. PyroWave streams are not affected.",
    "checkbox_content_rate_frame_rate": "Enable on a screen with several refresh modes, with Allow lower refresh rate (or a Balanced with FPS limit or Smoothest video pacing) on. Once the game's frame rate has held steady for two seconds, Android is told that rate and may move the screen to a mode that divides it evenly, for example 60 Hz for a 60 fps game in a 120 fps stream. Only seamless changes are requested (Android 11 or newer), at most one every two seconds, and never below the rate Allow lower refresh rate off keeps. Needs a host that announces clock sync (Rubylight); has no effect with Adaptive refresh, a TextureView or PyroWave.",
    "checkbox_virtual_display": "Enable to request a host display matching the stream. Requires host support and a ready display driver.",
    "host_display_location": "Explains where the mode is set: open the PC's menu, choose a mode and confirm Save display mode. Keep PC display primary preserves the monitor's primary role; make virtual display primary moves it to the stream. Saved separately per PC.",
    "seekbar_virtual_display_scale": "Change the PC's render resolution relative to the stream. Desktop text size stays as set on the host.",
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
    "list_three_finger_tap": "Choose System keyboard (default), PC keyboard, Keyboard & mouse, Middle click or Off. Works in Trackpad, Direct mouse and Local cursor modes.",
    "list_four_finger_tap": "Choose Keyboard & mouse, System keyboard, PC keyboard, Middle click or Off (default). Four fingers never also trigger the three-finger action.",
    "seekbar_mouse_speed": "Adjust physical relative mouse movement; 100% is normal speed.",
    "checkbox_mouse_nav_buttons": "Enable to send mouse Back/Forward buttons. Disable if it interferes with right-click.",
    "checkbox_absolute_mouse_mode": "Enable for desktop pointer acceleration. Leave off for game mouse look. Available on Android 8+ except devices using the separate raw pointer path.",
    "checkbox_input_only_any_host": "Enable to add Input-only to a game's menu on PCs whose host has no Input-only mode of its own (Sunshine, GameStream, Apollo with its input-only mode off). The PC still streams video and audio at 640x360, 30 FPS and 500 kbps, about 0.7 Mbit/s with overhead; Nightjar discards both. Hosts with their own mode keep using their Input-only tile. See [Input-only on any PC](controls.md#input-only-on-any-pc).",
    "keyboard_shortcuts_editor": "Create, edit or delete up to 32 named keyboard chords for the extra keyboard toolbar. Import/export JSON files to transfer them; importing asks before replacing the saved list. See [custom keyboard shortcuts](controls.md#custom-keyboard-shortcuts).",
    "checkbox_back_as_meta": "Enable to send the Windows key from a physical keyboard's Back key. Android's Back gesture and TV remotes keep their local navigation; the controller menu chord remains available. See [keyboard options](controls.md#keyboard-options).",
    "checkbox_back_opens_menu": "Enable for a handheld's dedicated physical Back key, such as on AYN Thor, to open the stream menu instead of being forwarded as a controller button. Off preserves its existing mapping. Select/Start, mouse right-click and Keyboard Back sends Windows key keep their behavior. Applies from the next stream.",
    "checkbox_right_alt_as_meta": "Enable on a Mac host so Right Alt acts as Command (the Windows key). Left Alt is unchanged; leave off if you type with AltGr.",
    "list_pc_keyboard_layout": "Pick the legends of the on-screen PC keyboard. Automatic follows the system language (German or English). Keys send positions, so the PC's Windows layout decides the character: set both alike.",
    "seekbar_pc_keyboard_size": "How much of the screen height the PC keyboard may take, 30-70%. Smaller leaves more of the stream visible; larger gives bigger keys.",
    "seekbar_pc_keyboard_opacity": "How opaque the PC keyboard is, 30-100%. Lower shows more of the stream behind it.",
    "checkbox_show_onscreen_controls": "Enable a touchscreen gamepad. The group is hidden on devices without a touchscreen.",
    "checkbox_vibrate_osc": "Enable feedback when pressing virtual buttons.",
    "checkbox_only_show_L3R3": "Enable when another controller supplies everything except stick clicks.",
    "checkbox_show_guide_button": "Enable to expose the virtual Guide/Home button.",
    "seekbar_osc_opacity": "Lower to see more of the game beneath the controls.",
    "reset_osc": "Confirm to restore virtual button sizes and positions. This layout is kept separately from a global settings reset.",
    "checkbox_enable_perf_overlay": "Choose Compact, Statistics or Debug while diagnosing frame rate, network loss or delay; Off hides the overlay. Compact is one line: rendered FPS, network round trip (RTT) and frame loss behind a green, amber or red dot. Statistics summarizes the stream with a status line of lasting facts. Debug shows detailed measured stages and is always available. Stored as `checkbox_enable_perf_overlay` (On for any choice but Off) and `performance_overlay_mode` (compact, statistics or debug; Off keeps the last mode). Can also be changed in the stream menu → Display → Overlay and diagnostics; Copy stats includes Debug details.",
    "gpu_driver": "Import and select a compatible Snapdragon Vulkan driver for PyroWave. Shown on arm64 Adreno devices only. The system driver remains the default; changes apply to the next stream.",
    "list_audio_config": "Choose 5.1 or 7.1 for a matching audio output and host configuration; use stereo for headphones or two speakers.",
    "checkbox_host_audio": "Enable to hear audio on the PC as well as Android.",
    "checkbox_enable_mic": "Enable to talk in games and chat apps on the PC. Asks for microphone access; if it is not allowed, the setting stays off. The PC hears it as its streaming microphone. Mute it from the stream menu; the overlay shows whether it is sending.",
    "checkbox_enable_audiofx": "Enable for Android equalizers or effects. Can add delay and changes the audio output path.",
    "checkbox_enable_post_stream_toast": "Enable to see the average decode time after a session ends; it is not end-to-end latency.",
    "checkbox_enable_pip": "Enable to keep a small video window when leaving the app. Input and overlay are hidden there; requires device support.",
    "checkbox_auto_reconnect": "Resume the same running game after transport failures or returning from screen lock, up to five attempts within one minute. Back or leaving during a retry cancels it. Graceful termination, pairing, permissions and decoder failures stay manual; recovery never launches or replaces a game.",
    "performance_overlay_position": "Choose the corner the performance overlay starts in. It moves to a free corner when on-screen controls would cover it, and always stays inside the camera cutout. Right-hand corners align the text to the right. Can also be changed in the stream menu → Display → Overlay and diagnostics → Position, which takes effect at once.",
    "list_network_grace": "Off by default: the stream ends after 10 seconds without contact, as before. 20, 30 or 60 seconds keeps it open through a Wi-Fi roam, a short dropout or a VPN moving between Wi-Fi and mobile data; the picture freezes until the network returns. Longer waits only help if the host waits too, so raise its own client timeout. When the wait runs out the stream ends with a message saying so, and Recover interrupted streams, if on, then reconnects. See [network drops](video-and-latency.md#network-drops).",
    "checkbox_client_roaming": "On by default. Experimental: when this device changes networks, the stream moves to the new address instead of waiting for the old one. Only with the Rust engine and a Rubylight PC that offers client roaming; other PCs keep the network grace period.",
    "checkbox_roaming_metered": "Off by default. Lets client roaming move the stream onto mobile data or another metered network. Leave off unless you want streams to keep running on mobile data; streaming uses a lot of data.",
    "list_languages": "Choose the interface language. Android 13+ uses the system per-app language setting.",
    "checkbox_small_icon_mode": "Enable to fit more games in the library grid.",
    "checkbox_console_home": "Enable to open on Home: the games to continue, your library across hosts and the hosts. Off opens the list of PCs. Home appears only once a host with the native API has shown its games.",
    "checkbox_quick_resume": "Enable to go straight back into the running game when you tap its PC, skipping the library. Use the PC's menu for the library while a game runs. Off keeps the tap opening the library.",
    "help": "Open the bundled help: connecting a PC, pairing, controls and the stream menu, common fixes and problem reports. Works offline; each topic links to these documents for detail.",
    "latency_benchmark": "Run a 10-second local frame-callback, timer and input-dispatch benchmark without a host. Use Measure input latency in the stream menu for end-to-end timing.",
    "report_problem": "Preview the explanation and share a redacted event/crash report through Android's share sheet.",
    "checkbox_check_updates": "Leave on to look for a newer release on GitHub when the PC list opens, at most every 12 hours; you choose whether to install it. Hidden when the app was installed from Google Play, which updates it.",
    "check_updates_now": "Look for a newer release now, even if the automatic check ran recently. A release you skipped is shown again. Hidden when the app was installed from Google Play.",
    "about_app": "View the installed app version and open Source and licenses.",
    "reset_all": "Confirm to reset global preferences. Keeps pairing, PC profiles, controller mappings, control layout and language.",
    "frame_pacing": "Use Lowest latency for response, Balanced for cadence, Balanced with FPS limit to cap pacing, or Smoothest video to retain frames at the cost of delay. Greyed out while Host frame times or Adaptive refresh is on.",
    "checkbox_codec_low_latency": "Leave on for supported Android 11+ decoders. Disable when comparing a decoder freeze or artifact.",
    "checkbox_vendor_low_latency": "Leave on for supported chipset decoder keys. Disable to isolate vendor-specific instability.",
    "checkbox_disable_hevc_low_latency": "On by default. Turn off if HEVC video stutters or freezes to omit Android and vendor low-latency flags on any device. H.264, AV1, reference-frame invalidation and decoder performance hints keep their own settings. Stored the other way round as `checkbox_disable_hevc_low_latency`: Off stores true, so older saved values and profiles keep their meaning. Applies to the next stream and decoder benchmark; changing it invalidates previous benchmark results.",
    "checkbox_codec_performance": "Leave on for decoder priority/rate hints on Android 6+. Disable to compare heat or battery use.",
    "checkbox_phone_performance_hints": "Enable for a measured comparison on Android 12+; compare decode time with it on and off.",
    "checkbox_gpu_max_clocks": "Off by default. Optionally request higher Adreno GPU clocks during PyroWave streaming to reduce decode time; this can increase power use and heat. Thermal protection remains active. Installs that already stored a choice keep it. Shown on Adreno devices only.",
    "checkbox_drop_late_frames": "Enable with Balanced pacing to discard older queued outputs. Favors response over smoothness. Shown beside Frame pacing only while it is Balanced.",
    "checkbox_unbatched_input": "Enable to bypass display-rate input batching. High-rate input can increase CPU and network load.",
    "checkbox_network_priority": "Enable to compare scheduling under load. Requests streaming thread priority on this device.",
    "checkbox_texture_view": "Try for black or flickering video or misplaced output. Adds composition work and is disabled for HDR, 10-bit color (SDR) and PyroWave.",
    "checkbox_reduce_refresh_rate": "Enable to allow a lower screen refresh for power savings.",
    "checkbox_full_range": "Use when the host and display both send full-range color, for full shadow and highlight detail.",
    "checkbox_usb_driver": "Leave on for built-in Xbox USB support. Android requests USB access when needed.",
    "checkbox_usb_dualsense": "Enable for direct USB DualSense/Edge input, feedback and adaptive triggers. Accept the explanation and Android USB permission during a stream; Bluetooth retains Android's input path.",
    "checkbox_usb_dualsense_haptics": "Enable alongside the USB DualSense driver for native waveform feedback. Hosts that support it (currently Rubylight) also forward controller speaker, headset and microphone audio. Microphone streaming needs its separate setting and Android permission; the controller mute button follows the stream mute. Reconnect after changing this option. USB only.",
    "checkbox_usb_switch_pro": "Experimental. Enable to read a wired Nintendo Switch Pro Controller directly: calibrated sticks, Home and Capture, motion, rumble, player lights and battery. Accept Android's USB permission during a stream; Bluetooth and Joy-Con keep Android's input path. See [Switch Pro over USB](controls.md#switch-pro-over-usb).",
    "checkbox_usb_bind_all": "Enable to prefer the app's built-in Xbox USB driver even when Android already recognizes the controller.",
    "checkbox_enable_sops": "Leave on. Tells the PC whether it may optimize a game's settings for this stream. Rubylight only passes the choice on to commands you run on the PC, as SUNSHINE_CLIENT_ENABLE_SOPS; other hosts may change the game's settings themselves.",
    "checkbox_disable_warnings": "Enable to hide stream connection warnings once your connection is tuned.",
    "checkbox_rust_engine": "On by default. Streams with Nightjar's new engine (written in Rust), including PyroWave and remote monitor sessions. Turn it off to use the classic engine (moonlight-common-c) if a stream fails to start or misbehaves, and report the problem. Hosts older than GFE 3.22, and hosts that report no codec support, use the classic engine automatically. Installs that stored the old default were switched on once; turning it off again is kept. Applies from the next stream.",
    "experimental_packed_444": "Off by default. Requires the new streaming engine, HEVC or AV1, and Rubylight's experimental packed 4:4:4 option. Android must pass the raw YUV sampling probe; HDR also needs a compatible 10-bit output. Unsupported requests fail with a reason instead of falling back. Physical HDR output and full end-to-end performance remain unverified.",
    "export_latency_csv": "Choose a destination for the latest local frame-timing CSV. Each row holds the measured stage timings.",
    "low_latency_decoding": "Leave on. Sets Android low-latency mode, Chipset low-latency mode and Phone performance hints together; turn off only to test a decoding problem. The individual switches are behind Customize; the row says when one is off.",
    "low_latency_customize": "Opens or closes the individual switches under Low-latency decoding: Android low-latency mode, Chipset low-latency mode, Phone performance hints, Decoder performance hints and HEVC low-latency flags. Settings search finds them while it is closed.",
    "checkbox_tell_host_wireless": "On by default. Tells a Rubylight PC when this device streams over Wi-Fi, so it paces the stream for Wi-Fi from the start. Other PCs are not told.",
    "checkbox_tell_host_link_speed": "On by default. Tells a Rubylight PC this device's wired port speed, so it never sends faster than the port can take. Other PCs are not told.",
    "pin_lock": "Off until a PIN is set. Asks for a PIN to open Nightjar or to start a stream. A forgotten PIN can only be reset by clearing Nightjar's app data in Android's settings, which also removes paired PCs.",
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
        elif key == "checkbox_always_dark":
            default = array("@array/theme_choice_names")[0]
            choices = "; ".join(array("@array/theme_choice_names"))
        elif key == "checkbox_enable_perf_overlay":
            default = array("@array/overlay_choice_names")[0]
            choices = "; ".join(array("@array/overlay_choice_names"))
        elif key == "low_latency_customize":
            default = "Closed"
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
