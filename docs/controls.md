# Controls

[Documentation](index.md) · [All defaults](settings.md)

The Rubylight host must grant the relevant input permission to this device. If video works but controls do not, check [host permissions](pairing.md#per-device-permissions) first.

## Gamepads

Connect a Bluetooth controller in Android's Bluetooth settings or plug a USB controller into the device. **Settings → Controls → Detect controllers** is on by default and supports multiple players; the protocol provides up to 16 controller slots.

Use **Controller buttons** to press a digital button and choose what it sends, including **Nothing**. Mappings are stored per controller model and apply on the next stream, including the USB drivers. **Reset this controller** restores its default mapping.

**Flip face buttons** swaps A/B and X/Y. Raise the stick deadzone if a stationary stick drifts. **Vibrate device for rumble** gives you rumble on the phone or tablet when the controller has no motors.

### DualSense and USB

Bluetooth DualSense uses Android's controller path. Controller motion, lights, battery and vibration extensions depend on Android and controller support; motion settings are available on Android 12+, with motion defaulting off on API 31 because of an OS bug.

For direct DualSense or DualSense Edge USB support:

1. Enable **Settings → Advanced → USB DualSense driver** and accept its explanation.
2. Connect the pad by USB and start a stream.
3. Allow Android's USB access request. Rubylight can then send host-provided adaptive trigger effects, rumble and lightbar changes and read the pad's input and Edge paddles.

The toggle is off by default. Denying USB access keeps the ordinary Android input path; unplug and reconnect to retry. The host must support the requested feedback.

For native waveform vibration from a compatible **Vibepollo nightly** host, also enable **USB DualSense waveform haptics**, then reconnect the controller and stream. This experimental option sends the host's distinct left/right samples to a USB DualSense or Edge. It takes over the controller's USB audio interfaces, so its headphone and microphone functions are unavailable while enabled. Use the phone or another audio device for game sound. Bluetooth waveform output is not supported by Android's standard controller API. See [waveform setup and limitations](dualsense-haptics.md).

The separate **USB controller driver** setting supplies Xbox USB support. **Always use the USB driver** prefers that path even if Android already recognizes the controller.

### Controller mouse mode

With **Controller mouse mode** enabled, hold **Start** to toggle it. By default the left stick moves the pointer, the right stick scrolls, **A** left-clicks and **B** right-clicks. **Scroll with an analog stick** can swap the sticks or make both move the pointer.

## Touch modes

Choose **Settings → Controls → Touch mode**, or **stream menu → Input mode → Touch mode**.

| Mode | Gestures and behavior |
| --- | --- |
| Trackpad (default) | Drag to move the pointer relatively. Tap with one finger for left-click, two for right-click. Hold then drag to drag with a mouse button; move two fingers to scroll. |
| Direct mouse | Tap a location to click there, drag to move while holding the left button, and hold still for right-click. A second finger scrolls. |
| Multi-touch | Send fingers directly to the host touch interface. If native touch is unavailable, Rubylight uses Direct mouse. Use the stream menu for the keyboard. |

**Trackpad speed** changes relative touch movement from 25–400%, with 100% as the default. Native pen input is forwarded when supported and permitted by the host.

Direct mouse, native touch and pen positions follow the streamed image, including when cutouts, fold panes or window padding shift it. Touch permission and a working host touch interface are still required for Multi-touch.

## Mouse and keyboard

USB and Bluetooth mice and keyboards use Android's input support. Android 8+ provides native pointer capture. **Mouse speed** scales relative movement from 25–400% (default 100%).

Leave **Desktop mouse mode** off for games that need relative mouse look; enable it for desktop pointer behavior. Enable **Mouse back/forward buttons** only when needed; some devices report right-click through the same Android key path.

Open the stream menu and select **Keyboard** for Android's on-screen keyboard. Hardware key events and text input are sent to the host; Android may reserve system keys. Check the host keyboard layout if punctuation or language-specific keys differ.

Select **stream menu → Extra keyboard keys** for a toolbar with Esc, Tab, Enter, Backspace, Delete, Home/End, Page Up/Down, arrows and F1–F12. Scroll each row horizontally to reach more keys. Ctrl, Alt, Shift and Win apply to the next key pressed in this toolbar, then reset. **Hide keys** closes the toolbar; **Keyboard** opens Android's text keyboard.

The toolbar also provides **Task Manager** (Ctrl+Shift+Esc), **Switch window** (Alt+Tab) and **Close window** (Alt+F4). To stop an unresponsive app, open Task Manager and use its **End task** action on the host. These controls require the host's keyboard permission.

## PC display mode and administration

Open your PC's menu → **Stream settings → PC display mode**, or choose **PC display mode** directly from its menu. The choice is saved per PC and applies from the next stream.

| Choice | What happens on the PC |
| --- | --- |
| The PC's own setting | Follow Host virtual display and the host's display settings. |
| Exclusive | Use a virtual display and switch the PC's other monitors off during the stream. |
| Extended | Keep the other monitors on; choose whether the PC display or virtual display is primary. |
| Extended and separated | Keep the other monitors on, but place the streamed display far away in the desktop layout. Choose which display is primary. Both displays share the same Windows session. |
| Physical display | Stream the PC's monitor without adding a virtual display. |

Rubylight restores the display layout when the stream ends. These choices apply to normal streaming; Remote Monitor and Input-only retain their host-controlled behavior. Other hosts may accept the virtual-display request without supporting the selected layout.

For an online Rubylight PC, **Open host administration** opens its web console in your browser. Manage applications, paired devices, permissions and host display settings there. The console uses its own login; pairing does not sign you in. Remote access needs a reachable console port, which is separate from the streaming ports. The dialog's address can be selected and copied if no browser is available.

## On-screen controls

Turn on **Settings → Controls → Show on-screen controls**, or use **stream menu → Input mode**. The virtual gamepad supports opacity, press vibration, a Guide button and an L3/R3-only layout.

During a stream choose **Input mode → Edit controller layout**. Select **Move controls** or **Resize controls**, drag the controls, then choose **Save layout and play**. **Reset controls layout** in Settings restores their sizes and positions.

## Shortcuts

### App navigation

Outside a stream, use D-pad/Tab to navigate and Enter/A to select. Long-press Select or use Menu/Shift+F10 for PC and app actions. **Ctrl+N** adds a PC, **Ctrl+,** opens Settings, **Ctrl+F** searches Settings, **F1** opens Help, and **Esc/B** goes back.

In Settings, Right opens the selected switch's help (Left in RTL). TV settings put Controls first; choose **Map a button** before capturing a controller mapping. Settings search includes every group, including Advanced; preset chips show the matching preset, Default before you change anything, or Custom.

### During a stream

| Shortcut | Action |
| --- | --- |
| Android Back / Ctrl+Alt+Shift+M | Open the stream menu. |
| Ctrl+Alt+Shift+S | Show or hide the performance overlay. |
| Ctrl+Alt+Shift+Z | Toggle input capture. |
| Ctrl+Alt+Shift+C | Toggle the local cursor's visibility and recapture input if needed. |
| Ctrl+Alt+Shift+Q | Disconnect the local stream; the PC app stays running. |
| Hold Start on a controller | Toggle controller mouse mode when enabled. |
| Start+Back+LB+RB on an Android-mapped controller | Disconnect after releasing the combination. |
| Start+LB when the controller has no Select/Back button | Send Select/Back. |
| Start+Select when the controller has no Guide button | Send Guide; without Select, use Start+RB. |
| Select+LB when device-motion fallback emulates a touchpad-capable controller | Send a touchpad click. |
| Three-finger tap in Trackpad/Direct mouse mode | Toggle the on-screen keyboard. |
| Stream menu → Overlay | Show or hide the overlay and choose Statistics or Debug. |

Rubylight can also pin app shortcuts to the Android launcher. For external game libraries and intent-based shortcuts, see [frontends](FRONTENDS.md).
