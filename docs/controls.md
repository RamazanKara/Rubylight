# Controls

[Documentation](index.md) · [All defaults](settings.md)

If your host has per-device permissions, it must grant the relevant input permission to this device. If video works but controls do not, check [host permissions](pairing.md#per-device-permissions) first.

## The stream menu

Open it with Android **Back** (swipe back from the screen edge with gesture navigation), **Ctrl+Alt+Shift+M** on a keyboard, or **Start+Back+LB+RB** on a controller, released together; this works for USB-driver controllers too. The first stream shows a one-line hint naming the ways this device has; tap it to close it.

The menu is one sheet:

| Part | What it holds |
| --- | --- |
| Top row | **Keyboard** (Android's keyboard), **Overlay** (show or hide the performance overlay) and **Microphone** (mute or unmute, when the PC offers a microphone). Overlay and Microphone switch in place. |
| Input | Touch mode, Show mouse cursor (with a mouse attached), Show on-screen controls, Edit controller layout, Controls for this game, Extra keyboard keys. |
| Display | Stretch video to fill the screen (local video without upscaling), Enable pinch zoom and pan, Reset zoom, **Overlay and diagnostics**, Picture-in-picture, Video output (with an external display). |
| Quality | Set bitrate, Automatic stream quality and Automatic bitrate, where the host supports them. |
| Host | Server commands, Send clipboard to host, Copy host clipboard, Open host web UI, Permissions and frame limits, as the host and its permissions allow. |
| Bottom | Reconnect stream, Disconnect, and **Quit game** in red, which asks first. Remote Monitor and Input-only end their session instead. |

**Overlay and diagnostics** opens a page in the same sheet: Compact, Statistics or Debug, Copy stats, Why did my stream stutter? and Measure input latency. Back, Escape or a controller's B returns from the page to the menu; Back again closes the menu, and the close button closes it from anywhere. The D-pad and a controller move through every row; on a USB-driver controller, its D-pad, A and B do this while the menu is open. Items a host or permission does not allow are left out rather than shown disabled.

## Gamepads

Connect a Bluetooth controller in Android's Bluetooth settings or plug a USB controller into the device. **Settings → Controls → Multiple controllers** is on by default and gives each controller its own player; the protocol provides up to 16 controller slots.

Use **Controller buttons** to press a digital button and choose what it sends, including **Nothing**. Mappings are stored per controller model and apply on the next stream, including the USB drivers. **Reset this controller** restores its default mapping.

Open **Sticks, triggers and D-pad** after selecting a controller. Each output stick axis can read either stick's horizontal or vertical axis, optionally inverted, or be disabled. Triggers can read either physical trigger or be disabled; their full analog range is preserved. Each D-pad direction can send another direction, a gamepad button, or nothing. These mappings apply to normalized Android and direct-USB reports, after deadzones, and also apply from the next stream. D-pad directions mapped to the same button stay held until every contributing source is released. Reset restores both digital and axis mappings. Axis mappings do not convert a stick into a trigger or emulate keyboard keys.

**Flip face buttons** swaps A/B and X/Y. Raise the stick deadzone if a stationary stick drifts. **Vibrate device for rumble** gives you rumble on the phone or tablet when the controller has no motors.

### DualSense and USB

Bluetooth DualSense uses Android's controller path. Controller motion, lights, battery and vibration extensions depend on Android and controller support; motion settings are available on Android 12+, with motion defaulting off on API 31 because of an OS bug.

For direct DualSense or DualSense Edge USB support:

1. Enable **Settings → Controls → USB DualSense driver** and accept its explanation.
2. Connect the pad by USB and start a stream.
3. Allow Android's USB access request. Nightjar can then send host-provided adaptive trigger effects, rumble and lightbar changes and read the pad's input and Edge paddles.

The toggle is off by default. Denying USB access keeps the ordinary Android input path; unplug and reconnect to retry. The host must support the requested feedback.

Enable **USB DualSense audio and haptics**, then reconnect the controller and stream, for native waveform vibration from hosts that support it (e.g. Rubylight or Vibepollo). Hosts that also support controller audio (currently Rubylight) carry its speaker, headset and microphone audio. Bluetooth waveform output is unavailable through Android’s standard controller API. See [audio and haptics setup](dualsense-haptics.md).

For gyro aiming and radial menus, use the game’s **Steam Input** controller layout on the host and enable controller motion in Nightjar. The client forwards motion and touchpad input; Steam supplies the aiming behavior and menu overlay. Games using native DualSense effects may need Steam Input disabled for that game.

The separate **USB controller driver** setting supplies Xbox USB support. **Always use the USB driver** prefers that path even if Android already recognizes the controller.

### Controller mouse mode

With **Controller mouse mode** enabled, hold **Start** to toggle it. By default the left stick moves the pointer, the right stick scrolls, **A** left-clicks and **B** right-clicks. **Scroll with an analog stick** can swap the sticks or make both move the pointer.

## Touch modes

Choose **Settings → Controls → Touch mode**, or **stream menu → Input → Touch mode**.

| Mode | Gestures and behavior |
| --- | --- |
| Trackpad (default) | Drag to move the pointer relatively. Tap with one finger for left-click, two for right-click. Hold then drag to drag with a mouse button; move two fingers to scroll. |
| Direct mouse | Tap a location to click there, drag to move while holding the left button, and hold still for right-click. A second finger scrolls. |
| Multi-touch | Send fingers directly to the host touch interface. If native touch is unavailable, Nightjar uses Direct mouse. Use the stream menu for the keyboard. |

**Trackpad speed** changes relative touch movement from 25–400%, with 100% as the default. Native pen input is forwarded when supported and permitted by the host.

Direct mouse, native touch and pen positions follow the streamed image, including when cutouts, fold panes or window padding shift it. Touch permission and a working host touch interface are still required for Multi-touch.

## Mouse and keyboard

USB and Bluetooth mice and keyboards use Android's input support. Android 8+ provides native pointer capture. **Mouse speed** scales relative movement from 25–400% (default 100%).

Leave **Desktop mouse mode** off for games that need relative mouse look; enable it for desktop pointer behavior. Enable **Mouse back/forward buttons** only when needed; some devices report right-click through the same Android key path.

Open the stream menu and select **Keyboard** in its top row for Android's on-screen keyboard. It stays hidden when a stream starts, including on tablets. Hardware key events and text input are sent to the host; Android may reserve system keys. Check the host keyboard layout if punctuation or language-specific keys differ.

Select **stream menu → Input → Extra keyboard keys** for a toolbar with Esc, Tab, Enter, Backspace, Delete, Home/End, Page Up/Down, arrows and F1–F12. Scroll each row horizontally to reach more keys. Ctrl, Alt, Shift and Win apply to the next key pressed in this toolbar, then reset. **Hide keys** closes the toolbar; **Keyboard** opens Android's text keyboard.

The toolbar also provides **Task Manager** (Ctrl+Shift+Esc), **Switch window** (Alt+Tab) and **Close window** (Alt+F4). To stop an unresponsive app, open Task Manager and use its **End task** action on the host. These controls require the host's keyboard permission.

### Custom keyboard shortcuts

Open **Settings → Controls → Keyboard shortcuts** and choose **Add shortcut**. Enter a button name and a chord such as `CTRL + SHIFT + ESC`, `WIN + D` or `ALT + F4`, then **Save**. Select a saved button to change its name or keys, or choose **Delete**. Unsaved editor text survives screen rotation.

On the next stream, open **stream menu → Input → Extra keyboard keys**. Your buttons appear in an additional row that scrolls horizontally. Tapping one presses its keys in the listed order and releases them in reverse order; put modifiers first. Toolbar modifier toggles reset when a shortcut is sent. Shortcuts require the host's keyboard permission and use the same input path as the built-in toolbar buttons.

Key names are case-insensitive. Use letters `A`–`Z`, digits `0`–`9`, `F1`–`F24`, modifiers `CTRL`, `SHIFT`, `ALT`, `WIN`, and names such as `ESC`, `TAB`, `ENTER`, `BACKSPACE`, `DELETE`, `HOME`, `END`, `PAGE_UP`, `PAGE_DOWN`, `LEFT`, `RIGHT`, `UP`, `DOWN` and `SPACE`. `CTRL_RIGHT`, `SHIFT_RIGHT`, `ALT_RIGHT` and `META_RIGHT` select right-hand modifiers; `NUMPAD_0`–`NUMPAD_9` select number-pad keys. Other Android key names are accepted only when Nightjar can forward them to the host.

Choose **Export shortcuts** to save a UTF-8 JSON file through Android's file picker. **Import shortcuts** validates a selected file and asks before replacing the entire saved list; canceling keeps your current buttons. Files can contain at most 32 shortcuts and must be at most 64 KiB. Each shortcut has a unique ID containing 1–64 letters, digits, underscores or hyphens, a nonblank name of at most 80 characters without control characters, and 1–8 supported keys without duplicates. The editor accepts at most 256 characters for a chord.

```json
{
  "version": 1,
  "data": [
    {"id": "copy", "name": "Copy", "keys": ["CTRL", "C"]},
    {"id": "desktop", "name": "Desktop", "keys": ["WIN", "D"]}
  ]
}
```

The importer also accepts this format without `version`, common Windows names such as `VK_LCONTROL` and `VK_C`, and supported Windows virtual-key values such as `0x41` for A. Exported files may use `ANDROID_KEYCODE_<number>` when the Android version lacks a symbolic label; this explicit form keeps Android codes distinct from digit keys. Empty `data` is valid and clears the list after confirmation.

## PC display mode and administration

Open your PC's menu → **PC settings… → PC display mode**. The choice is saved per PC and applies from the next stream. The entry appears only for PCs whose host supports display modes (Rubylight 2.2.1 or later); an older Rubylight shows it disabled with the reason. Other PCs with a virtual display get a plain **Host virtual display** switch in their settings.

| Choice | What happens on the PC |
| --- | --- |
| The PC's own setting | Follow Host virtual display and the host's display settings. |
| Exclusive | Use a virtual display and switch the PC's other monitors off during the stream. |
| Extended | Keep the other monitors on; choose whether the PC display or virtual display is primary. |
| Extended and separated | Keep the other monitors on, but place the streamed display far away in the desktop layout. Choose which display is primary. Both displays share the same Windows session. |
| Physical display | Stream the PC's monitor without adding a virtual display. |

The host restores the display layout when the stream ends. These choices apply to normal streaming; Remote Monitor and Input-only retain their host-controlled behavior. A saved choice is sent only to a host with display modes.

On every host except NVIDIA GameStream, **Open host web UI** in an online PC's settings opens the host's web UI in your browser, at its default port six above the PC's HTTPS port. Manage applications, paired devices and host settings there. The web UI uses its own login; pairing does not sign you in. Remote access needs a reachable web UI port, which is separate from the streaming ports. The dialog's address can be selected and copied if no browser is available.

## On-screen controls

Turn on **Settings → Controls → Show on-screen controls**, or use **stream menu → Input → Show on-screen controls**. The virtual gamepad supports opacity, press vibration, a Guide button and an L3/R3-only layout.

During a stream choose **stream menu → Input → Edit controller layout**. Select **Move controls** or **Resize controls**, drag the controls, then choose **Save layout and play**. **Reset controls layout** in Settings restores their sizes and positions.

### Per-game keyboard and mouse buttons

Choose **stream menu → Input → Controls for this game → Add keyboard or mouse button**. Give the button a label and a key or chord (`W`, `SPACE`, `CTRL + C`) or mouse binding (`MOUSE_1` left, `MOUSE_2` middle, `MOUSE_3` right, `MOUSE_4` back, `MOUSE_5` forward). Buttons normally hold their input while pressed. The **Tap to hold; tap again to release** option supports actions that would otherwise require a long press. TalkBack, Switch Access and keyboard activation can also activate a button; ordinary activation sends an 80 ms press.

Set button width and height as percentages of the available control surface. Every button retains at least a 48 dp touch target. Select **Move game buttons**, drag them to position, then **Save layout and play**. Touch input to the PC is suppressed while placing buttons. Select a button's name in this menu to edit its size, binding or label, or delete it. These controls can coexist with the virtual gamepad; the virtual gamepad's existing layout remains separate.

Layouts and their visibility are saved separately for each PC and game, using the app UUID when available and its numeric ID otherwise. Resume and Running Game tiles select the established game's saved controls when the stream connects. Shared modifier keys and mouse buttons remain held until all controls using them have released. Opening the menu, hiding the layout, entering picture-in-picture, leaving the activity, disconnecting, canceling a touch, or replacing a layout releases its held input. The host's ordinary keyboard/mouse permissions still apply; no host extension is needed.

**Export or share layout** offers selectable JSON, **Copy JSON**, and a JSON file through Android's share sheet. The share sheet may leave the stream according to Android's activity lifecycle; the PC game remains running. On another game/device, **Import layout** accepts pasted JSON and replaces that game's custom buttons only after pressing **Replace**. Layouts are limited to 32 buttons and 64 KiB. Unsupported versions, duplicate IDs, unsupported keys, nonfinite/out-of-bounds geometry and invalid mouse bindings are rejected without changing the current layout. Coordinates are normalized so layouts scale across devices; review placement on a different aspect ratio.

### Tablet zoom and external displays

Choose **stream menu → Display → Enable pinch zoom and pan**. Pinch to zoom from 1× to 4× and drag two fingers to pan within the desktop. One finger continues to use the selected touch mode; direct mouse, native touch, pen and absolute mouse positions use the inverse video transform. Trackpad movement stays relative. While enabled, the two-finger gesture controls the view instead of scrolling the PC. **Reset zoom** restores the full image. Android 5/6 require TextureView; Android 7+ support the SurfaceView path as well.

For a dock, USB-C/HDMI monitor or Android presentation-capable wireless display, choose **stream menu → Display → Video output** and select the display. The item appears while Android offers such a display, and while video is on one. Switching output safely disconnects/reconnects the client while preserving the PC application, and starts the decoder on a dedicated presentation surface. Game streams resume only the captured application: if it stops or another application replaces it during handoff, the reconnect fails without launching or quitting an application. This guard is independent of automatic network recovery. Remote Monitor keeps its dedicated view-only host operation. Video goes to the monitor; the device remains a touchpad with its keyboard toolbar, game buttons and stream menu. This works with any Moonlight-compatible host and does not ask the host to change its display layout. Android determines which attached outputs support presentations; mirroring-only outputs cannot be selected.

Select **This device** to return. Removing the external display triggers the same client reconnect back to the phone/tablet. A display that disappears before launch falls back to the device. Output selection is session-local and is preserved through reconnects, not saved as a global preference. PiP auto-entry is disabled while presenting externally. Locking/leaving the activity follows the normal stream lifecycle; no background input or unattended reconnect is introduced.

Android 0.4.9 tracks changes to the selected video display's maximum refresh rate. On hosts (currently Rubylight) and decoders supporting [in-place reconfiguration](recovery-and-quality.md#choosing-automatic-quality), streams that follow the display rate can update their FPS target and automatic-quality ceiling without reconnecting, even when the phone's refresh rate is unchanged. Returning to **This device** restores phone/tablet display tracking, including native-resolution changes. Received and shown FPS in the overlay remain the measurements of delivered video.

For external MediaCodec video, **Balanced** pacing uses the lowest-latency path for this session, with a notice when it changes. Android's public Choreographer callbacks follow the internal display, so using them to pace an external surface can hold video to the phone's cadence. The saved pacing preference is unchanged; other pacing modes keep their behavior. PyroWave keeps its own Vulkan presentation. External output does not send phase-lock timing derived from the phone display, and this fallback does not claim a measured FPS or VRR result.

The JSON validator, held-input ownership, axis/hat report remapping, zoom bounds and inverse touch mapping have headless unit coverage. Actual monitor attach/detach, OEM SurfaceView transforms, controller hardware and accessibility services still require device testing; no physical display, audio or controller is used by these tests.

## Shortcuts

### App navigation

Outside a stream, use D-pad/Tab to navigate and Enter/A to select. Long-press Select or use Menu/Shift+F10 for PC and app actions. **Ctrl+N** adds a PC, **Ctrl+,** opens Settings, **Ctrl+F** searches Settings, **F1** opens Help, and **Esc/B** goes back.

In Settings, Right opens the selected switch's help (Left in RTL). TV settings put Controls first; choose **Map a button** before capturing a controller mapping. Settings search includes every group, including Advanced; preset chips show the matching preset, Default before you change anything, or Custom.

### During a stream

| Shortcut | Action |
| --- | --- |
| Android Back / Ctrl+Alt+Shift+M | Open the stream menu. Back in a page of the menu returns to the menu; Back again closes it. |
| Ctrl+Alt+Shift+S | Show or hide the performance overlay. |
| Ctrl+Alt+Shift+Z | Toggle input capture. |
| Ctrl+Alt+Shift+C | Toggle the local cursor's visibility and recapture input if needed. With a mouse attached, **stream menu → Input → Show mouse cursor** does the same. |
| Ctrl+Alt+Shift+Q | Disconnect the local stream; the PC app stays running. |
| Hold Start on a controller | Toggle controller mouse mode when enabled. |
| Start+Back+LB+RB on a controller, USB-driver pads included | Open the stream menu once all four are released. The stream keeps running. |
| Start+LB when the controller has no Select/Back button | Send Select/Back. |
| Start+Select when the controller has no Guide button | Send Guide; without Select, use Start+RB. |
| Select+LB when device-motion fallback emulates a touchpad-capable controller | Send a touchpad click. |
| Three-finger tap in Trackpad/Direct mouse mode | Toggle the on-screen keyboard. |
| Stream menu → Overlay (top row) | Show or hide the overlay. **Display → Overlay and diagnostics** chooses Compact, Statistics or Debug. |

Nightjar can also pin app shortcuts to the Android launcher. For external game libraries and intent-based shortcuts, see [frontends](FRONTENDS.md).
