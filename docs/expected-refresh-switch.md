# Refresh rate switching on multi-rate phones

**Status: not yet seen on a multi-rate phone.** Nightjar's own devices are a Retroid Pocket 6, an emulator and a Linux NUC. The Retroid's refresh policy allows one rate at a time (`primaryRefreshRateRange` is [60 60] or pinned at 120), so a screen that switches seamlessly between several rates (a Pixel, a Samsung with Adaptive refresh rate) has never been in front of this code. The votes, the withdrawal and the reasons below are covered by unit tests with fake displays only.

## What the app does

- The stream votes its frame rate on the video surface with `Surface.setFrameRate()`: the base vote at connection (`CHANGE_FRAME_RATE_ALWAYS` on Android 12 and newer), and, with **Match content frame rate** on a Rubylight host, a content-rate vote (`CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS`, withdrawn when the game stops holding a rate). Android 11 has only the two-argument call, which switches seamlessly only.
- `RefreshSwitchMonitor` checks once a second whether the screen follows the last vote. A screen follows when it runs at the vote or a whole multiple of it (30 on 60 or 120, 59.94 against 60). When it has not followed for 3 seconds, the Statistics and debug overlays show **why**, and the log gets one `Refresh switch:` line with the numbers (vote, active Hz, the modes of this screen size, the user limit if known).
- The cause is inferred, because Android reports no reason when it ignores a vote. In order:

| Shown as | Meaning | Based on |
| --- | --- | --- |
| No mode | The screen offers no mode at the vote or a multiple of it at the current resolution. | `Display.getSupportedModes()` |
| Battery saver | The vote is above 60 Hz and battery saver is on. | `PowerManager.isPowerSaveMode()` |
| Hot | The vote is above 60 Hz and the thermal status is critical or worse (Android 10+). | `PowerManager.getCurrentThermalStatus()` |
| User limit | The Android refresh rate setting (Smooth display, Adaptive) allows less than the vote. | `Settings.System` `peak_refresh_rate` (not public API; unreadable means unknown) |
| Not seamless | The vote allowed seamless switches only (the content rate vote), and every matching mode needs a switch that would blank the screen. | `Display.Mode.getAlternativeRefreshRates()` (Android 12+) |
| Needs Match content frame rate | Every matching mode needs a switch that would blank the screen and the vote allowed it (the base vote); Android does that only when the user allows Match content frame rate in the display settings. | the same, plus the kind of vote |
| No reason given | A seamless matching mode exists and nothing readable explains why Android stayed. The rates are listed. | the above |

Before Android 12 no mode lists alternatives, so a blanking switch cannot be told from a seamless one: that case is never blamed and falls to "No reason given".

- A Variable refresh (VRR) request that cannot use Android 16's adaptive refresh rate (ARR) used to be logged only. It now also adds a line to the status: "needs Android 16" or "this screen has no adaptive refresh rate", and the stream runs at the panel's top rate.

## Researched behaviour

- `setFrameRate()` is a hint with no guarantee. A switch that is not seamless needs the user's **Match content frame rate** opt-in and the app's `CHANGE_FRAME_RATE_ALWAYS`. A switch is not seamless when the rates differ and the active mode's alternatives hold no multiple of the vote. `preferredRefreshRate` is ignored for a surface that voted. The platform may refuse, for example in battery saver. [Frame rate guide](https://developer.android.com/media/optimize/performance/frame-rate)
- Android 15 defaults games to 60 Hz; going higher needs an explicit frame rate request, and the system may still cap at 60 Hz for power saving or heat. Nightjar declares `isGame` and the game category. [Display refresh rate changes in games](https://developer.android.com/games/optimize/display-refresh-rate-change)
- In AOSP `DisplayModeDirector` the user's peak refresh rate setting (priority 8 and 9) outranks the app's request (5 to 7), and low power mode (17, 18) and critical skin temperature (20) cap at 60 Hz. Votes are dropped from the lowest priority upwards until a non-empty set of modes remains, and group switching is off by default (`WITHIN_GROUPS`), so a mode in another config group is never taken unless the switching type allows it. [DisplayModeDirector](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/services/core/java/com/android/server/display/mode/DisplayModeDirector.java), [Vote](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/services/core/java/com/android/server/display/mode/Vote.java)
- SurfaceFlinger's `RefreshRateSelector` scores each layer's vote (exact, exact or multiple, and so on) against the allowed ranges and only switches inside the default mode's config group unless group switching is allowed. [RefreshRateSelector.h](https://android.googlesource.com/platform/frameworks/native/+/refs/heads/main/services/surfaceflinger/Scheduler/RefreshRateSelector.h)
- Samsung: Adaptive goes up to 120 Hz, Standard stays at 60 Hz, and 120 Hz availability varies by app and device. [Samsung: how to set the display refresh rate](https://www.samsung.com/ie/support/mobile-devices/how-to-set-the-display-refresh-rate-on-your-galaxy-phone/)
- Pixel: an app `setFrameRate(90)` moved a 60 Hz default device to 90 Hz in the [Redfin (Pixel 5) device change c1bad51](https://android.googlesource.com/device/google/redfin/+/c1bad51), with the Smooth display options still honoured. Pixels also have a Developer options toggle, Disable default frame rate for games ([9to5Google](https://9to5google.com/how-to-disable-default-frame-rates-for-games/)).

Not checked against a source in this pass: the API levels of `getAlternativeRefreshRates()` (Android 12, API 31) and `Display.Mode.getSupportedHdrTypes()` (Android 14, API 34) are from the platform reference as remembered. The `peak_refresh_rate` and `min_refresh_rate` keys are internal and may differ on vendor ROMs.

## Other video and display items not seen on hardware

Looked at, with no code change because nothing can be decided without the hardware: Android TV and Shield boxes (HDMI mode switches are never seamless and take seconds, so a vote may blank the TV), foldables (the mode list changes with the fold state, which the monitor re-reads every second), and tablets with a single 60 Hz mode (the monitor reports "No mode" for any other vote). Android 16 ARR support is the one exception: its fallback reason is now shown (see above).

## What a tester should look for

On a 60/90/120 phone with Adaptive or Smooth display on, stream a 120 fps host with **Match content frame rate** on a Rubylight host and a game that holds 60:

1. The Display line shows the screen moving to 60 (or staying at 120) without a black flash.
2. With the system setting on Standard or battery saver on, the overlay names it after about 3 seconds.
3. Turning the game's frame rate back up withdraws and re-votes without a flash.

Report the phone, Android version, the three refresh settings and the `Refresh switch:` log line.

## Rubylight wording

- Plan, B1: "Refresh-rate mode switch on a multi-rate phone: Nightjar names the reason when the screen does not follow the vote."
- Hardware matrix: "Nightjar Android, seamless refresh switching on 60/90/120 phones | Unit-tested | None | Votes and the reasons are unit-tested with fake displays; the Retroid's policy allows one rate."
- Release notes: "The Statistics overlay says why the screen stays at a different refresh rate than the stream asked for."
