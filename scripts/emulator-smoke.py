"""Run after assembleNonRootDebug, or select an APK with --apk.

Needs Python 3 and an Android 35+ AVD (Google APIs, not Google Play).
Uses a disposable, read-only AVD session and a loopback serverinfo fixture.
This checks UI rendering, navigation, manual discovery and the pairing prompt.
Debug builds also drive the demo stream's real stream menu, hint and overlay.
Pairing completion and a live stream require a real host and phone.
"""
import argparse
import http.server
import io
import os
from pathlib import Path
import re
import struct
import subprocess
import threading
import time
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parents[1]
SDK = Path(os.environ.get("ANDROID_HOME", Path(os.environ.get("LOCALAPPDATA", "")) / "Android/Sdk"))
ADB = SDK / "platform-tools" / ("adb.exe" if os.name == "nt" else "adb")
EMULATOR = SDK / "emulator" / ("emulator.exe" if os.name == "nt" else "emulator")
SERIAL = "emulator-5554"
PACKAGE = "com.butterpollo.client"
SHOTS = ROOT / "docs/screenshots/ui-v2"
LOGS = ROOT / "app/build/emulator-smoke"
HOST_NAME = "Gaming PC"
ADDRESS_REQUIRED = "Enter an IP address or PC name"
PC_SETTINGS = "PC settings\u2026"
DEBUGGABLE = True
EXPECTED_ROTATION = 0
UI_CASES = {
    "393dp-font-1": (393, 1.0, 0),
    "412dp-font-1": (412, 1.0, 0),
    "393dp-font-1.3": (393, 1.3, 0),
    "393dp-font-2": (393, 2.0, 0),
    "393dp-font-2-landscape": (393, 2.0, 1),
}


def adb(*args, timeout=30, binary=False):
    result = subprocess.run([str(ADB), "-s", SERIAL, *args], check=True,
                            capture_output=True, timeout=timeout)
    return result.stdout if binary else result.stdout.decode("utf-8", errors="replace")


def set_rotation(rotation):
    """Persist rotation before locking it so settings reloads/UIAutomation cannot restore an old value."""
    global EXPECTED_ROTATION
    adb("shell", "settings", "put", "system", "accelerometer_rotation", "0")
    adb("shell", "settings", "put", "system", "user_rotation", str(rotation))
    adb("shell", "wm", "user-rotation", "lock", str(rotation))
    EXPECTED_ROTATION = rotation
    # UIAutomation saves/restores the *actual* display rotation when it connects.
    # Wait for the window transition without connecting UIAutomation, or it can
    # save the portrait launcher's old rotation and undo the requested landscape.
    deadline = time.monotonic() + 15
    matched_since = None
    while time.monotonic() < deadline:
        png = adb("exec-out", "screencap", "-p", binary=True)
        width, height = struct.unpack(">II", png[16:24])
        if (width > height) == (rotation in (1, 3)):
            if matched_since is None:
                matched_since = time.monotonic()
            elif time.monotonic() - matched_since >= 0.5:
                return
        else:
            matched_since = None
        time.sleep(0.25)
    raise AssertionError(f"Display did not reach rotation {rotation}: {width}x{height}")


def tree():
    for attempt in range(5):
        try:
            adb("shell", "uiautomator", "dump", "/sdcard/nightjar-smoke.xml")
            return ET.fromstring(adb("shell", "cat", "/sdcard/nightjar-smoke.xml"))
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired, ET.ParseError):
            if attempt == 4:
                raise
            time.sleep(2)


def bounds(node):
    return tuple(map(int, re.findall(r"-?\d+", node.attrib["bounds"])))


def wait_for_system_process(root):
    """On a busy host, System UI can miss its deadline and Android offers to close it; keep waiting."""
    button = next((n for n in root.iter("node") if n.get("resource-id") == "android:id/aerr_wait"), None)
    if button is None:
        return False
    x1, y1, x2, y2 = bounds(button)
    adb("shell", "input", "tap", str((x1+x2)//2), str((y1+y2)//2))
    time.sleep(1)
    return True


def find(label, scroll=False):
    previous_scroll_state = None
    unchanged_scrolls = 0
    for attempt in range(64 if scroll else 8):
        root = tree()
        if any(n.get("resource-id") == "android:id/immersive_cling_title" for n in root.iter("node")):
            button = next(n for n in root.iter("node") if n.get("resource-id") == "android:id/ok")
            x1, y1, x2, y2 = bounds(button)
            adb("shell", "input", "tap", str((x1+x2)//2), str((y1+y2)//2))
            continue
        if wait_for_system_process(root):
            continue
        for node in root.iter("node"):
            text = node.get("text") or ""
            description = node.get("content-desc") or ""
            if label in (text, text.split("\n")[0], description, node.get("resource-id")) or \
                    description.startswith(label + ", "):
                x1, y1, x2, y2 = bounds(node)
                x1, y1 = max(0, x1), max(0, y1)
                # UIAutomator can expose rows behind a system bar; tap only their visible part.
                for bar in root.iter("node"):
                    if bar.get("resource-id") in ("android:id/navigationBarBackground", "android:id/statusBarBackground"):
                        bx1, by1, bx2, by2 = bounds(bar)
                        if by1 <= y1 and by2 >= y2:
                            if bx1 <= x1 < bx2:
                                x1 = bx2
                            elif bx1 < x2 <= bx2:
                                x2 = bx1
                        if bx1 <= x1 and bx2 >= x2:
                            if by1 <= y1 < by2:
                                y1 = by2
                            elif by1 < y2 <= by2:
                                y2 = by1
                if x1 < x2 and y1 < y2:
                    node.set("bounds", f"[{x1},{y1}][{x2},{y2}]")
                    return node
        if scroll:
            state = ET.tostring(root)
            unchanged_scrolls = unchanged_scrolls + 1 if state == previous_scroll_state else 0
            if unchanged_scrolls >= 3:
                break
            previous_scroll_state = state
            area = next((n for n in root.iter("node") if n.get("scrollable") == "true"), None)
            if area is not None:
                x1, y1, x2, y2 = bounds(area)
                start, end = y1+(y2-y1)*3//4, y1+(y2-y1)//3
                if scroll == "up":
                    start, end = end, start
                adb("shell", "input", "swipe", str((x1+x2)//2), str(start), str((x1+x2)//2), str(end), "300")
        time.sleep(0.5)
    LOGS.joinpath("missing-node.xml").write_bytes(ET.tostring(root))
    raise AssertionError(f"UI node missing: {label}")


def find_text(pattern):
    for _ in range(8):
        for node in tree().iter("node"):
            if re.search(pattern, node.get("text") or ""):
                return node
        time.sleep(0.5)
    texts = [n.get("text") for n in tree().iter("node") if n.get("text")]
    raise AssertionError(f"No UI text matches: {pattern}; on screen: {texts[:30]}")


def native_resolution():
    """The panel's largest mode, landscape first, as the app stores it."""
    sizes = re.findall(r"width=(\d+), height=(\d+)", adb("shell", "dumpsys", "display"))
    width, height = max(((int(w), int(h)) for w, h in sizes), key=lambda size: size[0] * size[1])
    return f"{max(width, height)}x{min(width, height)}"


def pref_value(root, name):
    node = next((n for n in root if n.get("name") == name), None)
    return None if node is None else node.get("value", node.text)


def tap(label, scroll=False):
    x1, y1, x2, y2 = bounds(find(label, scroll))
    adb("shell", "input", "tap", str((x1+x2)//2), str((y1+y2)//2))


def navigate_up():
    """Settings names its back action for the destination; other screens use Navigate up."""
    if any(n.get("resource-id") == f"{PACKAGE}:id/settings_back" for n in tree().iter("node")):
        tap(f"{PACKAGE}:id/settings_back")
    else:
        tap("Navigate up")


def open_help():
    """Help left the PCs toolbar: Settings, App, Help from the shell bar."""
    tap(f"{PACKAGE}:id/shell_settings")
    tap("App", scroll=True)
    tap("Help", scroll=True)
    find("Connect a PC")


def close_help():
    """Back out of Help, the App section and the Settings list to the PCs."""
    for _ in range(3):
        navigate_up()
        if any(n.get("resource-id") == f"{PACKAGE}:id/home_toolbar" for n in tree().iter("node")):
            return
    find(f"{PACKAGE}:id/home_toolbar")


def enter_text(label, text):
    field = find(label)
    tap(label)
    adb("shell", "input", "keyevent", "123", *("67" for _ in range(len(field.get("text", "")) + 1)))
    if text:
        adb("shell", "input", "text", text)


def assert_touch_target(label, minimum_dp=48):
    node = find(label)
    x1, y1, x2, y2 = bounds(node)
    # Every configured smoke viewport uses 320 dpi (2 physical pixels per dp).
    assert min(x2 - x1, y2 - y1) >= minimum_dp * 2, f"Small touch target: {node.attrib}"


def scroll_to_end():
    """Scroll a visible UI-tree container until its children stop moving."""
    previous = None
    unchanged = 0
    for _ in range(64):
        root = tree()
        state = ET.tostring(root)
        unchanged = unchanged + 1 if state == previous else 0
        if unchanged >= 2:
            return
        previous = state
        area = next((n for n in root.iter("node") if n.get("scrollable") == "true"), None)
        if area is None:
            return
        x1, y1, x2, y2 = bounds(area)
        x = str((x1 + x2) // 2)
        adb("shell", "input", "swipe", x, str(y1 + (y2 - y1) * 9 // 10),
            x, str(y1 + (y2 - y1) // 10), "300")
    raise AssertionError("Scrollable UI did not reach its end after 64 swipes")


def system_bar_frames():
    """Visible status and navigation bar frames from the window manager, in screen pixels."""
    frames = {"statusBars": [], "navigationBars": []}
    for kind, x1, y1, x2, y2 in re.findall(
            r"InsetsSource id=\S+ type=(statusBars|navigationBars) frame=\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\] visible=true",
            adb("shell", "dumpsys", "window")):
        frame = tuple(map(int, (x1, y1, x2, y2)))
        if frame[2] > frame[0] and frame[3] > frame[1] and frame not in frames[kind]:
            frames[kind].append(frame)
    return frames


def assert_top_bar_clear(toolbar_id, action_id=None, root=None):
    """The top app bar must start below the status bar and show its title and actions."""
    root = root if root is not None else tree()
    status_bottom = max((f[3] for f in system_bar_frames()["statusBars"] if f[1] == 0), default=0)
    bar = next((n for n in root.iter("node") if n.get("resource-id") == f"{PACKAGE}:id/{toolbar_id}"), None)
    assert bar is not None, f"Top app bar {toolbar_id} is missing"
    x1, y1, x2, y2 = bounds(bar)
    assert y1 >= status_bottom, f"Top app bar {toolbar_id} starts at {y1}, under the status bar ending at {status_bottom}"
    assert any(n.get("text") for n in bar.iter("node")), f"Top app bar {toolbar_id} has no visible title"
    if action_id is not None:
        action = next((n for n in bar.iter("node") if n.get("resource-id") == f"{PACKAGE}:id/{action_id}"), None)
        assert action is not None, f"Top app bar action {action_id} is missing"
        ax1, ay1, ax2, ay2 = bounds(action)
        assert ay1 >= status_bottom and ax2 > ax1 and ay2 > ay1, f"Top app bar action {action_id} is not visible: {action.attrib}"
    return root


def assert_cold_start_insets(runs=5):
    """Force-stop and relaunch: the home bar must clear the status bar every time, not only after a config change."""
    for run in range(runs):
        adb("shell", "am", "force-stop", PACKAGE)
        adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.PcView")
        find(f"{PACKAGE}:id/home_toolbar")
        root = assert_top_bar_clear("home_toolbar")
        shell = find(f"{PACKAGE}:id/shell_settings")
        assert bounds(shell)[1] >= bounds(find(f"{PACKAGE}:id/home_toolbar"))[3] - 1, "The shell bar overlaps the top app bar"
        content = next((n for n in root.iter("node") if n.get("resource-id") in
                        (f"{PACKAGE}:id/no_pc_found_layout", f"{PACKAGE}:id/fragmentView")), None)
        nav_top = min((f[1] for f in system_bar_frames()["navigationBars"] if f[1] > 0), default=None)
        if content is not None and nav_top is not None:
            assert bounds(content)[3] <= nav_top, \
                f"Cold start {run + 1}: content ends at {bounds(content)[3]}, under the navigation bar at {nav_top}"
    print(f"Cold start keeps the home bar clear of the system bars ({runs} launches)", flush=True)


def focused_id(root=None):
    root = root if root is not None else tree()
    node = next((n for n in root.iter("node") if n.get("focused") == "true"), None)
    return None if node is None else node.get("resource-id")


def assert_dpad_reaches_home_actions():
    """D-pad and Tab must reach the shell bar from the home content, not only the list and Add PC."""
    adb("shell", "input", "keyevent", "KEYCODE_DPAD_DOWN")
    for _ in range(4):
        if focused_id() not in (f"{PACKAGE}:id/shell_home", f"{PACKAGE}:id/shell_pcs", f"{PACKAGE}:id/shell_settings", None):
            break
        adb("shell", "input", "keyevent", "KEYCODE_DPAD_DOWN")
    content = focused_id()
    assert content not in (None, f"{PACKAGE}:id/shell_home", f"{PACKAGE}:id/shell_pcs", f"{PACKAGE}:id/shell_settings"),         f"D-pad did not reach the home content: {content}"
    adb("shell", "input", "keyevent", "KEYCODE_DPAD_UP", "KEYCODE_DPAD_UP", "KEYCODE_DPAD_UP")
    reached = focused_id()
    assert reached in (f"{PACKAGE}:id/shell_home", f"{PACKAGE}:id/shell_pcs", f"{PACKAGE}:id/shell_settings"),         f"D-pad up from the home content stopped at {reached} instead of the shell bar"
    print(f"D-pad reaches the top app bar from {content.split('/')[-1]}", flush=True)


def host_menu(item=None):
    x1, y1, x2, y2 = bounds(find(HOST_NAME))
    x, y = str((x1+x2)//2), str((y1+y2)//2)
    adb("shell", "input", "swipe", x, y, x, y, "1000")
    if item is not None:
        tap(item, scroll=True)


PROFILE_RESET = "Same as all PCs for everything"
PROFILES_ORDER = ("When a stream starts, each value comes from the first of these that sets it: this game, this PC, "
                  "the profile, then the main settings. On mobile data, Bitrate on mobile data then caps the bitrate. "
                  "Changes never apply to a stream that is already running.")


def open_host_profile():
    host_menu(PC_SETTINGS)
    find("Stream resolution", scroll=True)


def uncheck(predicate):
    node = next((n for n in tree().iter("node") if n.get("checked") == "true" and predicate(n.get("text") or "")), None)
    if node is not None:
        x1, y1, x2, y2 = bounds(node)
        adb("shell", "input", "tap", str((x1+x2)//2), str((y1+y2)//2))


def profile_number(label, value):
    rows = {"Width": "Stream resolution", "Height": "Stream resolution",
            "Frame rate (FPS)": "Video frame rate", "Bitrate (Mbps)": "Video bitrate"}
    fields = [n for n in tree().iter("node") if n.get("class") == "android.widget.EditText"
              and n.get("content-desc") == label]
    if not fields:
        tap(rows[label], scroll=True)
        # Every value starts as "Same as all PCs"; a value of this PC's own needs that box off.
        uncheck(lambda text: text.startswith("Same as all PCs"))
        # The resolution may follow the phone's screen; a fixed size needs that choice off too.
        uncheck(lambda text: text == "Screen size (follows this screen)")
        fields = [n for n in tree().iter("node") if n.get("class") == "android.widget.EditText"
                  and n.get("content-desc") == label]
    field = fields[0]
    x1, y1, x2, y2 = bounds(field)
    adb("shell", "input", "tap", str((x1+x2)//2), str((y1+y2)//2))
    adb("shell", "input", "keyevent", "123", *("67" for _ in range(len(field.get("text", "")))))
    adb("shell", "input", "text", value)
    adb("shell", "input", "keyevent", "4")
    tap("android:id/button1")


def prefs(name, check, message):
    """SharedPreferences.apply() writes asynchronously, so poll briefly before failing."""
    for _ in range(20):
        try:
            if DEBUGGABLE:
                contents = adb("shell", "run-as", PACKAGE, "cat", f"shared_prefs/{name}.xml")
            else:
                contents = adb("shell", "cat", f"/data/user/0/{PACKAGE}/shared_prefs/{name}.xml")
            root = ET.fromstring(contents)
            if check(root):
                return root
        except (subprocess.SubprocessError, ET.ParseError):
            pass
        time.sleep(0.5)
    raise AssertionError(message)


def screenshot(name):
    ui = tree()
    png = adb("exec-out", "screencap", "-p", binary=True)
    width, height = struct.unpack(">II", png[16:24])
    try:
        # Retain native dimensions for font and touch-target review.
        from PIL import Image
        image = Image.open(io.BytesIO(png))
        image.quantize(128).save(SHOTS.joinpath(name + ".png"), optimize=True)
    except ImportError:
        SHOTS.joinpath(name + ".png").write_bytes(png)
    LOGS.joinpath(name + ".xml").write_bytes(ET.tostring(ui))
    assert (width > height) == (EXPECTED_ROTATION in (1, 3)), \
        f"Wrong screenshot orientation for {name}: {width}x{height}, expected rotation {EXPECTED_ROTATION}"
    assert int(ui.get("rotation", "-1")) == EXPECTED_ROTATION, \
        f"UI hierarchy has rotation {ui.get('rotation')}, expected {EXPECTED_ROTATION} for {name}"
    print(f"Captured {name}", flush=True)
    return width, height


def frontend_entry(name, contents):
    entry = LOGS / f"nightjar-smoke-{name}.art"
    entry.write_text(contents, encoding="utf-8", newline="\n")
    remote = f"/sdcard/Download/{entry.name}"
    adb("push", str(entry), remote)
    adb("shell", "am", "broadcast", "-a", "android.intent.action.MEDIA_SCANNER_SCAN_FILE",
        "-d", f"file://{remote}")
    for _ in range(30):
        rows = adb("shell", "content", "query", "--uri", "content://media/external/downloads",
                   "--projection", "_id:_display_name")
        match = re.search(rf"\b_id=(\d+), _display_name={re.escape(entry.name)}\r?$", rows, re.MULTILINE)
        if match:
            uri = f"content://media/external/downloads/{match[1]}"
            assert adb("exec-out", "content", "read", "--uri", uri, binary=True) == entry.read_bytes(), \
                f"Content URI does not contain the fixture: {uri}"
            return uri
        time.sleep(1)
    raise AssertionError(f"MediaStore did not index {remote}: {rows}")


def capture_ui_set(width, font, rotation=0):
    prefix = f"{width}dp-font-{font:g}" + ("-landscape" if rotation else "")
    adb("shell", "am", "force-stop", PACKAGE)
    adb("shell", "wm", "size", f"{width * 2}x1704")
    adb("shell", "wm", "density", "320")
    adb("shell", "settings", "put", "system", "font_scale", str(font))
    adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.PcView")
    set_rotation(rotation)
    find(HOST_NAME)
    screenshot(prefix + "-home-host")
    tap(f"More options for {HOST_NAME}")
    close = f"{PACKAGE}:id/action_sheet_close"
    assert find(close).get("content-desc") == "Close menu", "Sheet close button has no accessible name"
    assert_touch_target(close)
    find("Pair with one-time PIN")
    screenshot(prefix + "-pc-sheet")
    # A long sheet must scroll through its final action without dragging it closed.
    find("Delete PC", scroll=True)
    scroll_to_end()
    assert_touch_target("Delete PC")
    assert_touch_target(close)
    screenshot(prefix + "-pc-sheet-bottom")
    tap(close)
    open_host_profile()
    assert find(PROFILE_RESET, scroll=True).get("enabled") == "false", "A new PC should follow the settings for all PCs"
    find("Stream resolution", scroll="up")
    screenshot(prefix + "-per-pc")
    find("View details", scroll=True)
    screenshot(prefix + "-per-pc-host")
    navigate_up()
    open_help()
    screenshot(prefix + "-help")
    # Every topic links out, down to the last one.
    find("What to include", scroll=True)
    # The last link can be found while the bottom edge still clips it; measure it fully shown.
    scroll_to_end()
    assert_touch_target("What to include")
    close_help()
    tap(f"{PACKAGE}:id/shell_settings")
    find("Presets", scroll=True)
    screenshot(prefix + "-settings-root")
    assert not any(n.get("text") == "Developer options" for n in tree().iter("node")), \
        "Settings still has a separate Developer options section"
    for title, name, setting in (("Video", "video", "Stream resolution"),
                                 ("Display & image", "display", "Frame pacing"),
                                 ("Controls", "controls", "Controller buttons"),
                                 ("Advanced", "advanced", "Low-latency decoding")):
        tap(title, scroll=True)
        find(setting, scroll=True)
        screenshot(prefix + "-" + name)
        navigate_up()
    # Named profiles: the list, the choice by network and the precedence order on one screen.
    tap("Video", scroll=True)
    tap("Profiles", scroll=True)
    find("New profile", scroll=True)
    screenshot(prefix + "-profiles")
    find("Choose by network", scroll=True)
    find(PROFILES_ORDER, scroll=True)
    screenshot(prefix + "-profiles-order")
    navigate_up()
    find("Bitrate on mobile data", scroll=True)
    navigate_up()
    find("Presets", scroll=True)
    search = f"{PACKAGE}:id/search_src_text"
    # Search sits behind a top app bar action until it is used.
    assert not any(n.get("resource-id") == search for n in tree().iter("node")), "Settings search is open before it was asked for"
    tap(f"{PACKAGE}:id/settings_search_button")
    for _ in range(5):
        # Typing can race the field getting focus right after the tap; retry until the query is in the field.
        tap(search)
        time.sleep(0.5)
        adb("shell", "input", "keyevent", "123", *("67" for _ in range(20)))
        adb("shell", "input", "text", "sharpness")
        if find(search).get("text") == "sharpness":
            break
    else:
        raise AssertionError("Settings search did not take the query")
    # Enter submits, which keeps the query and hides the keyboard; Back would clear the search if the keyboard was already gone.
    adb("shell", "input", "keyevent", "66")
    find("Upscaling sharpness", scroll=True)
    # The group above a result names the section once, then its group.
    find("Display & image · Image", scroll=True)
    screenshot(prefix + "-settings-search")
    tap("Upscaling sharpness", scroll=True)
    find("Display & image")
    find("Upscaling sharpness", scroll=True)
    navigate_up()
    assert find(search).get("text") == "sharpness", "Back from a result lost its search query"
    find("Search results")
    navigate_up()
    assert not any(n.get("resource-id") == search and n.get("text") == "sharpness" for n in tree().iter("node")),         "Back from search did not clear its query"
    find("Presets", scroll=True)
    navigate_up()
    # After a few minutes in settings the home screen times out the cached host state and polls again;
    # an unknown host opens its menu instead of pairing, so wait for the fixture to report back.
    find("Not paired")
    tap(HOST_NAME)
    find("Pair with PC")
    screenshot(prefix + "-pairing-pin")
    tap("android:id/button2")
    find(HOST_NAME)
    host_menu("Delete PC")
    tap("android:id/button1")
    find(f"{PACKAGE}:id/discovery_add", scroll=True)
    screenshot(prefix + "-home-empty")
    tap(f"{PACKAGE}:id/discovery_add", scroll=True)
    tap(f"{PACKAGE}:id/addPcButton", scroll=True)
    # A field-level error persists after the transient toast would have disappeared.
    time.sleep(2)
    find(ADDRESS_REQUIRED, scroll="up")
    tap(f"{PACKAGE}:id/hostTextView", scroll="up")
    adb("shell", "input", "text", "127.0.0.1:47989")
    assert not any(n.get("text") == ADDRESS_REQUIRED for n in tree().iter("node")), \
        "Editing a valid host address did not clear the inline error"
    adb("shell", "input", "keyevent", "66")
    find(HOST_NAME)
    if DEBUGGABLE:
        adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.LatencyOverlaySmokeActivity")
        find(f"{PACKAGE}:id/performanceOverlay")
        screenshot(prefix + "-overlay-statistics")
        adb("shell", "input", "keyevent", "4")


def library_regressions():
    """The debug fixture supplies real AppView cards without pairing a synthetic host."""
    adb("shell", "am", "force-stop", PACKAGE)
    adb("shell", "wm", "size", "786x1704")
    adb("shell", "settings", "put", "system", "font_scale", "2.0")
    adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.demo.DemoLauncherActivity",
        "--es", "state", "library", "--es", "codec", "h264")
    set_rotation(0)
    find("Racing Game", scroll=True)
    screenshot("library-font-2")
    # Rotating without a query changes the column count; the grid used to come back empty.
    set_rotation(1)
    cards = []
    for _ in range(6):
        cards = [n for n in tree().iter("node") if n.get("resource-id") == f"{PACKAGE}:id/grid_image"]
        if cards:
            break
        time.sleep(0.5)
    assert cards, "Library grid is blank in landscape without a search query"
    assert_top_bar_clear("library_toolbar", "library_search_button")
    screenshot("library-landscape-font-2")
    set_rotation(0)
    find("Racing Game", scroll=True)
    tap(f"{PACKAGE}:id/library_search_button")
    search = f"{PACKAGE}:id/search_src_text"
    enter_text(search, "zzznomatch")
    adb("shell", "input", "keyevent", "66")
    find("Clear search")
    screenshot("library-no-results-font-2")
    tap("Clear search")
    find("Racing Game", scroll=True)
    if not any(n.get("resource-id") == search for n in tree().iter("node")):
        tap(f"{PACKAGE}:id/library_search_button")
    enter_text(search, "Racing")
    adb("shell", "input", "keyevent", "66")
    find("Racing Game")
    assert not any("Desktop" in (n.get("text"), (n.get("content-desc") or "").split(", ")[0])
                   for n in tree().iter("node")), "Library query was not applied"
    set_rotation(1)
    find("Racing Game")
    assert find(search).get("text") == "Racing", "Rotation lost the library query"
    screenshot("library-search-landscape-font-2")
    tap("More options for Racing Game", scroll=True)
    find(f"{PACKAGE}:id/action_sheet_close")
    find("Resume")
    screenshot("library-app-sheet-landscape-font-2")
    # A game's own stream values open from its sheet and say where unset values come from.
    tap("Stream settings for this game", scroll=True)
    find("Racing Game")
    find_text(r"(?m)from the main settings$")
    screenshot("game-settings-landscape-font-2")
    find(f"{PACKAGE}:id/host_profile_save", scroll=True)
    navigate_up()
    find("Racing Game")
    adb("shell", "am", "force-stop", PACKAGE)
    adb("shell", "settings", "put", "system", "font_scale", "1.0")


def library_covers():
    """The running game leads the cover grid with its chip; a tile's menu matches the game's state."""
    adb("shell", "am", "force-stop", PACKAGE)
    adb("shell", "wm", "size", "786x1704")
    adb("shell", "settings", "put", "system", "font_scale", "1.0")
    adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.demo.DemoLauncherActivity",
        "--es", "state", "library", "--es", "codec", "h264")
    set_rotation(0)
    find("Racing Game")
    root = tree()
    tiles = [n for n in root.iter("node") if (n.get("content-desc") or "") and
             n.get("resource-id") != f"{PACKAGE}:id/grid_more" and n.get("class") != "android.widget.ImageButton" and
             any(c.get("resource-id") == f"{PACKAGE}:id/grid_image" for c in n.iter("node"))]
    assert tiles and tiles[0].get("content-desc") == "Racing Game, Running", \
        f"The running game is not pinned first: {[t.get('content-desc') for t in tiles[:3]]}"
    assert any(n.get("text") == "Running" for n in root.iter("node")), "The running game has no Running chip"
    assert not any(n.get("text") in ("Play", "Resume") for n in root.iter("node")), "Tiles still repeat their action"
    screenshot("library-covers")
    tap("More options for Puzzle Night", scroll=True)
    # The demo host confirms on its side before it replaces the running game, so the sheet only plays.
    find("Play\nYour PC asks before it closes Racing Game")
    screenshot("library-app-sheet-other-game")
    tap(f"{PACKAGE}:id/action_sheet_close")
    adb("shell", "am", "force-stop", PACKAGE)


def home_states():
    """The demo PCs cover every row state: running, unpaired and offline, each with its own short menu."""
    adb("shell", "am", "force-stop", PACKAGE)
    adb("shell", "wm", "size", "786x1704")
    adb("shell", "settings", "put", "system", "font_scale", "1.0")
    adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.demo.DemoLauncherActivity")
    set_rotation(0)
    find("Playing Racing Game")
    root = tree()
    names = [n.get("text") for n in root.iter("node") if n.get("resource-id") == f"{PACKAGE}:id/grid_text"]
    assert names == ["Living-room PC", "Studio PC", "Laptop"], f"Rows are not running, unpaired, offline: {names}"
    statuses = [n.get("text") for n in root.iter("node") if n.get("resource-id") == f"{PACKAGE}:id/grid_status"]
    assert statuses == ["Playing Racing Game", "Not paired", "Offline"], f"Unexpected row status lines: {statuses}"
    screenshot("home-states")
    for name, action, shot in (("Living-room PC", "Resume Racing Game", "pc-sheet-running"),
                               ("Studio PC", "Pairing help", "pc-sheet-unpaired"),
                               ("Laptop", "Wake PC", "pc-sheet-offline")):
        tap(f"More options for {name}")
        find(action)
        actions = [n.get("text") for n in tree().iter("node") if n.get("class") == "android.widget.Button"]
        assert len(actions) <= 7 and actions[-1] == "Delete PC", f"{name} menu: {actions}"
        screenshot(shot)
        tap(f"{PACKAGE}:id/action_sheet_close")
    # D-pad: down onto the first row, right onto its options button, Enter opens the menu.
    adb("shell", "input", "keyevent", "KEYCODE_DPAD_DOWN")
    for _ in range(3):
        if focused_id() == f"{PACKAGE}:id/fragmentView":
            break
        adb("shell", "input", "keyevent", "KEYCODE_DPAD_DOWN")
    assert focused_id() == f"{PACKAGE}:id/fragmentView", f"D-pad did not reach the PC rows: {focused_id()}"
    adb("shell", "input", "keyevent", "KEYCODE_DPAD_RIGHT", "KEYCODE_ENTER")
    find(f"{PACKAGE}:id/action_sheet_close")
    tap(f"{PACKAGE}:id/action_sheet_close")
    print("D-pad opens a PC row's menu from its options button", flush=True)
    adb("shell", "am", "force-stop", PACKAGE)


def capture_ui_sweep(cases=None):
    """Run identical responsive/navigation checks in full and focused UI runs."""
    for case in cases or UI_CASES:
        capture_ui_set(*UI_CASES[case])
    adb("shell", "settings", "put", "system", "font_scale", "1.0")
    if DEBUGGABLE:
        capture_debug_checks()


def capture_debug_checks():
    """Debug fixtures exercise real library cards and overlay renderers without a paired PC."""
    home_states()
    library_covers()
    library_regressions()
    # 786 px at 320 dpi gives a 393 dp phone viewport.
    adb("shell", "wm", "size", "786x1704")
    adb("shell", "wm", "density", "320")
    adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.LatencyOverlaySmokeActivity")
    for rotation, suffix in ((0, ""), (1, "-landscape")):
        set_rotation(rotation)
        time.sleep(1)
        node = find(f"{PACKAGE}:id/performanceOverlay")
        text = node.get("text", "")
        assert "Displayed: 119.9 FPS" in text and "Video payload: 14.2 Mbps" in text, node.attrib
        assert "Host processing: 2.10 ms" in text and "Display wait: 1.20 ms" in text, node.attrib
        assert "Host CPU: 41.6%" in text and "Host GPU 11520: 42.0%" in text, node.attrib
        width, height = screenshot("overlay-statistics" + suffix)
        assert (width < height) == (rotation == 0), "Overlay did not rotate"
        x1, y1, x2, y2 = bounds(node)
        assert 0 <= x1 < x2 <= width and 0 <= y1 < y2 <= height, node.attrib
        assert 80 < y2 - y1 < height, "Statistics should be a readable multiline panel"
        x, y = str((x1 + x2) // 2), str((y1 + y2) // 2)
        adb("shell", "input", "swipe", x, y, x, y, "1000")
        node = find(f"{PACKAGE}:id/performanceOverlay")
        text = node.get("text", "")
        for section in ("Video\n", "\n\nNetwork\n", "\n\nDecode\n", "\n\nHost\n"):
            assert section in text, node.attrib
        assert "Low latency mode: " in text, node.attrib
        assert "Queue wait: " in text, node.attrib
        # The smoke turns Wi-Fi off, so the connection line reports that rather than vanishing.
        assert re.search(r"Wi-Fi band: |Connection: |Wi-Fi details: not available", text), node.attrib
        assert "Stall rhythm: about every 2 min" in text, node.attrib
        assert "Host hardware (all GPUs)" in text and "GPU 11520 · AMD Radeon (preview)" in text, node.attrib
        assert "GPU power: 28.0 W · Board power: 40.0 W" in text, node.attrib
        screenshot("overlay-debug" + suffix)
        x1, y1, x2, y2 = bounds(node)
        x, y = str(x1 + 20), str(y1 + 20)
        adb("shell", "input", "swipe", x, y, x, y, "1000")
        # Debug, then the compact line, then Statistics again for the next rotation.
        node = find(f"{PACKAGE}:id/performanceOverlay")
        assert node.get("text") == COMPACT_TEXT, node.attrib
        x1, y1, x2, y2 = bounds(node)
        x, y = str((x1 + x2) // 2), str((y1 + y2) // 2)
        adb("shell", "input", "swipe", x, y, x, y, "1000")
    stream_health_checks()
    stream_ui_checks()


def stream_health_checks():
    """The real stream doctor text over a scripted two-minute stall rhythm."""
    adb("shell", "am", "force-stop", PACKAGE)
    for rotation, suffix in ((0, ""), (1, "-landscape")):
        # As for the overlay above: lock the rotation once the stream-themed activity is in front.
        adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.LatencyOverlaySmokeActivity",
            "--es", "mode", "health")
        set_rotation(rotation)
        time.sleep(1)
        find("Why did my stream stutter?")
        node = find_text(r"Repeating stalls: 5 network stalls in the last 20 minutes started about every 2 min")
        text = node.get("text", "")
        assert "Wi-Fi scan throttling" in text and "low-latency Wi-Fi lock" in text, node.attrib
        assert re.search(r"Connection\n(Wi-Fi band|Connection|Wi-Fi details): ", text), node.attrib
        screenshot("stream-health-rhythm" + suffix)
        adb("shell", "am", "force-stop", PACKAGE)
    set_rotation(0)


COMPACT_TEXT = "● 119.9 FPS · 8 ms RTT · 0.1% loss"


def cutout_insets(width, height):
    """The panel's camera cutout as (left, top, right, bottom) insets of the screen as captured.

    The window manager stops reporting the cutout under the smoke's wm size override, so this reads the
    panel's own cutout in its natural orientation, scales it to the capture and turns it with the
    rotation the smoke locked.
    """
    for line in adb("shell", "dumpsys", "display").splitlines():
        cutout = re.search(r"DisplayCutout\{insets=Rect\((\d+), (\d+) - (\d+), (\d+)\)", line)
        real = re.search(r"real (\d+) x (\d+)", line)
        if not cutout or not real or not any(map(int, cutout.groups())):
            continue
        left, top, right, bottom = map(int, cutout.groups())
        natural = min(map(int, real.groups()))
        scale = min(width, height) / natural
        turned = {0: (left, top, right, bottom), 1: (top, right, bottom, left),
                  2: (right, bottom, left, top), 3: (bottom, left, top, right)}[EXPECTED_ROTATION]
        return tuple(round(value * scale) for value in turned)
    return (0, 0, 0, 0)


def overlaps(a, b):
    return a[0] < b[2] and b[0] < a[2] and a[1] < b[3] and b[1] < a[3]


def assert_clear_of_cutout(rect, width, height, what):
    left, top, right, bottom = cutout_insets(width, height)
    for band in ((0, 0, left, height), (0, 0, width, top), (width - right, 0, width, height), (0, height - bottom, width, height)):
        if band[2] > band[0] and band[3] > band[1]:
            assert not overlaps(rect, band), f"{what} {rect} reaches into the camera cutout {band}"


def onscreen_controls(root, overlay, width, height):
    """The on-screen gamepad's elements and layout button: small app views without an id of their own."""
    controls = []
    for node in root.iter("node"):
        if node.get("package") != PACKAGE or node.get("resource-id") or node is overlay:
            continue
        if node.get("class") not in ("android.view.View", "android.widget.Button"):
            continue
        x1, y1, x2, y2 = bounds(node)
        if 0 < (x2 - x1) * (y2 - y1) < width * height // 4:
            controls.append((x1, y1, x2, y2))
    return controls


def assert_compact_overlay(name, controls_shown):
    """One line, out of the cutout, and in landscape off every on-screen control."""
    width, height = screenshot(name)
    root = ET.fromstring(LOGS.joinpath(name + ".xml").read_bytes())
    node = next(n for n in root.iter("node") if n.get("resource-id") == f"{PACKAGE}:id/performanceOverlay")
    assert node.get("text") == COMPACT_TEXT, node.attrib
    rect = bounds(node)
    assert rect[3] - rect[1] < 120, f"The compact overlay is more than one line: {rect}"
    assert_clear_of_cutout(rect, width, height, "The compact overlay")
    controls = onscreen_controls(root, node, width, height)
    if controls_shown:
        gear = next((n for n in root.iter("node") if n.get("content-desc") == "Edit controller layout"), None)
        assert gear is not None and controls, f"{name}: the on-screen controls did not show"
        # Phones stream controller layouts in landscape; portrait has no free spot to promise.
        if width > height:
            for control in controls:
                assert not overlaps(rect, control), f"{name}: the compact overlay {rect} covers an on-screen control {control}"
    return rect


def stream_ui_checks():
    """The real stream menu, first-stream hint and compact overlay, without a host."""
    adb("shell", "wm", "size", "786x1704")
    adb("shell", "wm", "density", "320")
    # The compact overlay alone stays top-start; beside the on-screen gamepad it moves to a free spot.
    for controls in (False, True):
        adb("shell", "am", "force-stop", PACKAGE)
        extra = ("--ez", "controls", "true") if controls else ()
        adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.LatencyOverlaySmokeActivity",
            "--es", "mode", "compact", *extra)
        for rotation, suffix in ((0, ""), (1, "-landscape"), (3, "-landscape-reverse")):
            set_rotation(rotation)
            time.sleep(1)
            find(f"{PACKAGE}:id/performanceOverlay")
            rect = assert_compact_overlay("overlay-compact" + ("-controls" if controls else "") + suffix, controls)
            if not controls:
                assert rect[0] < 200 and rect[1] < 200, f"Without controls the compact overlay left top-start: {rect}"
    adb("shell", "input", "keyevent", "4")

    # The demo stream runs the production menu, hint and overlay, with the on-screen gamepad shown.
    adb("shell", "am", "force-stop", PACKAGE)
    adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.demo.DemoLauncherActivity",
        "--es", "state", "touch", "--es", "codec", "h264", "--ez", "hint", "true")
    hint_id = f"{PACKAGE}:id/streamMenuHint"
    close = f"{PACKAGE}:id/action_sheet_close"
    for rotation, suffix in ((0, ""), (1, "-landscape")):
        set_rotation(rotation)
        time.sleep(1)
        hint = find(hint_id)
        assert (hint.get("text") or "").startswith("To open the stream menu, "), hint.attrib
        width, height = screenshot("stream-hint" + suffix)
        root = ET.fromstring(LOGS.joinpath("stream-hint" + suffix + ".xml").read_bytes())
        hint = next(n for n in root.iter("node") if n.get("resource-id") == hint_id)
        overlay = next(n for n in root.iter("node") if n.get("resource-id") == f"{PACKAGE}:id/performanceOverlay")
        assert not overlaps(bounds(hint), bounds(overlay)), "The hint covers the performance overlay"
        assert_clear_of_cutout(bounds(hint), width, height, "The hint")
        assert_compact_overlay("stream-compact-controls" + suffix, True)

    for rotation, suffix in ((1, "-landscape"), (3, "-landscape-reverse"), (0, "")):
        set_rotation(rotation)
        time.sleep(1)
        # Back opens the menu, and the hint has done its job.
        adb("shell", "input", "keyevent", "4")
        find(close)
        # Let the sheet finish sliding in before it is measured and captured.
        time.sleep(0.8)
        assert not any(n.get("resource-id") == hint_id for n in tree().iter("node")), "The hint stayed over the menu"
        for label in ("Stream", "Keyboard", "Overlay", "Input", "Touch mode", "Display", "Overlay and diagnostics"):
            find(label, scroll=True)
        # Capture the menu from its top: the quick row and the first section. Swipe only while the
        # quick row is cut off, so a sheet already at its top is never dragged closed.
        for _ in range(10):
            root = tree()
            area = next((n for n in root.iter("node") if n.get("scrollable") == "true"), None)
            keyboard = next((n for n in root.iter("node") if n.get("text") == "Keyboard"), None)
            if area is None or (keyboard is not None and bounds(keyboard)[1] > bounds(area)[1]):
                break
            x1, y1, x2, y2 = bounds(area)
            adb("shell", "input", "swipe", str((x1 + x2) // 2), str(y1 + (y2 - y1) // 4),
                str((x1 + x2) // 2), str(y1 + (y2 - y1) * 3 // 4), "500")
        find("Keyboard")
        width, height = screenshot("stream-menu" + suffix)
        root = ET.fromstring(LOGS.joinpath("stream-menu" + suffix + ".xml").read_bytes())
        sheet = next(n for n in root.iter("node") if n.get("resource-id") == f"{PACKAGE}:id/design_bottom_sheet")
        assert_clear_of_cutout(bounds(sheet), width, height, "The stream menu")
        for label in ("Quality", "Host", "Server commands", "Open host web UI", "Reconnect stream", "Disconnect", "Quit game"):
            find(label, scroll=True)
        scroll_to_end()
        assert_touch_target("Quit game")
        screenshot("stream-menu-bottom" + suffix)
        # The overlay page opens in the same sheet; Back returns to the menu, and Back again closes it.
        tap("Overlay and diagnostics", scroll="up")
        for label in ("Compact", "Statistics", "Debug", "Copy stats", "Why did my stream stutter?"):
            find(label, scroll=True)
        find("Compact", scroll="up")
        find(f"{PACKAGE}:id/action_sheet_back")
        screenshot("stream-menu-overlay-page" + suffix)
        adb("shell", "input", "keyevent", "4")
        find("Keyboard", scroll="up")
        find("Input", scroll=True)
        adb("shell", "input", "keyevent", "4")
        for _ in range(8):
            if not any(n.get("resource-id") == close for n in tree().iter("node")):
                break
            time.sleep(0.5)
        else:
            raise AssertionError("Back on the first page did not close the stream menu")

    # D-pad: the focus starts in the menu and moves down through its rows.
    adb("shell", "input", "keyevent", "4")
    find(close)
    seen = set()
    for _ in range(6):
        adb("shell", "input", "keyevent", "KEYCODE_DPAD_DOWN")
        node = next((n for n in tree().iter("node") if n.get("focused") == "true"), None)
        assert node is not None and node.get("package") == PACKAGE, "D-pad focus left the stream menu"
        seen.add(bounds(node))
    assert len(seen) >= 4, f"D-pad down did not move through the menu rows: {seen}"
    adb("shell", "input", "keyevent", "4")

    # On a wide phone the centred hint reaches the top-start overlay that stays there without controls.
    adb("shell", "wm", "size", "1080x2400")
    adb("shell", "wm", "density", "420")
    for align in ("center", "top"):
        adb("shell", "am", "force-stop", PACKAGE)
        adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.demo.DemoLauncherActivity",
            "--es", "state", "stream", "--es", "codec", "h264", "--ez", "hint", "true", "--es", "align", align)
        find(hint_id)
        time.sleep(1)
        width, height = screenshot(f"stream-hint-1080-{align}")
        root = ET.fromstring(LOGS.joinpath(f"stream-hint-1080-{align}.xml").read_bytes())
        hint = next(n for n in root.iter("node") if n.get("resource-id") == hint_id)
        overlay = next(n for n in root.iter("node") if n.get("resource-id") == f"{PACKAGE}:id/performanceOverlay")
        assert not overlaps(bounds(hint), bounds(overlay)), f"At 1080x2400 ({align}) the hint covers the performance overlay"
        assert_clear_of_cutout(bounds(hint), width, height, "The hint")
    adb("shell", "wm", "size", "786x1704")
    adb("shell", "wm", "density", "320")
    adb("shell", "am", "force-stop", PACKAGE)
    set_rotation(0)
    print("Stream menu sections, page Back, D-pad focus, hint and compact overlay placement checked", flush=True)


def assert_no_app_crashes():
    crashes = adb("logcat", "-b", "crash", "-d")
    LOGS.joinpath("logcat.txt").write_text(adb("logcat", "-d"), encoding="utf-8")
    LOGS.joinpath("crashes.txt").write_text(crashes, encoding="utf-8")
    # uiautomator itself can time out under host load and logs a FATAL EXCEPTION of its own;
    # only crashes of this app count.
    app_crashes = [block for block in re.split(r"(?=FATAL EXCEPTION)|(?=Fatal signal)", crashes)
                   if ("FATAL EXCEPTION" in block or "Fatal signal" in block) and PACKAGE in block]
    if app_crashes:
        raise AssertionError("Emulator crash buffer is not clean")


class HostFixture(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.split("?", 1)[0] == "/pair":
            # Hold the real pairing prompt open without completing a synthetic pairing.
            self.server.pairing_wait.wait(60)
            return
        if self.path.split("?", 1)[0] != "/serverinfo":
            self.send_error(404)
            return
        body = f'''<?xml version="1.0"?><root status_code="200">
          <hostname>{HOST_NAME}</hostname>
          <uniqueid>00000000-0000-4000-8000-000000000006</uniqueid>
          <appversion>7.1.431.-1</appversion><GfeVersion>3.23.0.74</GfeVersion>
          <PairStatus>0</PairStatus><currentgame>0</currentgame>
          <state>SUNSHINE_SERVER_FREE</state><HttpsPort>47984</HttpsPort>
          <ServerCodecModeSupport>197377</ServerCodecModeSupport>
          <RustHostVersion>2.0.0</RustHostVersion>
        </root>'''.encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/xml")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def main():
    global DEBUGGABLE, SHOTS, LOGS, PACKAGE, SERIAL
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apk", type=Path,
                        default=ROOT / "app/build/outputs/apk/nonRoot/debug/app-nonRoot-debug.apk",
                        help="APK to install; defaults to the nonRoot debug build")
    parser.add_argument("--avd", default=os.environ.get("NIGHTJAR_AVD", "sunset"),
                        help="Existing rootable AVD name (default: NIGHTJAR_AVD or sunset)")
    parser.add_argument("--port", type=int, default=5554,
                        help="Even emulator console port from 5554 to 5682")
    parser.add_argument("--output-dir", type=Path,
                        help="Write screenshots and logs under this directory instead of the repository")
    scope = parser.add_mutually_exclusive_group()
    scope.add_argument("--ui-only", action="store_true",
                        help="Fresh install and PC fixture setup, then only responsive/navigation/overlay checks")
    scope.add_argument("--debug-only", action="store_true",
                       help="Fresh install, then only debug library and overlay fixture checks")
    parser.add_argument("--ui-case", action="append", choices=tuple(UI_CASES),
                        help="Run a selected UI viewport (repeatable); defaults to every viewport")
    args = parser.parse_args()
    if args.debug_only and args.ui_case:
        parser.error("--debug-only cannot be combined with --ui-case")
    if args.port < 5554 or args.port > 5682 or args.port % 2:
        parser.error("--port must be an even number from 5554 to 5682")
    SERIAL = f"emulator-{args.port}"
    if args.output_dir:
        SHOTS = args.output_dir.resolve() / "screenshots"
        LOGS = args.output_dir.resolve() / "logs"
    apk = args.apk.resolve()
    if not apk.is_file():
        raise FileNotFoundError(f"Build the selected APK before running the smoke test: {apk}")
    aapt = max((SDK / 'build-tools').glob('*/' + ('aapt.exe' if os.name == 'nt' else 'aapt')))
    badging = subprocess.check_output([str(aapt), 'dump', 'badging', str(apk)], text=True)
    PACKAGE = re.search(r"package: name='([^']+)'", badging).group(1)
    devices = subprocess.check_output([str(ADB), "devices"], text=True, timeout=30)
    if re.search(rf"^{re.escape(SERIAL)}\s", devices, re.MULTILINE):
        raise RuntimeError(f"{SERIAL} is already in use; choose another --port")
    SHOTS.mkdir(parents=True, exist_ok=True)
    LOGS.mkdir(parents=True, exist_ok=True)
    fixture = http.server.ThreadingHTTPServer(("127.0.0.1", 0), HostFixture)
    fixture.pairing_wait = threading.Event()
    threading.Thread(target=fixture.serve_forever, daemon=True).start()
    with LOGS.joinpath("emulator.log").open("w") as log:
        process = subprocess.Popen([str(EMULATOR), "-avd", args.avd, "-read-only",
                                    "-no-snapshot", "-no-window", "-no-audio", "-no-boot-anim",
                                    "-gpu", os.environ.get("NIGHTJAR_EMULATOR_GPU", "swiftshader"), "-cores", "2", "-memory", "4096",
                                    "-port", str(args.port)], stdout=log, stderr=subprocess.STDOUT,
                                   creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        try:
            deadline = time.monotonic() + int(os.environ.get("NIGHTJAR_BOOT_TIMEOUT", "180"))
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f"Emulator exited; see {LOGS / 'emulator.log'}")
                try:
                    if (adb("shell", "getprop", "sys.boot_completed", timeout=5).strip() == "1" and
                            adb("shell", "pm", "path", "android", timeout=5).startswith("package:")):
                        break
                except (subprocess.SubprocessError, OSError):
                    pass
                time.sleep(2)
            else:
                raise TimeoutError("Emulator boot or package manager did not become ready")
            adb("shell", "input", "keyevent", "82", "3")
            previous = None
            while time.monotonic() < deadline:
                try:
                    launcher = adb("shell", "cmd", "package", "resolve-activity", "--brief",
                                   "-a", "android.intent.action.MAIN", "-c", "android.intent.category.HOME",
                                   timeout=5).strip().split("\n")[-1]
                    ui = tree()
                    if wait_for_system_process(ui):
                        previous = None
                        continue
                    current = ET.tostring(ui)
                    # HOME can still resolve to a temporary boot screen after boot_completed.
                    if ("/" in launcher and not launcher.startswith(("com.android.settings/", "com.google.android.googlesdksetup/")) and
                            any(n.get("package") == launcher.split("/")[0] for n in ui.iter("node"))):
                        if current == previous:
                            break
                        previous = current
                    else:
                        previous = None
                except (subprocess.SubprocessError, ET.ParseError):
                    previous = None
                time.sleep(2)
            else:
                raise TimeoutError("Emulator launcher did not settle")
            print(f"Emulator services and launcher ready; installing {apk}", flush=True)
            adb("install", "-r", str(apk), timeout=90)
            package_info = adb("shell", "dumpsys", "package", PACKAGE)
            DEBUGGABLE = "DEBUGGABLE" in package_info
            if not DEBUGGABLE:
                # A release APK stays non-debuggable; the disposable emulator provides
                # root access for the same persistence assertions used on debug builds.
                adb("root")
                adb("wait-for-device")
                if adb("shell", "id", "-u").strip() != "0":
                    raise RuntimeError("Release smoke needs a rootable Google APIs AVD, not a Google Play image")
                SHOTS = SHOTS / "release"
                LOGS = LOGS / "release"
                SHOTS.mkdir(parents=True, exist_ok=True)
                LOGS.mkdir(parents=True, exist_ok=True)
            version = re.search(r"versionName=(\S+)", package_info).group(1)
            adb("shell", "pm", "clear", PACKAGE)
            adb("shell", "cmd", "locale", "set-app-locales", PACKAGE, "--user", "0", "--locales", "en")
            adb("shell", "wm", "size", "786x1704")
            adb("shell", "wm", "density", "320")
            adb("shell", "settings", "put", "system", "font_scale", "1.0")
            adb("shell", "settings", "put", "secure", "show_ime_with_hard_keyboard", "1")
            adb("shell", "svc", "wifi", "disable")
            adb("shell", "svc", "data", "disable")
            adb("reverse", "tcp:47989", f"tcp:{fixture.server_port}")
            adb("logcat", "-c")
            adb("shell", "input", "keyevent", "82")
            adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.PcView")
            set_rotation(0)
            find(f"{PACKAGE}:id/discovery_add")
            assert_cold_start_insets()
            assert_dpad_reaches_home_actions()
            if args.debug_only:
                if not DEBUGGABLE:
                    raise ValueError("--debug-only requires a debug APK with library and overlay fixtures")
                capture_debug_checks()
                assert_no_app_crashes()
                print(f"PASS: debug home row states and menus, D-pad to a row's menu, library covers with the running game first, library search, clear action, query persistence across actual rotation, app action sheets, statistics/debug/compact overlay contents and bounds in portrait and landscape, compact overlay clear of on-screen controls and the cutout, first-stream hint, stream menu sections, page Back and D-pad focus; screenshots: {SHOTS}", flush=True)
                return
            if args.ui_only:
                tap(f"{PACKAGE}:id/discovery_add")
                enter_text(f"{PACKAGE}:id/hostTextView", "127.0.0.1:47989")
                tap(f"{PACKAGE}:id/addPcButton")
                find(HOST_NAME)
                capture_ui_sweep(args.ui_case)
                assert_no_app_crashes()
                print(f"PASS: UI-only fresh install and PC setup; sheet scrolling/close, settings search/back restoration, pairing and manual discovery for {', '.join(args.ui_case or UI_CASES)}; library search/rotation and live overlay rendering (debug only); screenshots: {SHOTS}", flush=True)
                return
            screenshot("01-home-empty")
            tap(f"{PACKAGE}:id/discovery_help")
            find("Connect to your PC")
            screenshot("01-launch")
            native = native_resolution()
            latency_switches = ("checkbox_codec_low_latency", "checkbox_vendor_low_latency",
                                "checkbox_phone_performance_hints", "checkbox_drop_late_frames",
                                "checkbox_unbatched_input", "checkbox_network_priority")
            prefs(f"{PACKAGE}_preferences", lambda root: pref_value(root, "prefs_version") == "4" and
                  pref_value(root, "list_resolution") == native and pref_value(root, "frame_pacing") == "latency" and
                  all(pref_value(root, key) == "true" for key in latency_switches) and
                  pref_value(root, "checkbox_gpu_max_clocks") in (None, "false"),
                  f"Fresh install is not at native {native} with every latency setting on and GPU clocks left alone")
            tap("android:id/button1")
            # Settings, App, Help opens the bundled help, which needs no network.
            open_help()
            find("Fixing common problems", scroll=True)
            screenshot("23-help")
            close_help()
            tap(f"{PACKAGE}:id/discovery_add")
            enter_text(f"{PACKAGE}:id/hostTextView", "https://example.com/path")
            adb("shell", "input", "keyevent", "66")
            find("Enter an IP address or PC name, optionally followed by a port (for example, 192.168.1.10:47989).")
            screenshot("02-manual-host-invalid")
            enter_text(f"{PACKAGE}:id/hostTextView", "127.0.0.1:47989")
            screenshot("02-manual-host")
            tap(f"{PACKAGE}:id/addPcButton")
            find(HOST_NAME)
            screenshot("03-host-added")
            host_menu()
            find("Pair with one-time PIN")
            assert not any(n.get("text") == "Add games to ES-DE" for n in tree().iter("node")), \
                "Unpaired PC offers frontend export"
            screenshot("19-unpaired-host-menu")
            tap("Pair with one-time PIN")
            find("One-time PIN")
            screenshot("16-otp-pairing")
            tap("android:id/button2")
            open_host_profile()
            # The PC's details moved from its menu to its settings screen.
            tap("View details", scroll=True)
            find("Copy debug info")
            find("Rubylight 2.0.0")
            screenshot("15-host-details")
            tap("android:id/button1")
            find("Stream resolution", scroll="up")
            profile_number("Width", "0")
            find("Enter a whole number from 64 to 16384")
            profile_number("Width", "1920")
            profile_number("Height", "1080")
            profile_number("Frame rate (FPS)", "59.94")
            profile_number("Bitrate (Mbps)", "45.5")
            screenshot("09-host-profile")
            tap(f"{PACKAGE}:id/host_profile_save")
            prefs("HostStreamProfiles", lambda root: any(
                n.get("name") == "00000000-0000-4000-8000-000000000006" and
                (n.text or "") == "v2;res=1920x1080;refresh=5994;bitrate=45500" for n in root), "Host profile was not saved")
            open_host_profile()
            find("59.94 FPS", scroll=True)
            find("Prefer YUV 4:4:4", scroll=True)
            screenshot("10-host-codec-profile")
            tap(PROFILE_RESET, scroll=True)
            tap("android:id/button1")
            tap(f"{PACKAGE}:id/host_profile_save")
            prefs("HostStreamProfiles", lambda root: not list(root), "Host profile reset did not clear overrides")
            tap(f"{PACKAGE}:id/shell_settings")
            find("Presets")
            screenshot("04-settings")
            tap("Battery saver")
            tap("android:id/button1")
            prefs(f"{PACKAGE}_preferences", lambda root: any(
                n.get("name") == "list_fps" and n.text == "30" for n in root), "Battery saver was not applied")
            tap("Native (recommended)")
            tap("android:id/button1")
            prefs(f"{PACKAGE}_preferences", lambda root: pref_value(root, "list_resolution") == native and
                  pref_value(root, "list_fps") != "30" and pref_value(root, "checkbox_reduce_refresh_rate") == "false",
                  "Native was not applied")
            tap("Video")
            find("Stream resolution")
            screenshot("05-video-settings")
            tap("Video bitrate")
            tap("80")
            tap("Increase")
            find("81 Mbps")
            screenshot("06-bitrate")
            tap("android:id/button1")
            prefs(f"{PACKAGE}_preferences", lambda root: any(
                n.get("name") == "seekbar_bitrate_kbps" and n.get("value") == "81000" for n in root),
                "Bitrate was not stored in kbps")
            tap("Video codec", scroll=True)
            tap("PyroWave (host support required)")
            tap("Video codec")
            tap("android:id/button3")
            help_text = " ".join(n.get("text", "") for n in tree().iter("node"))
            assert any(s in help_text for s in ("Not supported: ", "Ready on this phone", "GPU does not support PyroWave")), help_text
            screenshot("20-pyrowave-readiness")
            tap("android:id/button1")
            tap("Automatic (recommended)")
            find("Stream resolution")
            navigate_up()
            find("Presets")
            find_text(r"^(\d+×\d+|\d+p) · \d+ FPS · 81 Mbps · Automatic")
            find("Custom")
            tap("Display & image")
            vrr = "Adaptive refresh (ARR / VRR)"
            tap(vrr, scroll=True)
            prefs(f"{PACKAGE}_preferences", lambda root: any(
                n.get("name") == "checkbox_vrr" and n.get("value") == "true" for n in root),
                "VRR preference was not enabled")
            screenshot("21-host-display-vrr")
            adb("shell", "am", "force-stop", PACKAGE)
            adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.PcView")
            find(HOST_NAME)
            tap(f"{PACKAGE}:id/shell_settings")
            tap("Display & image")
            find(vrr, scroll=True)
            # The row is exposed either as a clickable container or, since the accessibility pass, as a checkable switch.
            row = next(n for n in tree().iter("node") if "true" in (n.get("clickable"), n.get("checkable")) and
                       any(child.get("text") == vrr for child in n.iter("node")))
            assert any(n.get("checked") == "true" for n in row.iter("node")), \
                "VRR switch was not restored after restarting the app"
            prefs(f"{PACKAGE}_preferences", lambda root: any(
                n.get("name") == "checkbox_vrr" and n.get("value") == "true" for n in root),
                "VRR preference did not persist after restarting the app")
            tap(vrr)
            prefs(f"{PACKAGE}_preferences", lambda root: any(
                n.get("name") == "checkbox_vrr" and n.get("value") == "false" for n in root),
                "VRR preference could not be disabled")
            navigate_up()
            tap("Advanced", scroll=True)
            decoding = "Low-latency decoding"
            parts = ("checkbox_codec_low_latency", "checkbox_vendor_low_latency", "checkbox_phone_performance_hints")
            tap(decoding, scroll=True)
            prefs(f"{PACKAGE}_preferences", lambda root: all(pref_value(root, key) == "false" for key in parts),
                  "Low-latency decoding was not on by default or did not turn its parts off")
            screenshot("13-advanced-settings")
            tap(decoding)
            prefs(f"{PACKAGE}_preferences", lambda root: all(pref_value(root, key) == "true" for key in parts),
                  "Low-latency decoding did not turn its parts back on")
            hints = "Phone performance hints"
            tap(hints, scroll=True)
            prefs(f"{PACKAGE}_preferences", lambda root: any(
                n.get("name") == "checkbox_phone_performance_hints" and n.get("value") == "false" for n in root),
                "Phone performance hints were not on by default or could not be disabled")
            screenshot("22-advanced-diagnostics")
            tap(hints)
            prefs(f"{PACKAGE}_preferences", lambda root: any(
                n.get("name") == "checkbox_phone_performance_hints" and n.get("value") == "true" for n in root),
                "Phone performance hints could not be enabled")
            # Returning to the overview and reopening Advanced keeps the saved value.
            navigate_up()
            find("Presets", scroll="up")
            tap("Advanced", scroll=True)
            find(hints, scroll=True)
            prefs(f"{PACKAGE}_preferences", lambda root: pref_value(root, "checkbox_phone_performance_hints") == "true",
                  "Reopening Advanced changed a value")
            find("Latency benchmark", scroll=True)
            navigate_up()
            tap("Streaming", scroll="up")
            overlay = "Show performance overlay"
            tap(overlay, scroll=True)
            screenshot("13-streaming-settings")
            prefs(f"{PACKAGE}_preferences", lambda root: any(
                n.get("name") == "checkbox_enable_perf_overlay" and n.get("value") == "true" for n in root),
                "Overlay preference was not enabled")
            # All overlay modes are available without a separate settings gate.
            tap("Performance overlay mode", scroll=True)
            find("Compact")
            find("Statistics")
            find("Debug")
            adb("shell", "input", "keyevent", "4")
            find(overlay)
            adb("shell", "input", "keyevent", "3")
            adb("shell", "am", "start", "-W", "-n", f"{PACKAGE}/com.limelight.PcView")
            find(overlay)
            navigate_up()
            tap("App")
            tap("About", scroll=True)
            screenshot("14-settings-about")
            summary = next((n.get("text", "") for n in tree().iter("node")
                            if "Moonlight" in n.get("text", "")), "")
            assert "GPL-3.0" in summary, "About entry lost the Moonlight attribution"
            assert f"Version {version}" in summary, "About entry does not show the installed app version"
            tap("android:id/button1")
            navigate_up()
            tap("Controls")
            tap("Controller buttons", scroll=True)
            find("Connect a controller, then tap “Map a button”.")
            tap("Map a button")
            find("Press the controller button to map. Back cancels.")
            adb("shell", "input", "gamepad", "keyevent", "KEYCODE_BUTTON_Y")
            find("Y sends…")
            screenshot("11-controller-choose")
            tap("A")
            find("Sends A")
            screenshot("12-controller-buttons")
            prefs("ControllerButtonMaps", lambda root: any(n.text == "100:96" for n in root),
                  "Controller mapping was not saved")
            adb("shell", "input", "keyevent", "4")
            find("Controller buttons")
            navigate_up()
            tap("App")
            tap("Reset all settings", scroll=True)
            tap("android:id/button1")
            navigate_up()
            find_text(r"^(\d+×\d+|\d+p) · \d+ FPS · \d+ Mbps · Automatic")
            prefs(f"{PACKAGE}_preferences", lambda root: pref_value(root, "list_resolution") == native and
                  pref_value(root, "checkbox_network_priority") == "true",
                  "Reset did not return to native resolution with latency settings on")
            prefs(f"{PACKAGE}_preferences", lambda root: not any(
                n.get("name") == "checkbox_enable_perf_overlay" and n.get("value") == "true" for n in root),
                "Reset kept the overlay setting")
            prefs("ControllerButtonMaps", lambda root: any(n.text == "100:96" for n in root),
                  "Reset removed the controller mapping")
            adb("shell", "input", "keyevent", "4")
            find(HOST_NAME)
            for name, contents, message, shot in (
                    ("malformed", "This is not a game entry.\n",
                     "Game file unavailable.", "17-frontend-malformed"),
                    ("unknown-host", "# Nightjar game entry\n"
                     "[host_uuid] 00000000-0000-4000-8000-000000000007\n"
                     "[host_name] Unknown smoke PC\n[app_uuid] 00000000-0000-4000-8000-000000000008\n"
                     "[app_name] Smoke game\n[app_id] 123\n", "PC not found", "18-frontend-unknown-host")):
                uri = frontend_entry(name, contents)
                adb("shell", "input", "keyevent", "3")
                find(f"{launcher.split('/')[0]}:id/workspace")
                # A URI grant from root (release runs use adb root) is refused by MediaStore; grant as the shell user.
                adb("shell", *(() if DEBUGGABLE else ("su", "2000")), "am", "start", "-W", "-n",
                    f"{PACKAGE}/com.limelight.ShortcutTrampoline",
                    "-a", "android.intent.action.VIEW", "-d", uri, "--grant-read-uri-permission")
                find(message)
                if name == "malformed":
                    assert "Unreadable frontend entry: Not a game entry file" in adb("logcat", "-d"), \
                        "Malformed entry was not read by the app"
                screenshot(shot)
                tap("android:id/button1")
                find(HOST_NAME)
            capture_ui_sweep(args.ui_case)
            assert_no_app_crashes()
            overlay_result = "overlay rendering" if DEBUGGABLE else "overlay settings (live rendering needs a paired host)"
            print(f"PASS: native {native} and latency defaults on a fresh install, pairing guide and PIN prompt, manual discovery and inline validation, OTP and details dialogs, PC settings and host profile validation/save/reset, PC menu close, settings search/back restoration for {', '.join(args.ui_case or UI_CASES)}, VRR toggle/persistence, PyroWave readiness, bitrate, controller mapping, reset, frontend entries, unpaired export menu, overlay mode choices beside the overlay switch, {overlay_result}, demo home row states and menus, cover library and rotation; screenshots: {SHOTS}", flush=True)
        finally:
            try:
                LOGS.joinpath("logcat.txt").write_text(adb("logcat", "-d"), encoding="utf-8")
                LOGS.joinpath("crashes.txt").write_text(adb("logcat", "-b", "crash", "-d"), encoding="utf-8")
            except (subprocess.SubprocessError, OSError):
                pass
            try:
                if "OK" not in adb("emu", "kill", timeout=10):
                    adb("shell", "reboot", "-p", timeout=10)
            except (subprocess.SubprocessError, OSError):
                process.terminate()
            try:
                process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)
            fixture.pairing_wait.set()
            fixture.shutdown()
            fixture.server_close()


if __name__ == "__main__":
    main()
