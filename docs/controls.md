# Controls

[Documentation](index.md) · [All defaults](settings.md)

Choose controls for the game you want to stream: a gamepad, touch, mouse and keyboard, or the on-screen gamepad. The Rubylight host must grant the corresponding input permissions to this device; see [host permissions](pairing.md#per-device-permissions).

## Gamepads

Connect a Bluetooth controller in Android's Bluetooth settings or plug a USB controller into the device. **Settings → Controls → Detect controllers** is on by default and supports multiple players; the protocol provides up to 16 controller slots, while games may accept fewer.

Use **Controller buttons** to press a digital button and choose what it sends, including **Nothing**. Mappings are stored per controller model and apply on the next stream, including the USB drivers. **Reset this controller** restores its default mapping. Analog axes are not remapped by this screen.

**Flip face buttons** swaps A/B and X/Y. Raise the stick deadzone if a stationary stick drifts. **Vibrate device for rumble** supplies phone/tablet feedback when the controller cannot rumble.

### DualSense and USB

Bluetooth DualSense uses Android's controller path. Controller motion, lights, battery and vibration extensions depend on Android and controller support; motion settings are available on Android 12+, with motion defaulting off on API 31 because of an OS bug.

For direct DualSense or DualSense Edge USB support:

1. Enable **Settings → Advanced → USB DualSense driver** and accept its explanation.
2. Connect the pad by USB and start a stream.
3. Allow Android's USB access request. Rubylight can then send host-provided adaptive trigger effects, rumble and lightbar changes and read the pad's input and Edge paddles.

The toggle is off by default. Denying USB access keeps the ordinary Android input path; unplug and reconnect to retry. The host must support the requested feedback. Bluetooth adaptive triggers and USB audio haptics are not implemented by this driver.

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

**Trackpad speed** changes relative touch movement from 25–400%, with 100% as the default. Trackpad and Direct mouse need **Mouse input** permission; Multi-touch needs **Touch input**, and its mouse fallback needs **Mouse input**. Native pen input needs **Pen input** and a supporting host.

## Mouse and keyboard

USB and Bluetooth mice and keyboards use Android's input support. Android 8+ provides native pointer capture. **Mouse speed** scales relative movement from 25–400% (default 100%).

Leave **Desktop mouse mode** off for games that need relative mouse look; enable it for desktop pointer behavior. Enable **Mouse back/forward buttons** only when needed; some devices report right-click through the same Android key path.

Open the stream menu and select **Keyboard** for Android's on-screen keyboard. Hardware key events and text input are sent to the host; Android may reserve system keys. Check the host keyboard layout if punctuation or language-specific keys differ.

## On-screen controls

Turn on **Settings → Controls → Show on-screen controls**, or use **stream menu → Input mode**. The virtual gamepad supports opacity, press vibration, a Guide button and an L3/R3-only layout.

During a stream choose **Input mode → Edit controller layout**. Select **Move controls** or **Resize controls**, drag the controls, then choose **Save layout and play**. **Reset controls layout** in Settings restores their sizes and positions.

## Shortcuts

### App navigation

Outside a stream, use D-pad/Tab to navigate and Enter/A to select. Long-press Select or use Menu/Shift+F10 for PC and app actions. **Ctrl+N** adds a PC, **Ctrl+,** opens Settings, **Ctrl+F** searches Settings, **F1** opens Help, and **Esc/B** goes back.

In Settings, Right opens the selected switch's help (Left in RTL). TV settings put Controls first; choose **Map a button** before capturing a controller mapping. Settings search includes every group, including Advanced; preset chips show the matching preset or Custom.

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
| Hold the performance overlay | Switch Compact/Advanced mode. |

Rubylight can also pin app shortcuts to the Android launcher. For external game libraries and intent-based shortcuts, see [frontends](frontends.md).
