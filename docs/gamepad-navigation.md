# USB and Bluetooth pads in the menus and the PC keyboard

[Documentation](index.md) · [Controls](controls.md)

**Status: Experimental.** Nightjar's own test devices are a Retroid Pocket 6 (built-in controls) and a TV remote. Nobody has plugged a USB pad or paired a Bluetooth pad against this code. What follows is researched from Android's documentation and source, written to match it, and covered by unit tests that feed synthesized key and motion events with gamepad sources. It is not "Tested". If a pad misbehaves, please [open an issue](https://github.com/RamazanKara/nightjar/issues) with the pad's name and what happened.

## What Android delivers

- **Two ways to say "D-pad".** A pad reports its D-pad as `KEYCODE_DPAD_*` key events, as a hat (`AXIS_HAT_X` -1 left / +1 right, `AXIS_HAT_Y` -1 up / +1 down), or both. Android's guidance is to treat them as the same input. Which one a pad uses depends on its Linux driver and the key layout file Android picked for it.
- **Both at once happens.** The AOSP layout for the Switch Pro Controller (`Vendor_057e_Product_2009.kl`) declares `DPAD_UP` to `DPAD_RIGHT` keys *and* `HAT_X`/`HAT_Y` axes. For any joystick event no view handles, Android's `ViewRootImpl` also turns the hat and the left stick (past +-0.5) into `DPAD_*` key events with `FLAG_FALLBACK`, and repeats them itself. A pad with real D-pad keys and a hat can therefore deliver two copies of one press.
- **Sources.** `SOURCE_GAMEPAD` means face buttons (`KEYCODE_BUTTON_*`), `SOURCE_JOYSTICK` means analog axes, `SOURCE_DPAD` means D-pad keys. Some generic HID pads advertise only a joystick source, so Nightjar accepts either. Source and axis ids are reused by other device types, so the source is always checked.
- **Face buttons are by position.** `KEYCODE_BUTTON_A` is the south button, `BUTTON_B` east, `BUTTON_X` west, `BUTTON_Y` north, whatever is printed on them. Xbox and PlayStation pads follow the Xbox order (Cross is `BUTTON_A`); 8BitDo and generic HID pads depend on the mode they are in and the layout Android picks, which we could not check. Nintendo pads print A and B the other way round; the AOSP Switch Pro layout comments say the button labelled "B" produces `BUTTON_A`. So on a Nintendo-style pad the south button (labelled B) confirms. **Flip face buttons** in Settings → Controls swaps this for users who want the printed A to confirm.
- **Android TV rules.** Back is `KEYCODE_BUTTON_B` or `KEYCODE_BACK`, selection is `KEYCODE_BUTTON_A`, `KEYCODE_BUTTON_SELECT`, Enter or `DPAD_CENTER` (Android's TV documentation).
- **System buttons.** Xbox View and DualShock/DualSense Share arrive as `BUTTON_SELECT` (the DualSense layout maps Share to Select and Options to Start), Menu/Options as `BUTTON_START` or `KEYCODE_MENU`, Guide/PS as `BUTTON_MODE`. A DualShock 4 touchpad click can arrive as `BUTTON_SELECT` with scan code 317 (Nightjar already remaps that for games). We found no authoritative source for how Android 13 and later expose the Xbox Series Share button or the DualSense touchpad, so those two are assumptions: the touchpad is a separate device that behaves like a mouse, and a button with no job here is swallowed so it never reaches the game.

## What Nightjar does with it

- **Menus and the console shell** (`NightjarActivity` screens, the action sheets such as the stream menu, and `NavigationDialogBuilder` dialogs): a D-pad press that arrives both as a real key and as the hat's `FLAG_FALLBACK` copy moves the focus once. The first copy to arrive owns the key until it is released; the other copy, its repeats and its release are dropped. A pad that sends only one kind, a TV remote and a keyboard are unaffected. Held keys are forgotten when the screen pauses or its window loses focus (a dialog has no such hook, but lives only briefly).
- **PC keyboard.** The D-pad keys, the hat and the left stick all name the same direction, so they cannot double-move. The keyboard repeats the move itself (first after 350 ms, then every 100 ms) and ignores the system's key repeats. Only one pad steers at a time (USB-driver pads included): a second pad resting at centre does not cancel a direction held on the first, and a push on the second takes over. Unplugging a pad, cancelling the motion stream, closing the keyboard or losing input stops a held hat or stick; nothing keeps walking.
- **Buttons.** A confirms (hold repeats), B or a pad's own Back (or a button mapped to Back) closes the keyboard, and the repeats and release of the button that closed it never reach the PC, X is Backspace, Y is Space, Start/Menu is Enter, LB and RB tap Shift and Ctrl. Everything else on a pad (Select, Mode, stick clicks, the digital trigger keys, C and Z) is swallowed and sends nothing. Flip face buttons and the per-controller button map apply as in games.
- **Opening the keyboard from a pad.** The pad's menu chord (Start+Back+LB+RB) opens the stream menu; **PC keyboard** is in its top row. Ctrl+Alt+Shift+K needs a physical keyboard and the three- or four-finger tap needs a touch screen, so on a pad-only setup the menu is the way.

## Known limits

- A diagonal hat (both axes at full) picks the vertical direction, so a pad's diagonal never moves two keys at once.
- The first pad press after touch only shows the focus ring, by design.
- The DualSense or DualShock touchpad, if Android exposes it as a mouse, moves the PC's pointer while the keyboard is open, as it does in a game.

## Sources

- [Handle controller actions](https://developer.android.com/develop/ui/views/touch-and-input/game-controllers/controller-input): sources, hat axes against D-pad keys ("treat the hat axis events and the D-pad key codes as the same input events"), `BUTTON_A` is the south button even when labelled B.
- [Game controllers on TV](https://developer.android.com/training/tv/start/controllers): Back and selection buttons.
- [`ViewRootImpl.SyntheticJoystickHandler`](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/view/ViewRootImpl.java): the `FLAG_FALLBACK` D-pad keys made from hat and stick axes.
- [Key layout files](https://source.android.com/docs/core/interaction/input/key-layout-files), [`Generic.kl`](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/data/keyboards/Generic.kl), [`Vendor_057e_Product_2009.kl`](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/data/keyboards/Vendor_057e_Product_2009.kl) (Switch Pro) and [`Vendor_054c_Product_0ce6.kl`](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/data/keyboards/Vendor_054c_Product_0ce6.kl) (DualSense).

Tests: `GamePadKeyboardNavigationTest`, `DpadEchoFilterTest` and `PcKeyboardControllerTest`.
