# Gamepad navigation

[Documentation](index.md) · [Controls](controls.md)

Nightjar's menus and its on-screen PC keyboard work with a controller alone. This page covers USB and Bluetooth controllers on Android: the controls, and how Nightjar handles the different ways Android reports controller input. For in-game controls, see [controls](controls.md).

## Controls

| Input | In menus | In the PC keyboard |
| --- | --- | --- |
| D-pad, hat or left stick | Move the focus | Move between keys; holding repeats the move (after 350 ms, then every 100 ms) |
| A | Activate | Press the focused key; holding repeats it |
| B, or the controller's Back | Go back or cancel | Close the keyboard |
| X | Options for the selected PC or game | Backspace |
| Y | | Space |
| Start or Menu | | Enter |
| LB / RB | | Shift / Ctrl |

Other buttons (Select, Guide, stick clicks, digital trigger keys, C and Z) do nothing in the keyboard and send nothing to the PC. **Flip face buttons** and the controller's button mapping apply here as they do in games. The button that closes the keyboard does not reach the PC, including its repeats and release.

To open the PC keyboard with only a controller, press **Start+Back+LB+RB** to open the stream menu and select **PC keyboard** in its top row. (**Ctrl+Alt+Shift+K** needs a physical keyboard, and the finger-tap gestures need a touch screen.)

Only one controller steers the keyboard at a time: a push on another controller takes over, and a second controller resting at center does not cancel a direction held on the first. Unplugging a controller, closing the keyboard or losing input stops any held direction.

On Nintendo-style controllers, the south button (labelled B) confirms, because Android reports buttons by position. Turn on **Settings → Controls → Flip face buttons** to make the button labelled A confirm.

## Behavior details

- A diagonal on the hat selects the vertical direction, so a diagonal never moves two keys at once.
- After touch input, the first controller press only shows the focus ring.
- A DualSense or DualShock touchpad that Android exposes as a mouse moves the PC's pointer while the keyboard is open, as it does in a game.

## How Android reports controllers

Android delivers controller input in several overlapping forms, and Nightjar normalizes them:

- **D-pad keys and hat axes.** A controller reports its D-pad as `KEYCODE_DPAD_*` key events, as a hat (`AXIS_HAT_X` and `AXIS_HAT_Y`), or both; which depends on its Linux driver and the key layout file Android selects. Android's guidance is to treat both as the same input.
- **Duplicate presses.** Some layouts declare both. The AOSP layout for the Switch Pro Controller (`Vendor_057e_Product_2009.kl`), for example, declares D-pad keys and hat axes. In addition, for any joystick event that no view handles, Android synthesizes `DPAD_*` key events with `FLAG_FALLBACK` from the hat and the left stick, and repeats them itself. One press can therefore arrive twice.
- **Sources.** `SOURCE_GAMEPAD` carries face buttons (`KEYCODE_BUTTON_*`), `SOURCE_JOYSTICK` analog axes and `SOURCE_DPAD` D-pad keys. Some generic HID controllers report only a joystick source, so Nightjar accepts either, and always checks the source, since other device types reuse the same axis ids.
- **Face buttons by position.** `BUTTON_A` is the south button, `BUTTON_B` east, `BUTTON_X` west and `BUTTON_Y` north, whatever is printed on them. Xbox and PlayStation controllers follow this order (Cross is `BUTTON_A`). The layout of 8BitDo and generic controllers depends on their mode and the layout file Android selects.
- **TV conventions.** Back is `KEYCODE_BUTTON_B` or `KEYCODE_BACK`; selection is `BUTTON_A`, `BUTTON_SELECT`, Enter or `DPAD_CENTER`.
- **System buttons.** Xbox View and PlayStation Share arrive as `BUTTON_SELECT`, Menu and Options as `BUTTON_START` or `KEYCODE_MENU`, Guide and PS as `BUTTON_MODE`. A DualShock 4 touchpad click can arrive as `BUTTON_SELECT` with scan code 317, which Nightjar remaps for games. Buttons without a function in the menus or keyboard are consumed and never reach the game.

Nightjar's handling:

- **Menus and dialogs.** When a D-pad press arrives both as a key and as its fallback copy, the first copy owns the press until it is released; the other copy, its repeats and its release are dropped, so the focus moves once. Controllers that send one form, TV remotes and keyboards are unaffected. Held keys are cleared when the screen pauses or loses focus.
- **PC keyboard.** D-pad keys, the hat and the left stick map to one direction state, so they cannot move twice. The keyboard generates its own repeats and ignores the system's.

## References

- [Handle controller actions](https://developer.android.com/develop/ui/views/touch-and-input/game-controllers/controller-input): input sources, hat axes and D-pad keys as the same input, `BUTTON_A` as the south button.
- [Game controllers on TV](https://developer.android.com/training/tv/start/controllers): Back and selection buttons.
- [`ViewRootImpl.SyntheticJoystickHandler`](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/view/ViewRootImpl.java): the `FLAG_FALLBACK` D-pad keys synthesized from hat and stick axes.
- [Key layout files](https://source.android.com/docs/core/interaction/input/key-layout-files), [`Generic.kl`](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/data/keyboards/Generic.kl), [`Vendor_057e_Product_2009.kl`](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/data/keyboards/Vendor_057e_Product_2009.kl) (Switch Pro) and [`Vendor_054c_Product_0ce6.kl`](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/data/keyboards/Vendor_054c_Product_0ce6.kl) (DualSense).

If a controller does not behave as described, [open an issue](https://github.com/RamazanKara/nightjar/issues) with its name and what happened.
