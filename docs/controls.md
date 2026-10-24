# Controls

[Documentation](index.md) · [All defaults](settings.md)

If your host has per-device permissions, it must grant the relevant input permission to this device. If video works but controls do not, check [host permissions](pairing.md#per-device-permissions) first.

## The stream menu

Open it with Android **Back** (swipe back from the screen edge with gesture navigation), **Ctrl+Alt+Shift+M** on a keyboard, or **Start+Back+LB+RB** on a controller, released together; this works for USB-driver controllers too. The first stream shows a one-line hint naming the ways this device has; tap it to close it.

The menu is one sheet:

| Part | What it holds |
| --- | --- |
| Top row | **System keyboard** (Android's keyboard), **PC keyboard** (the full on-screen PC keyboard, see below), **Overlay** (show or hide the performance overlay) and **Microphone** (mute or unmute, when the PC offers a microphone). Overlay and Microphone switch in place. |
| Input | Touch mode, Show mouse cursor (with a mouse attached), Show on-screen controls, Edit controller layout, Controls for this game, Keyboard & mouse. |
| Display | Video scaling (Fit, Fill, Stretch or 1:1, local video without upscaling), Enable pinch zoom and pan, Reset zoom, **Overlay and diagnostics**, Picture-in-picture, Video output (with an external display). |
| Quality | Set bitrate, Automatic stream quality and Automatic bitrate, where the host supports them. |
| Host | Server commands, Send clipboard to host, Copy host clipboard, Open host web UI, Permissions and frame limits, as the host and its permissions allow. |
| Bottom | Reconnect stream, Disconnect, and **Quit game** in red, which asks first. Remote Monitor and Input-only end their session instead. Disconnect also quits a game set to **Quit the game when I leave the stream** (see [leaving a game](#leaving-a-game)). |

**Overlay and diagnostics** opens a page in the same sheet: Compact, Statistics or Debug, Copy stats, Why did my stream stutter? and Measure input latency. Back, Escape or a controller's B returns from the page to the menu; Back again closes the menu, and the close button closes it from anywhere. The D-pad and a controller move through every row; on a USB-driver controller, its D-pad, A and B do this while the menu is open. Items a host or permission does not allow are left out rather than shown disabled.

**Host → Server commands** runs commands configured on the host, including Sleep. Nightjar asks for confirmation and checks the command and permission again before sending it. **Command sent; result unknown** confirms submission, not successful execution: the protocol provides no execution result. If the PC stays awake, check the host log for the command execution and any error, then check that the same configured command can put that PC to sleep when run on the host. The command's elevation setting and the PC's supported sleep states are controlled on the host.

## Gamepads

Connect a Bluetooth controller in Android's Bluetooth settings or plug a USB controller into the device. **Settings → Controls → Multiple controllers** is on by default and gives each controller its own player; the protocol provides up to 16 controller slots.

Use **Controller buttons** to press a digital button and choose what it sends, including **Nothing**. Mappings are stored per controller model and apply on the next stream, including the USB drivers. **Reset this controller** restores its default mapping.

Open **Sticks, triggers and D-pad** after selecting a controller. Each output stick axis can read either stick's horizontal or vertical axis, optionally inverted, or be disabled. Triggers can read either physical trigger or be disabled; their full analog range is preserved. Each D-pad direction can send another direction, a gamepad button, or nothing. These mappings apply to normalized Android and direct-USB reports, after deadzones, and also apply from the next stream. D-pad directions mapped to the same button stay held until every contributing source is released. Reset restores both digital and axis mappings. Axis mappings do not convert a stick into a trigger or emulate keyboard keys.

**Flip face buttons** swaps A/B and X/Y. Raise the stick deadzone if a stationary stick drifts. **Vibrate device for rumble** gives you rumble on the phone or tablet when the controller has no motors. **Disable rumble** (off by default) stops every vibration the host asks for: controller and trigger rumble (including the USB Switch Pro driver), USB DualSense waveform haptics and the device vibration above. It applies from the next stream.

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

For a GameSir G8+ MFi, use its green **Xbox mode** for direct USB input and rumble. Keep **USB controller driver** enabled and allow Nightjar’s USB access request when you start a stream or open **Controller test**. If Android already recognizes the controller, enable **Always use the USB driver** and reconnect it. Blue **Android mode** uses Android’s input support, which may not expose rumble. See the [GameSir manual](https://gamesir.com/support/manuals/gamesir-g8-plus-mfi) for switching modes.

### Switch Pro over USB

**Untested on hardware.** This driver was written from the published protocol notes and has not yet been tried with a real controller. If it misbehaves, turn it off; Android's own input path keeps working.

Android reads a Switch Pro Controller through its own input path; what that offers for motion, rumble and the Capture button depends on the device and Android version. For the direct USB driver:

1. Enable **Settings → Controls → USB Switch Pro driver** (off by default).
2. Connect a Nintendo Switch Pro Controller by USB and start a stream.
3. Allow Android's USB access request. Denying it keeps the ordinary Android input path; unplug and reconnect to retry.

The driver reads the factory or user stick calibration and motion calibration from the controller, then sends all buttons including **Home** (Guide) and **Capture** (sent as the Misc button), both sticks, ZL/ZR as fully pressed or released triggers, gyro and accelerometer in the same axes as a DualSense, battery state and rumble. The face buttons follow their position, like Android's own Switch driver: the bottom button (B on the Switch) is A. Use **Flip face buttons** or **Controller buttons** to match the labels instead. The stream's stick deadzone applies as for any controller, the player lights show the controller's slot, and **Disable rumble** silences it. The host's two rumble motors map to the controller's low and high band at a fixed 150 Hz; the host's HD rumble waveforms are not used.

Unplugging the controller releases every button and stops motion on the host; ending the stream stops rumble, turns the motion sensors off and hands the controller back to Android. Joy-Con and the Joy-Con Charging Grip are not claimed by this driver and stay on Android's input path. Third-party Switch controllers with other USB IDs are not claimed either. The **Controller test** page reads the controller through this driver when it is on.

### Controller mouse mode

With **Controller mouse mode** enabled, hold **Start** to toggle it. By default the left stick moves the pointer, the right stick scrolls, **A** left-clicks and **B** right-clicks. **Scroll with an analog stick** can swap the sticks or make both move the pointer.

**Fixed-step mouse movement** (off by default) helps with small targets: while you hold **X**, each stick push moves the pointer a fixed number of pixels in the stick's direction instead of speeding up with the push. Press **Y** to double the step, 1, 2, 4 and so on up to 128 pixels, then back to 1; a short message shows the new step.

## Physical Back on a handheld

If a handheld's dedicated **Back** button works in the app but does not open the stream menu, enable **Settings → Controls → Physical Back opens stream menu**, then start a new stream. This covers devices such as AYN Thor that can report the physical button through their gamepad input device.

The option is off by default to preserve existing controller mappings. When enabled, a physical `KEYCODE_BACK` opens Nightjar's stream menu instead of being forwarded to the PC. **Select** and **Start** retain their mappings and the **Start+Back+LB+RB** menu shortcut still works. Mouse right-click, Android's Back gesture and **Keyboard Back sends Windows key** on an alphabetic keyboard retain their behavior. A button configured by Android or the handheld's software to emit another key code needs to be mapped to Android Back first.

## TV and Fire TV remotes

In the PC list, library, settings and stream menus, use the **D-pad** to move focus, **Select/OK** to activate the highlighted item and **Back** to return. On a selected PC or game, the remote's **Menu** button opens its actions. Long-pressing Select also opens the card's actions.

During a stream, **Back** and **Menu** open Nightjar's stream menu. They stay local to Nightjar, so a Fire TV remote cannot accidentally send gamepad Start/Select to the PC or lose the way out of the stream. In the open menu, D-pad/Select navigate its controls and Back returns to the preceding page or closes it.

With the menu closed and input captured, the remote's **D-pad** sends PC arrow keys and **Select** sends Enter. The host must allow keyboard input for that device. A TV remote provides basic keyboard navigation; use a controller or mouse for games or desktop actions that need those inputs. Connected gamepads retain their controller mapping, including the Start button and menu shortcut. Home, voice search and other system-reserved buttons follow the TV's behavior.

This detection follows Amazon's [remote identification](https://developer.amazon.com/docs/fire-tv/identify-controllers.html) and [remote button events](https://developer.amazon.com/docs/fire-tv/remote-input.html): a non-alphabetic D-pad device without gamepad buttons or joystick axes is a remote. An alphabetic keyboard with arrow keys keeps its keyboard behavior.

## Menu navigation

Use the D-pad to move between cards, settings, menu rows, dialog choices and buttons. **Select/OK** or gamepad **A** activates the focused control. **Back**, gamepad **B** and keyboard **Escape** return from a menu page or cancel an ordinary dialog; canceling a confirmation does not run its action. Dialogs that require an explicit choice still require their on-screen buttons.

In the PC/game grids, **Menu** opens the selected card's actions. A card's small options button can also be highlighted with the direction toward it, then activated. Settings rows expose their explanation with **D-pad right** (left in right-to-left layouts), or **F1**. Sliders in an open editor use left/right to adjust; move focus to **Save/OK** to apply or cancel to keep the prior value. Controller mapping captures inputs only after you explicitly start capture.

## Touch modes

Choose **Settings → Controls → Touch mode**, or **stream menu → Input → Touch mode**.

| Mode | Gestures and behavior |
| --- | --- |
| Trackpad (default) | Drag to move the pointer relatively. Tap with one finger for left-click, two for right-click. Lift both fingers quickly in either order. Hold then drag to drag with a mouse button; move two fingers to scroll. |
| Direct mouse | Tap and release to left-click at that location. Move beyond the small hold area to drag with the left button, or hold still for about 0.65 seconds to right-click. A hold sends only the right click. A second finger scrolls. |
| Multi-touch | Send fingers directly to the host touch interface. If native touch is unavailable, Nightjar uses Direct mouse. Use the stream menu for the keyboard. |
| Local cursor | Like Trackpad, but Nightjar draws the pointer on this device, so it follows your finger with no round trip to the PC. The pointer's position goes to the host as an absolute position, so a tap clicks exactly where the drawn arrow points, including while pinch zoom is on. Tap, two-finger tap, hold-then-drag and two-finger scroll work as in Trackpad. The drawn pointer is a plain arrow and the PC's own cursor may still show in the video, unless the host offers [cursor shapes](protocol/cursor-shape.md): then Nightjar draws the PC's cursor (I-beam, resize arrows and so on), hides it when the PC does, and the host leaves it out of the video. No host offers this yet. |
| Touch off | The stream ignores fingers, for playing with a controller without stray touches. Android's Back gesture or Back button still opens the stream menu, where you can switch the mode back. On-screen controls, per-game touch buttons, pinch zoom (if you turned it on), a mouse and a pen keep working. |

**Trackpad speed** changes Trackpad movement and **Local cursor speed** changes Local cursor movement, each from 25–400% with 100% as the default. At 100%, the local pointer moves as far as your finger. Native pen input is forwarded when supported and permitted by the host.

These options in **Settings → Controls** shape Trackpad and Local cursor movement. All are off or unchanged by default and apply from the next stream:

- **Pointer acceleration** with **Acceleration strength** (Gentle, Medium or Strong): slow finger movement moves the pointer less, for precise aiming, and fast swipes move it farther.
- **Glide after a flick** with **Glide distance** (Short, Medium or Long): after a quick flick, the pointer keeps moving and slows down. Touching the screen stops it at once, and so do changing the touch mode, opening the stream menu, picture-in-picture, leaving the app and disconnecting. A finger that rests before lifting, a drag and a two-finger scroll do not glide.
- **Trackpad area** (Full screen, Right half, Left half, Bottom half or Top half): Trackpad and Local cursor gestures count only when the first finger lands in that part of the screen; a gesture that starts elsewhere is ignored until every finger lifts. Use it to keep a thumb resting on one side, for example in strategy games.
- **Three-finger tap** and **Four-finger tap**: assign each gesture to **System keyboard**, **PC keyboard**, **Keyboard & mouse**, **Middle click**, or **Off** in Settings → Controls → Touch & mouse. Three fingers open Android’s keyboard by default; four fingers are off by default. **Keyboard & mouse** opens or hides the desktop panel in its saved mode. These gestures work in Trackpad, Direct mouse and Local cursor modes. Lift all fingers quickly without dragging. Four fingers never also trigger the three-finger action; five fingers, a drag or a canceled touch do not trigger either shortcut.

With an external display, the phone always works as a Trackpad (or stays off with Touch off), since a pointer drawn on the phone would not match the external screen.

How acceleration and glide feel depends on the touch screen's sampling rate; try the strengths on your device.

Direct mouse, native touch and pen positions follow the streamed image, including when cutouts, fold panes or window padding shift it. Touch permission and a working host touch interface are still required for Multi-touch.

## Mouse and keyboard

USB and Bluetooth mice and keyboards use Android's input support. Android 8+ provides native pointer capture. **Mouse speed** scales relative movement from 25–400% (default 100%).

Leave **Desktop mouse mode** off for games that need relative mouse look; enable it for desktop pointer behavior. Enable **Mouse back/forward buttons** only when needed; some devices report right-click through the same Android key path.

Open the stream menu and select **System keyboard** in its top row for Android's on-screen keyboard. Opening it hides the desktop panel. It stays hidden when a stream starts, including on tablets. Hardware key events and text input are sent to the host; Android may reserve system keys. Check the host keyboard layout if punctuation or language-specific keys differ.

Select **stream menu → Input → Keyboard & mouse** for the desktop panel. Use the mode selector in the compact toolbar to choose **Keyboard**, **Trackpad**, or **Combined**. The adjacent key-bank selector opens its choices in the same toolbar; select a bank or use Back to return. Keyboard banks provide letters and numbers, F1–F12, navigation and punctuation, a number pad, and shortcuts. Bank choices scroll horizontally; tall key layouts scroll vertically. The single toolbar frees screen space while preserving key sizes and any custom panel height. Combined mode puts the trackpad beside the keyboard on wide screens and below it on narrow screens.

Tap Ctrl, Alt, Shift or Win to combine it with the next panel key; hold a modifier to send it alone, for example Win to open Start. Selected modifiers stay local until a key is sent. **Shortcuts** includes copy/paste, Show desktop, **Task Manager** (Ctrl+Shift+Esc), **Switch window** (Alt+Tab) and **Close window** (Alt+F4). **System keyboard** opens Android's text keyboard, which uses its own modifiers.

Move one finger on the trackpad to move the pointer; tap to left-click, use two fingers to scroll, or tap two fingers to right-click. Hold **L** while moving on the pad to drag. The **L/R** buttons work independently of keys and the pad, including in Combined mode.

The panel follows **Local cursor** and **Desktop mouse mode**, using its own pointer-speed setting. Movement scales to the streamed picture, including local-cursor zoom, so a dense display does not slow it down. Empty panel margins absorb touches, and the panel leaves keyboard focus on the stream. In split-screen or freeform windows, the first direct button press works while Android is restoring window focus.

The gear opens customization: key and panel size, background opacity, pointer speed, scrolling direction, tap-to-click, mouse buttons (including optional middle/Back/Forward), trackpad side, visible banks, and modifiers that stay selected for several keys. **Edit keyboard shortcuts** adds, edits, deletes or reorders saved chords inside the stream. Apply saves panel settings for this device; Reset panel restores defaults while keeping saved shortcuts. Closing the panel, changing modes, losing focus or disconnecting releases held mouse buttons and clears modifiers. The host's input permissions still apply.

### PC keyboard

**stream menu → PC keyboard** (top row), **Ctrl+Alt+Shift+K** or a finger-tap gesture set to **PC keyboard** opens a full PC keyboard over the bottom of the stream: Esc and F1-F12, the number row, letters, Tab, Caps, Enter, Backspace, Space, arrows, Home, End, PgUp, PgDn, Ins, Del and PrtSc. It is for keys a phone keyboard lacks, such as F-keys, Ctrl combinations and Win. Opening it closes the desktop panel and the system keyboard, and the other way round.

- **Modifiers are sticky.** Tap Ctrl, Alt, Shift or Win once to apply it to the next key (blue), tap twice to lock it (amber) and tap again to release it. Locked modifiers stay held on the PC until released. A Win or Alt that was never combined with a key sends a neutral key first when it is released, so it does not open the Start menu or the menu bar; use Ctrl+Esc for Start.
- **Keys repeat** while held, like a PC keyboard, and several fingers can press at once.
- **Layout.** English (US) or German (QWERTZ), chosen from the system language (German legends only for German, Austrian, Luxembourg and Liechtenstein locales, so Swiss German gets US). Change it for this stream with the layout label in the keyboard's header, or permanently in **Settings → Controls → Keyboard → Keyboard layout**. The keys send key positions, not characters, so the Windows layout on the PC decides which character appears. Set both to the same layout.
- **Size and opacity** are in the same settings: the keyboard takes 30-70% of the screen height (default 50%) and 30-100% opacity (default 90%). It only covers the bottom of the stream and respects display cutouts and system bars. It fits landscape phones, handhelds and TVs.
- **Closing releases everything.** Closing the keyboard, leaving the stream or losing input releases every key and modifier it pressed, so none stays stuck on the PC.

While the keyboard is open, controllers and TV remotes steer it and the game gets no controller input; a physical keyboard and mouse keep working. Closing it gives the controller back to the game, and buttons still held at that moment reach the game only after they were let go.

| Control | Action |
| --- | --- |
| D-pad or left stick | Move focus between keys (the focused key has a white outline). Holding repeats the move. |
| A (OK on a TV remote, Select) | Press the focused key. Hold to repeat. Modifier keys arm, lock and release. |
| B (or the controller's own Back) | Close the keyboard. |
| X | Backspace |
| Y | Space |
| LB / RB | Toggle Shift / Ctrl |
| Start | Enter |

With **Flip face buttons** on, A/B and X/Y swap here as they do in games, and a button you remapped for that controller in the button map does what its target does here (a disabled button does nothing). A TV remote uses the D-pad and OK. A controller that reports its D-pad as keys and as a hat moves the selection once. USB and Bluetooth controller behaviour is researched but not yet tried on real pads; see [USB and Bluetooth pads in the menus and the PC keyboard](gamepad-navigation.md). This works for Android-mapped and USB-driver controllers alike.

### Keyboard options

Both are in **Settings → Controls → Keyboard** and off by default.

- **Keyboard Back sends Windows key**: the Back key of a physical keyboard sends the Windows key (Command on a Mac host). Only keyboard Back changes: a controller's Back, a TV remote, Android's navigation button or Back gesture and a mouse's right-click Back keep their usual roles. With this on, open the stream menu with **Ctrl+Alt+Shift+M**, Android's Back gesture or navigation button, or **Start+Back+LB+RB** on a controller. While input is released (Ctrl+Alt+Shift+Z), keyboard Back opens the menu as before.
- **Right Alt as Command**: Right Alt sends the Windows key, which macOS hosts use as Command, so Right Alt+C and Right Alt+V copy and paste. Left Alt stays Alt. Leave it off if you type characters with AltGr, since Right Alt then no longer works as AltGr.

### Custom keyboard shortcuts

Open **Settings → Controls → Keyboard shortcuts** and choose **Add shortcut**. Enter a button name and a chord such as `CTRL + SHIFT + ESC`, `WIN + D` or `ALT + F4`, then **Save**. Select a saved button to change its name or keys, or choose **Delete**. Unsaved editor text survives screen rotation.

Open **stream menu → Input → Keyboard & mouse → Shortcuts** to use your buttons. Tapping one presses its keys in the listed order and releases them in reverse order; put modifiers first. Panel modifier selections reset when a shortcut is sent unless you enabled the keep-selected option. Shortcuts require the host's keyboard permission and use the same input path as the built-in panel keys.

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

### Per-game controls

Choose **stream menu → Input → Controls for this game → Add control** and pick a type:

- **Keyboard or mouse button**: a key or chord (`W`, `SPACE`, `CTRL + C`) or a mouse button (`MOUSE_1` left, `MOUSE_2` middle, `MOUSE_3` right, `MOUSE_4` back, `MOUSE_5` forward).
- **Gamepad button**: A, B, X, Y, LB, RB, LS, RS (stick clicks), Back, Start or Guide.
- **Analog trigger**: LT or RT. Pressing it pulls the trigger fully.
- **Analog stick**: the left or right stick. The deadzone (0–50% of the stick's travel) sends nothing near the center, and the rest of the travel is rescaled so small movements still register.
- **D-pad**: eight directions, with the center quarter ignored.
- **WASD stick**: a stick that holds keys instead of moving an axis, `W + A + S + D` by default (up, left, down, right). **Diagonals** sets how far either side of a diagonal both neighboring keys are held: 0 never holds two keys, 45 gives eight equal directions, 90 holds two keys everywhere off the axes.

Sticks and WASD sticks can be **floating**: the stick centers wherever the thumb lands inside its area, so make the area large (for example the left third of the screen). A fixed stick centers in the middle of its area.

Buttons and triggers hold while pressed. **Tap to hold; tap again to release** turns one into a toggle for actions that would otherwise need a long press. TalkBack, Switch Access and keyboard activation can also activate a button; ordinary activation sends an 80 ms press. Sticks, D-pads and WASD sticks take touch only.

The same key, mouse button or gamepad button can be on several controls. Each control releases only its own press, so a shared key stays down until every control holding it lets go. Two sticks on one side send whichever is pushed further.

Gamepad controls join controller 1, the same slot as the fixed on-screen gamepad and a first physical controller; their input is combined. They work without turning on the fixed on-screen gamepad, and like it they send each change a few more times over the next 75 ms so a dropped packet cannot leave a stick pushed.

Set width and height as percentages of the control surface; every control keeps at least a 48 dp touch target. Select **Move game buttons**, drag controls into place, then **Save layout and play**. Touch input to the PC is paused while placing controls. **Snap to grid** (Off, 2.5%, 5% or 10% of the shorter screen edge) shows the grid while moving and snaps each control's top-left corner to it; a control that cannot reach the far edge on the grid can sit flush against it. The grid size is saved for this device. Select a control's name in this menu to edit or delete it.

Layouts and their visibility are saved separately for each PC and game, using the app UUID when available and its numeric ID otherwise. Resume and Running Game tiles select the established game's saved controls when the stream connects. Opening the menu, hiding the layout, entering picture-in-picture, the stream losing focus, leaving the activity, disconnecting, canceling a touch or replacing a layout releases everything the layout holds: keys and mouse buttons go up, gamepad buttons go up, triggers return to 0 and sticks recenter. The host's ordinary keyboard, mouse and gamepad permissions still apply; no host extension is needed.

**Export or share layout** offers selectable JSON, **Copy JSON**, and a JSON file through Android's share sheet. The share sheet may leave the stream according to Android's activity lifecycle; the PC game remains running. On another game or device, **Import layout** accepts pasted JSON and replaces that game's controls only after pressing **Replace**. Layouts are limited to 32 controls and 64 KiB. Coordinates are normalized so layouts scale across devices; review placement on a different aspect ratio.

Layouts with only keyboard and mouse buttons are still saved as version 1, so older Nightjar releases can import them. A layout with any other control is version 2. Unsupported versions, duplicate IDs, unsupported keys or buttons, out-of-range deadzone or diagonal values, nonfinite or off-screen geometry and invalid mouse bindings are rejected without changing the current layout, and the message names the problem and, for a single control, its number and name.

#### Import from Artemis

To move a layout from Artemis, export it there (**Export keyboard layout** for the on-screen keyboard, and keep your **Custom Keys** JSON file if you use custom keys). In Nightjar, long-press the game in the library (or use its options button), choose **Import game controls**, and pick the layout file, the Custom Keys file, or both together. **Import layout** in the stream menu also accepts either file's text. Nightjar shows what it will import and lists everything it skipped, with the reason, before you press **Replace**.

| Artemis element | In Nightjar |
| --- | --- |
| Keys (`key_…`), including toggling ones | Keyboard buttons; toggling ones become **Tap to hold** |
| Mouse buttons left, middle, right, and the toggling right button | Mouse buttons |
| Keyboard stick (`rocker_1`) and keyboard D-pad (`dpad_1`) | WASD sticks with W, A, S and D; the stick's double-tap center key (Shift) is skipped |
| Custom keys (`VK_*` names or `0x..` codes; `sticky` toggles) | Keyboard buttons; `VK_LBUTTON`, `VK_MBUTTON`, `VK_RBUTTON`, `VK_XBUTTON1/2` alone become mouse buttons. Keys the Moonlight protocol cannot send, and keys mixed with mouse buttons, are skipped |
| Touchpad areas (`m_9`, `m_10`, `m_11`) | Skipped: no equivalent |
| Elements turned off or hidden in Artemis | Left out, and counted in the summary |

Artemis saves pixel positions without the screen size, so Nightjar reads them against this device's screen in landscape. A layout made on a larger screen is scaled down to fit, and the summary says so; check the placement either way. A Custom Keys file on its own has no positions, so its keys are laid out in rows as Artemis does, made smaller where needed (as on a portrait screen) so that every key fits without covering another. Custom keys placed in a layout file need the Custom Keys file picked together with it; otherwise they are listed as skipped. Layouts of Artemis's fixed virtual gamepad are not exported by Artemis and so cannot be imported.

### Tablet zoom and external displays

Choose **stream menu → Display → Enable pinch zoom and pan**. Pinch to zoom from 1× to 4× and drag two fingers to pan within the desktop. One finger continues to use the selected touch mode; direct mouse, native touch, pen and absolute mouse positions use the inverse video transform. Trackpad movement stays relative; in Local cursor mode the drawn pointer stays over the same spot of the zoomed image and moves as far as your finger. While enabled, moving two fingers controls the view instead of scrolling the PC. A quick two-finger tap still right-clicks in Trackpad and Local cursor, within the configured trackpad area. Small tap movement does not pan the view, and a pan or pinch does not also click. **Reset zoom** restores the full image. Android 5/6 require TextureView; Android 7+ support the SurfaceView path as well.

For a dock, USB-C/HDMI monitor or Android presentation-capable wireless display, choose **stream menu → Display → Video output** and select the display. The item appears while Android offers such a display, and while video is on one. Switching output safely disconnects/reconnects the client while preserving the PC application, and starts the decoder on a dedicated presentation surface. Game streams resume only the captured application: if it stops or another application replaces it during handoff, the reconnect fails without launching or quitting an application. This guard is independent of automatic network recovery. Remote Monitor keeps its dedicated view-only host operation. Video goes to the monitor; the device remains a touchpad with its keyboard toolbar, game buttons and stream menu. This works with any Moonlight-compatible host and does not ask the host to change its display layout. Android determines which attached outputs support presentations; mirroring-only outputs cannot be selected.

Select **This device** to return. Removing the external display triggers the same client reconnect back to the phone/tablet. A display that disappears before launch falls back to the device. Output selection is session-local and is preserved through reconnects, not saved as a global preference. PiP auto-entry is disabled while presenting externally. Locking/leaving the activity follows the normal stream lifecycle; no background input or unattended reconnect is introduced.

Android 0.4.9 tracks changes to the selected video display's maximum refresh rate. On hosts (currently Rubylight) and decoders supporting [in-place reconfiguration](recovery-and-quality.md#choosing-automatic-quality), streams that follow the display rate can update their FPS target and automatic-quality ceiling without reconnecting, even when the phone's refresh rate is unchanged. Returning to **This device** restores phone/tablet display tracking, including native-resolution changes. Received and shown FPS in the overlay remain the measurements of delivered video.

For external MediaCodec video, **Balanced** pacing uses the lowest-latency path for this session, with a notice when it changes. Android's public Choreographer callbacks follow the internal display, so using them to pace an external surface can hold video to the phone's cadence. The saved pacing preference is unchanged; other pacing modes keep their behavior. PyroWave keeps its own Vulkan presentation. External output does not send phase-lock timing derived from the phone display, and this fallback does not claim a measured FPS or VRR result.

The JSON validator and its version 1 fixtures, the Artemis import mapping, stick deadzone, floating centering, WASD directions, grid snapping, release of held gamepad input, held-input ownership, axis/hat report remapping, zoom bounds and inverse touch mapping have headless unit coverage. Actual monitor attach/detach, OEM SurfaceView transforms, controller hardware and accessibility services still require device testing; no physical display, audio or controller is used by these tests.

### Touch mapping and video scaling

Every [video scaling](video-and-latency.md#video-scaling) mode maps touch and mouse input to the stream pixel under your finger. Direct mouse, Multi-touch, a pen and an absolute mouse go through the video's rectangle and then pinch zoom, so a tap lands on what you see in Fit, Fill, Stretch and 1:1 alike. A touch on a border beside a Fit or 1:1 picture goes to the nearest edge of the stream. In Fill, the cropped edges are still part of the stream: Trackpad and Local cursor can move the pointer there, and the drawn pointer then leaves the screen until you move it back. Trackpad speed follows the picture's size on screen, so the pointer keeps pace with your finger in every mode. On-screen controls and per-game touch buttons stay where you placed them; they belong to the screen, not the video. When the screen turns ([orientation](video-and-latency.md#orientation)), touch buttons keep their place as a share of the screen's width and height, the on-screen gamepad is laid out again, and input maps through the new picture rectangle.

## Input-only on any PC

Input-only turns the phone into a controller, keyboard or touchpad for a game shown elsewhere, for example on the PC's own screen or on another device's stream. Rubylight and Vibepollo list an Input-only tile, and Apollo lists its Remote Input app while its input-only mode is on; those start no video or audio at all.

Other hosts (Sunshine, GameStream, Apollo with its input-only mode off) have no such mode. Turn on **Settings → Streaming → Input-only on any PC** (off by default), then long-press a game in the library and choose **Input-only**. Nightjar then starts an ordinary stream at the smallest size every host accepts and discards it on the phone:

- The PC still encodes and sends video at 640x360, 30 FPS and 500 kbps, plus stereo audio: about 0.7 Mbit/s with error correction, or roughly 310 MB an hour. The row in the game's menu and the stream screen say so.
- The phone does not decode or show the video (no decoder starts) and does not play the audio. Its video and audio still arrive, so the host sees a healthy client.
- The stream asks the PC to keep its own sound and its display mode: no optimal game settings, virtual display, HDR or VRR.
- The row appears only for the running game, or for any game while the PC runs none. It never quits a running game; if another game started meanwhile, the stream stops with a message instead.
- The row is disabled, with the reason, until this device is paired and the host allows it to view streams and send input.
- If the host refuses the stream, the error says so. A host that allows only one stream at a time refuses it while another device streams: end that stream or raise the host's limit on simultaneous streams.
- Hosts that set their display to the client's resolution in their own display settings may switch to 640x360; turn that off on the host for Input-only.

The stream menu and Back behave as for the host's own Input-only. Disconnect leaves the game running.

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
| Ctrl+Alt+Shift+K | Show or hide the PC keyboard. |
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
| Three-finger tap in Trackpad, Direct mouse or Local cursor mode | Toggle the system keyboard (or the PC keyboard when set so), or middle-click when **Three-finger tap** is set to Middle click. |
| Stream menu → Overlay (top row) | Show or hide the overlay. **Display → Overlay and diagnostics** chooses Compact, Statistics or Debug. |

Nightjar can also pin app shortcuts to the Android launcher. For external game libraries and intent-based shortcuts, see [frontends](FRONTENDS.md).
