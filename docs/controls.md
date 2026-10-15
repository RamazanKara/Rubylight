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
| Bottom | Reconnect stream, Disconnect, and **Quit game** in red, which asks first. Remote Monitor and Input-only end their session instead. Disconnect also quits a game set to **Quit the game when I leave the stream** (see [leaving a game](#leaving-a-game)). |

**Overlay and diagnostics** opens a page in the same sheet: Compact, Statistics or Debug, Copy stats, Why did my stream stutter? and Measure input latency. Back, Escape or a controller's B returns from the page to the menu; Back again closes the menu, and the close button closes it from anywhere. The D-pad and a controller move through every row; on a USB-driver controller, its D-pad, A and B do this while the menu is open. Items a host or permission does not allow are left out rather than shown disabled.

## Gamepads

Connect a Bluetooth controller in Android's Bluetooth settings or plug a USB controller into the device. **Settings → Controls → Multiple controllers** is on by default and gives each controller its own player; the protocol provides up to 16 controller slots.

Use **Controller buttons** to press a digital button and choose what it sends, including **Nothing**. Mappings are stored per controller model and apply on the next stream, including the USB drivers. **Reset this controller** restores its default mapping.

Open **Sticks, triggers and D-pad** after selecting a controller. Each output stick axis can read either stick's horizontal or vertical axis, optionally inverted, or be disabled. Triggers can read either physical trigger or be disabled; their full analog range is preserved. Each D-pad direction can send another direction, a gamepad button, or nothing. These mappings apply to normalized Android and direct-USB reports, after deadzones, and also apply from the next stream. D-pad directions mapped to the same button stay held until every contributing source is released. Reset restores both digital and axis mappings. Axis mappings do not convert a stick into a trigger or emulate keyboard keys.

**Flip face buttons** swaps A/B and X/Y. Raise the stick deadzone if a stationary stick drifts. **Vibrate device for rumble** gives you rumble on the phone or tablet when the controller has no motors. **Disable rumble** (off by default) stops every vibration the host asks for: controller and trigger rumble, USB DualSense waveform haptics and the device vibration above. It applies from the next stream.

### Controller test

Open **Settings → Controls → Controller test**, or **Test controller** on the Controller buttons page, to check a controller without a stream:

- **Buttons** light up while held, and the line under them names them for screen readers. LT and RT count as pressed past half travel.
- **Sticks and triggers** draw both sticks and both triggers and print their values, followed by every raw axis the controller reports.
- **Rumble** runs each motor the controller exposes (strong, weak, and the triggers where present) for a moment; **Stop** ends it.
- **Lights** sets an RGB lightbar to red, green or blue where Android 12+ or Nightjar's USB driver can drive it; **Reset** gives it back.
- **Motion sensors** and **Touchpad** show the gyro, accelerometer and finger position where the controller has them.

Anything a controller or Android version lacks says so instead of being hidden. Pads that Nightjar's USB driver owns (see below) are read through the driver, as in a stream, and Android 12+ is needed for rumble per motor, lights and motion on other pads. The page works with a D-pad or controller alone: the D-pad and A move around it, and holding B or Back for one second leaves it. Leaving stops rumble, restores the lights and hands USB pads back to Android.

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

**Fixed-step mouse movement** (off by default) helps with small targets: while you hold **X**, each stick push moves the pointer a fixed number of pixels in the stick's direction instead of speeding up with the push. Press **Y** to double the step, 1, 2, 4 and so on up to 128 pixels, then back to 1; a short message shows the new step.

## Touch modes

Choose **Settings → Controls → Touch mode**, or **stream menu → Input → Touch mode**.

| Mode | Gestures and behavior |
| --- | --- |
| Trackpad (default) | Drag to move the pointer relatively. Tap with one finger for left-click, two for right-click. Hold then drag to drag with a mouse button; move two fingers to scroll. |
| Direct mouse | Tap a location to click there, drag to move while holding the left button, and hold still for right-click. A second finger scrolls. |
| Multi-touch | Send fingers directly to the host touch interface. If native touch is unavailable, Nightjar uses Direct mouse. Use the stream menu for the keyboard. |
| Local cursor | Like Trackpad, but Nightjar draws the pointer on this device, so it follows your finger with no round trip to the PC. The pointer's position goes to the host as an absolute position, so a tap clicks exactly where the drawn arrow points, including while pinch zoom is on. Tap, two-finger tap, hold-then-drag and two-finger scroll work as in Trackpad. The drawn pointer is a plain arrow; the PC's own cursor may still show in the video. |
| Touch off | The stream ignores fingers, for playing with a controller without stray touches. Android's Back gesture or Back button still opens the stream menu, where you can switch the mode back. On-screen controls, per-game touch buttons, pinch zoom (if you turned it on), a mouse and a pen keep working. |

**Trackpad speed** changes Trackpad movement and **Local cursor speed** changes Local cursor movement, each from 25–400% with 100% as the default. At 100%, the local pointer moves as far as your finger. Native pen input is forwarded when supported and permitted by the host.

These options in **Settings → Controls** shape Trackpad and Local cursor movement. All are off or unchanged by default and apply from the next stream:

- **Pointer acceleration** with **Acceleration strength** (Gentle, Medium or Strong): slow finger movement moves the pointer less, for precise aiming, and fast swipes move it farther.
- **Glide after a flick** with **Glide distance** (Short, Medium or Long): after a quick flick, the pointer keeps moving and slows down. Touching the screen stops it at once, and so do changing the touch mode, opening the stream menu, picture-in-picture, leaving the app and disconnecting. A finger that rests before lifting, a drag and a two-finger scroll do not glide.
- **Trackpad area** (Full screen, Right half, Left half, Bottom half or Top half): Trackpad and Local cursor gestures count only when the first finger lands in that part of the screen; a gesture that starts elsewhere is ignored until every finger lifts. Use it to keep a thumb resting on one side, for example in strategy games.
- **Three-finger tap**: a quick tap with three fingers opens Android's keyboard (the default) or sends a middle click. It works in Trackpad, Direct mouse and Local cursor; a tap with four fingers or one where the fingers travel does not count.

With an external display, the phone always works as a Trackpad (or stays off with Touch off), since a pointer drawn on the phone would not match the external screen.

How acceleration and glide feel depends on the touch screen's sampling rate; try the strengths on your device.

Direct mouse, native touch and pen positions follow the streamed image, including when cutouts, fold panes or window padding shift it. Touch permission and a working host touch interface are still required for Multi-touch.

## Mouse and keyboard

USB and Bluetooth mice and keyboards use Android's input support. Android 8+ provides native pointer capture. **Mouse speed** scales relative movement from 25–400% (default 100%).

Leave **Desktop mouse mode** off for games that need relative mouse look; enable it for desktop pointer behavior. Enable **Mouse back/forward buttons** only when needed; some devices report right-click through the same Android key path.

Open the stream menu and select **Keyboard** in its top row for Android's on-screen keyboard. It stays hidden when a stream starts, including on tablets. Hardware key events and text input are sent to the host; Android may reserve system keys. Check the host keyboard layout if punctuation or language-specific keys differ.

Select **stream menu → Input → Extra keyboard keys** for a toolbar with Esc, Tab, Enter, Backspace, Delete, Home/End, Page Up/Down, arrows and F1–F12. Scroll each row horizontally to reach more keys. Ctrl, Alt, Shift and Win apply to the next key pressed in this toolbar, then reset. **Hide keys** closes the toolbar; **Keyboard** opens Android's text keyboard.

The toolbar also provides **Task Manager** (Ctrl+Shift+Esc), **Switch window** (Alt+Tab) and **Close window** (Alt+F4). To stop an unresponsive app, open Task Manager and use its **End task** action on the host. These controls require the host's keyboard permission.

### Keyboard options

Both are in **Settings → Controls → Keyboard** and off by default.

- **Keyboard Back sends Windows key**: the Back key of a physical keyboard sends the Windows key (Command on a Mac host). Only keyboard Back changes: a controller's Back, a TV remote, Android's navigation button or Back gesture and a mouse's right-click Back keep their usual roles. With this on, open the stream menu with **Ctrl+Alt+Shift+M**, Android's Back gesture or navigation button, or **Start+Back+LB+RB** on a controller. While input is released (Ctrl+Alt+Shift+Z), keyboard Back opens the menu as before.
- **Right Alt as Command**: Right Alt sends the Windows key, which macOS hosts use as Command, so Right Alt+C and Right Alt+V copy and paste. Left Alt stays Alt. Leave it off if you type characters with AltGr, since Right Alt then no longer works as AltGr.

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

Choose **stream menu → Display → Enable pinch zoom and pan**. Pinch to zoom from 1× to 4× and drag two fingers to pan within the desktop. One finger continues to use the selected touch mode; direct mouse, native touch, pen and absolute mouse positions use the inverse video transform. Trackpad movement stays relative; in Local cursor mode the drawn pointer stays over the same spot of the zoomed image and moves as far as your finger. While enabled, the two-finger gesture controls the view instead of scrolling the PC. **Reset zoom** restores the full image. Android 5/6 require TextureView; Android 7+ support the SurfaceView path as well.

For a dock, USB-C/HDMI monitor or Android presentation-capable wireless display, choose **stream menu → Display → Video output** and select the display. The item appears while Android offers such a display, and while video is on one. Switching output safely disconnects/reconnects the client while preserving the PC application, and starts the decoder on a dedicated presentation surface. Game streams resume only the captured application: if it stops or another application replaces it during handoff, the reconnect fails without launching or quitting an application. This guard is independent of automatic network recovery. Remote Monitor keeps its dedicated view-only host operation. Video goes to the monitor; the device remains a touchpad with its keyboard toolbar, game buttons and stream menu. This works with any Moonlight-compatible host and does not ask the host to change its display layout. Android determines which attached outputs support presentations; mirroring-only outputs cannot be selected.

Select **This device** to return. Removing the external display triggers the same client reconnect back to the phone/tablet. A display that disappears before launch falls back to the device. Output selection is session-local and is preserved through reconnects, not saved as a global preference. PiP auto-entry is disabled while presenting externally. Locking/leaving the activity follows the normal stream lifecycle; no background input or unattended reconnect is introduced.

Android 0.4.9 tracks changes to the selected video display's maximum refresh rate. On hosts (currently Rubylight) and decoders supporting [in-place reconfiguration](recovery-and-quality.md#choosing-automatic-quality), streams that follow the display rate can update their FPS target and automatic-quality ceiling without reconnecting, even when the phone's refresh rate is unchanged. Returning to **This device** restores phone/tablet display tracking, including native-resolution changes. Received and shown FPS in the overlay remain the measurements of delivered video.

For external MediaCodec video, **Balanced** pacing uses the lowest-latency path for this session, with a notice when it changes. Android's public Choreographer callbacks follow the internal display, so using them to pace an external surface can hold video to the phone's cadence. The saved pacing preference is unchanged; other pacing modes keep their behavior. PyroWave keeps its own Vulkan presentation. External output does not send phase-lock timing derived from the phone display, and this fallback does not claim a measured FPS or VRR result.

The JSON validator, held-input ownership, axis/hat report remapping, zoom bounds and inverse touch mapping have headless unit coverage. Actual monitor attach/detach, OEM SurfaceView transforms, controller hardware and accessibility services still require device testing; no physical display, audio or controller is used by these tests.

## Leaving a game

Disconnect ends the stream on this device and leaves the game running on the PC, so you can resume it later. To choose per game:

- **Quit the game when I leave the stream**: long-press the game in the library (or use its options button) and choose this row to turn it on or off; the row says what Disconnect will do. It is stored per PC and game on this device. With it on, **Disconnect**, **Ctrl+Alt+Shift+Q** and Disconnect on the picture-in-picture window ask the PC to quit the game, then close the stream. If the PC refuses or does not answer, the stream still closes and a message says the game is still running. A lost connection, an error, the PC ending the stream, or leaving the app without choosing Disconnect never quits the game, so a dropped Wi-Fi link can't cost you unsaved progress. The row is shown when the host allows this device to launch and quit games; Remote Monitor, Input-only and host actions are not offered it.
- **Quit game** right away: long-press the running game in the library, or the PC in the PC list, and choose **Quit game**, which asks first. It needs the same permission.
- **Quick resume** (Settings → App): a tap on a PC that is running a game goes straight back into the game instead of opening the library. The PC's menu still opens the library. Off by default. In the library, a tap on the running game always resumes it.

Quitting uses the standard Moonlight request, so it works with any compatible host that gives this device permission to launch apps. A host may refuse to quit a game another device started.

## Shortcuts

### App navigation

Outside a stream, use D-pad/Tab to navigate and Enter/A to select. Long-press Select or use Menu/Shift+F10 for PC and app actions. **Ctrl+N** adds a PC, **Ctrl+,** opens Settings, **Ctrl+F** searches Settings, **F1** opens Help, and **Esc/B** goes back.

In Settings, Right opens the selected switch's help (Left in RTL). TV settings put Controls first; choose **Map a button** before capturing a controller mapping. Settings search includes every group, including Advanced; preset chips show the matching preset, Default before you change anything, or Custom.

### During a stream

| Shortcut | Action |
| --- | --- |
| Android Back / Ctrl+Alt+Shift+M | Open the stream menu. Back in a page of the menu returns to the menu; Back again closes it. With **Keyboard Back sends Windows key** on, a keyboard's Back key sends Win instead. |
| Ctrl+Alt+Shift+S | Show or hide the performance overlay. |
| Ctrl+Alt+Shift+Z | Toggle input capture. |
| Ctrl+Alt+Shift+C | Toggle the local cursor's visibility and recapture input if needed. With a mouse attached, **stream menu → Input → Show mouse cursor** does the same. |
| Ctrl+Alt+Shift+Q | Disconnect the local stream; the PC app stays running unless it is set to quit when you leave. |
| Hold Start on a controller | Toggle controller mouse mode when enabled. |
| Hold X / press Y in controller mouse mode | Move in fixed steps / double the step, with **Fixed-step mouse movement** on. |
| Start+Back+LB+RB on a controller, USB-driver pads included | Open the stream menu once all four are released. The stream keeps running. |
| Start+LB when the controller has no Select/Back button | Send Select/Back. |
| Start+Select when the controller has no Guide button | Send Guide; without Select, use Start+RB. |
| Select+LB when device-motion fallback emulates a touchpad-capable controller | Send a touchpad click. |
| Three-finger tap in Trackpad, Direct mouse or Local cursor mode | Toggle the on-screen keyboard, or middle-click when **Three-finger tap** is set to Middle click. |
| Stream menu → Overlay (top row) | Show or hide the overlay. **Display → Overlay and diagnostics** chooses Compact, Statistics or Debug. |

Nightjar can also pin app shortcuts to the Android launcher. For external game libraries and intent-based shortcuts, see [frontends](FRONTENDS.md).
