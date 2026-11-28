package com.limelight.binding.input;

import android.os.Build;
import android.view.InputDevice;
import android.view.KeyEvent;

import com.limelight.Game;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.NightjarInputJni;

/**
 * Handles Android {@link KeyEvent} button presses and releases, quirk remapping, handheld
 * Back-as-Guide handling, Guide-hold stream-menu gestures, and quit combos for
 * {@link ControllerHandler}.
 */
final class ControllerButtonRouter {
    private static final int REMAP_CONSUME = -2;

    private final ControllerHandler handler;
    private final int[] remapOut = new int[1];
    private final int[] buttonStepOut = new int[5];
    private final int[] comboOut = new int[2];

    ControllerButtonRouter(ControllerHandler handler) {
        this.handler = handler;
    }

    // Return a valid keycode, -2 to consume, or -1 to not consume the event
    // Device MAY BE NULL
    private int handleRemapping(ControllerHandler.InputDeviceContext context, KeyEvent event) {
        int quirks = 0;
        if (context.ignoreBack) quirks |= NightjarInputJni.QUIRK_IGNORE_BACK;
        if (context.hasShare) quirks |= NightjarInputJni.QUIRK_HAS_SHARE;
        if (context.isDualShockStandaloneTouchpad) quirks |= NightjarInputJni.QUIRK_DS4_STANDALONE_TOUCHPAD;
        if (context.usesLinuxGamepadStandardFaceButtons) quirks |= NightjarInputJni.QUIRK_LINUX_STD_FACE;
        if (context.isNonStandardDualShock4) quirks |= NightjarInputJni.QUIRK_NON_STD_DS4;
        if (context.isServal) quirks |= NightjarInputJni.QUIRK_SERVAL;
        if (context.isNonStandardXboxBtController) quirks |= NightjarInputJni.QUIRK_NON_STD_XBOX_BT;
        if (context.hatXAxis == -1 && context.hatYAxis == -1 && !context.hasHardwareDpadKeys) {
            quirks |= NightjarInputJni.QUIRK_NO_HAT;
        }
        if (context.backIsStart) quirks |= NightjarInputJni.QUIRK_BACK_IS_START;
        if (context.modeIsSelect) quirks |= NightjarInputJni.QUIRK_MODE_IS_SELECT;
        if (context.searchIsMode) quirks |= NightjarInputJni.QUIRK_SEARCH_IS_MODE;

        int mapped = NightjarInputJni.remapKeyEvent(
                handler.prefConfig.backAsGuide, event.getKeyCode(), event.getScanCode(),
                event.getFlags(), event.hasNoModifiers(), context.vendorId, context.productId,
                Build.VERSION.SDK_INT, quirks, remapOut);
        if ((remapOut[0] & 1) != 0) {
            context.backIsStart = false;
        }
        if ((remapOut[0] & 2) != 0) {
            context.modeIsSelect = false;
        }
        return mapped;
    }

    boolean handleHandheldBackAsGuide(KeyEvent event) {
        if (!ControllerHandler.isBackAsGuide(handler.prefConfig.backAsGuide, event.getKeyCode(), event.getFlags())) {
            return false;
        }
        InputDevice dev = event.getDevice();
        boolean virtualDevice = dev == null || dev.isVirtual();
        if (!ControllerHandler.isHandheldSystemBack(event.getKeyCode(), event.getFlags(), event.getSource(), virtualDevice,
                !virtualDevice && ControllerDeviceClassifier.isExternal(dev),
                !virtualDevice && dev.getKeyboardType() == InputDevice.KEYBOARD_TYPE_ALPHABETIC,
                !virtualDevice && (ControllerDeviceClassifier.hasGamepadButtons(dev) || ControllerDeviceClassifier.hasJoystickAxes(dev)),
                handler.classifier.backIsSpareButton())) {
            return false;
        }
        if (handler.stopped) {
            return true;
        }

        // The built-in pad's own Guide: with the on-screen controls on controller 1, its number, not theirs
        ControllerHandler.InputDeviceContext context = handler.slots.handheldContext();
        if (event.getAction() == KeyEvent.ACTION_DOWN) {
            boolean repeat = event.getRepeatCount() > 0;
            if (!repeat) {
                endGuideTap(context);
                // No key event to replay: a chord sends the flag itself (sendHeldGuideOn)
                context.guideHeldDown = null;
            }
            if (holdGuide(context, repeat)) {
                return true;
            }
            context.inputMap |= ControllerPacket.SPECIAL_BUTTON_FLAG;
        }
        else if (event.getAction() == KeyEvent.ACTION_UP) {
            GuideHold.Release release = releaseGuide(context);
            if (release == GuideHold.Release.TAP) {
                tapDefaultGuide(context);
            }
            if (release != GuideHold.Release.PASS) {
                return true;
            }
            // Firmware injects the press and release together; hold Guide long enough for games to see it
            long remaining = NightjarInputJni.tapHoldRemainingMs(event.getDownTime(), event.getEventTime());
            if (remaining > 0) {
                scheduleDefaultGuideRelease(context, remaining);
                return true;
            }
            context.inputMap &= ~ControllerPacket.SPECIAL_BUTTON_FLAG;
        }
        handler.sendControllerInputPacket(context);
        return true;
    }

    boolean handleButtonUp(KeyEvent event) {
        ControllerHandler.InputDeviceContext context = handler.getContextForEvent(event);
        if (context == null) {
            return true;
        }

        // User mapping takes precedence over device quirk remapping and Guide hold so remappable
        // extra buttons (e.g. 8BitDo BTN_C/BTN_Z/L4/R4) cannot be forced to Guide.
        int source = ControllerButtonMap.sourceForKey(event.getKeyCode(), event.getScanCode());
        int userTarget = context.buttonMap.map(source);
        int keyCode;
        if (userTarget == ControllerButtonMap.DISABLED) {
            context.quitButtons &= ~ControllerHandler.quitButton(event.getKeyCode());
            releaseQuitCombo(context, context.quitButtons);
            return true;
        } else if (userTarget != ControllerButtonMap.UNMAPPED) {
            context.quitButtons &= ~ControllerHandler.quitButton(event.getKeyCode());
            releaseQuitCombo(context, context.quitButtons);
            keyCode = userTarget;
        } else {
            keyCode = handleRemapping(context, event);
            if (keyCode < 0) {
                return (keyCode == REMAP_CONSUME);
            }
            context.quitButtons &= ~ControllerHandler.quitButton(keyCode);
            releaseQuitCombo(context, context.quitButtons);
            if (handler.prefConfig.flipFaceButtons) {
                keyCode = NightjarInputJni.flipFaceButtonKeyCode(keyCode);
            }
        }

        if (keyCode == KeyEvent.KEYCODE_BUTTON_MODE && !context.guideTapping) {
            GuideHold.Release release = releaseGuide(context);
            if (release == GuideHold.Release.TAP) {
                tapGuide(context, event);
            }
            if (release != GuideHold.Release.PASS) {
                return true;
            }
        }

        int physicalKey = event.getScanCode() != 0 ? event.getScanCode() : ~event.getKeyCode();
        if (!ControllerHandler.updateMappedButton(context.pressedButtons, physicalKey, keyCode, false)) {
            return true;
        }

        int finalKeyCode = keyCode;
        long eventTime = event.getEventTime();
        int scanCode = event.getScanCode();
        Runnable previous = context.pendingButtonReleases.remove(finalKeyCode);
        if (previous != null) {
            handler.mainThreadHandler.removeCallbacks(previous);
        }
        long remaining = NightjarInputJni.tapHoldRemainingMs(event.getDownTime(), eventTime);
        if (remaining > 0 && NightjarInputJni.isHandledGamepadKey(finalKeyCode, scanCode, context.hasPaddles)) {
            Runnable release = new Runnable() {
                @Override
                public void run() {
                    if (context.pendingButtonReleases.get(finalKeyCode) == this) {
                        context.pendingButtonReleases.remove(finalKeyCode);
                    }
                    if (context.destroyed || handler.stopped) {
                        return;
                    }
                    finishButtonUp(context, finalKeyCode, eventTime, scanCode);
                }
            };
            context.pendingButtonReleases.put(finalKeyCode, release);
            handler.mainThreadHandler.postDelayed(release, remaining);
            return true;
        }

        return finishButtonUp(context, finalKeyCode, eventTime, scanCode);
    }

    private boolean finishButtonUp(ControllerHandler.InputDeviceContext context, int keyCode, long eventTime, int scanCode) {
        NightjarInputJni.buttonUpStep(
                context.inputMap, context.leftTrigger, context.rightTrigger,
                context.hasPaddles, context.hatXAxisUsed, context.hatYAxisUsed,
                context.leftTriggerAxisUsed, context.rightTriggerAxisUsed,
                keyCode, scanCode, buttonStepOut);
        int status = buttonStepOut[0];
        if (status == 1) {
            return true;
        }
        if (status == 2) {
            return false;
        }
        int stepFlags = buttonStepOut[4];
        if ((stepFlags & 1) != 0 &&
                NightjarInputJni.startLongPressTogglesMouse(
                        context.startDownTime, eventTime, handler.prefConfig.mouseEmulation)) {
            context.toggleMouseEmulation();
        }
        context.inputMap = buttonStepOut[1];
        context.leftTrigger = (byte) buttonStepOut[2];
        context.rightTrigger = (byte) buttonStepOut[3];
        if ((stepFlags & 2) != 0) {
            context.lastLbUpTime = eventTime;
        }
        if ((stepFlags & 4) != 0) {
            context.lastRbUpTime = eventTime;
        }

        if ((context.emulatingButtonFlags & (ControllerHandler.EMULATING_SELECT | ControllerHandler.EMULATING_SPECIAL | ControllerHandler.EMULATING_TOUCHPAD)) != 0) {
            boolean hasTargetTouchpad = (context.emulatingButtonFlags & ControllerHandler.EMULATING_TOUCHPAD) != 0 &&
                    context.pressedButtons.containsValue(ControllerButtonMap.TARGET_TOUCHPAD);
            boolean hasSelectHeld = (context.emulatingButtonFlags & ControllerHandler.EMULATING_TOUCHPAD) != 0 &&
                    context.pressedButtons.containsValue(KeyEvent.KEYCODE_BUTTON_SELECT);
            boolean hasLbHeld = (context.emulatingButtonFlags & ControllerHandler.EMULATING_TOUCHPAD) != 0 &&
                    context.pressedButtons.containsValue(KeyEvent.KEYCODE_BUTTON_L1);
            NightjarInputJni.applyEmulatedCombosOnUp(
                    context.inputMap, context.emulatingButtonFlags,
                    hasTargetTouchpad, hasSelectHeld, hasLbHeld, comboOut);
            context.inputMap = comboOut[0];
            context.emulatingButtonFlags = comboOut[1];
        }

        handler.sendControllerInputPacket(context);

        // pressedButtons too: after a clickpad press, a Select or LB still held is not in inputMap.
        if (NightjarInputJni.shouldOpenMenuOnRelease(
                context.pendingExit, context.inputMap, context.pressedButtons.isEmpty())) {
            context.pendingExit = false;
            if (handler.activityContext instanceof Game) {
                ((Game) handler.activityContext).showStreamMenuFromController();
            }
        }

        return true;
    }

    boolean handleButtonDown(KeyEvent event) {
        ControllerHandler.InputDeviceContext context = handler.getContextForEvent(event);
        if (context == null) {
            return true;
        }

        // Check user mapping first so remapped extra buttons bypass device quirk remaps and Guide hold.
        int source = ControllerButtonMap.sourceForKey(event.getKeyCode(), event.getScanCode());
        int userTarget = context.buttonMap.map(source);
        int keyCode;
        if (userTarget == ControllerButtonMap.DISABLED) {
            // Like the desktop's, the quit combo is the physical buttons, even remapped or disabled.
            context.quitButtons |= ControllerHandler.quitButton(event.getKeyCode());
            holdQuitCombo(context, context.quitButtons);
            return true;
        } else if (userTarget != ControllerButtonMap.UNMAPPED) {
            context.quitButtons |= ControllerHandler.quitButton(event.getKeyCode());
            holdQuitCombo(context, context.quitButtons);
            keyCode = userTarget;
        } else {
            keyCode = handleRemapping(context, event);
            if (keyCode < 0) {
                return (keyCode == REMAP_CONSUME);
            }
            context.quitButtons |= ControllerHandler.quitButton(keyCode);
            holdQuitCombo(context, context.quitButtons);
            if (handler.prefConfig.flipFaceButtons) {
                keyCode = NightjarInputJni.flipFaceButtonKeyCode(keyCode);
            }
        }

        // Hold Guide to open the stream menu, on Guide (or a button mapped to Guide, or Back as Guide)
        if (keyCode == KeyEvent.KEYCODE_BUTTON_MODE && !context.guideTapping) {
            boolean repeat = event.getRepeatCount() > 0;
            if (!repeat) endGuideTap(context);
            boolean wasActive = context.guideHold.active();
            if (holdGuide(context, repeat)) {
                if (!(repeat && wasActive)) context.guideHeldDown = new KeyEvent(event);
                context.hasMode = true;
                return true;
            }
        }
        else if (keyCode != KeyEvent.KEYCODE_BUTTON_MODE) {
            // A chord with a waiting Guide sends Guide first
            sendHeldGuide(context);
        }

        Runnable pendingRelease = context.pendingButtonReleases.remove(keyCode);
        if (pendingRelease != null) {
            handler.mainThreadHandler.removeCallbacks(pendingRelease);
            pendingRelease.run();
        }

        int physicalKey = event.getScanCode() != 0 ? event.getScanCode() : ~event.getKeyCode();
        if (!ControllerHandler.updateMappedButton(context.pressedButtons, physicalKey, keyCode, true)) {
            return true;
        }

        NightjarInputJni.buttonDownStep(
                context.inputMap, context.leftTrigger, context.rightTrigger,
                context.hasPaddles, context.hatXAxisUsed, context.hatYAxisUsed,
                context.leftTriggerAxisUsed, context.rightTriggerAxisUsed,
                keyCode, event.getScanCode(), buttonStepOut);
        int status = buttonStepOut[0];
        if (status == 1) {
            return true;
        }
        if (status == 2) {
            return false;
        }
        context.inputMap = buttonStepOut[1];
        context.leftTrigger = (byte) buttonStepOut[2];
        context.rightTrigger = (byte) buttonStepOut[3];
        int stepFlags = buttonStepOut[4];
        if ((stepFlags & 1) != 0) {
            context.hasSelect = true;
        }
        if ((stepFlags & 2) != 0) {
            context.hasMode = true;
        }
        if ((stepFlags & 4) != 0 && event.getRepeatCount() == 0) {
            context.startDownTime = event.getEventTime();
        }

        // Wait for the menu combo to lift so it cannot activate a menu item. It goes by the
        // held buttons: a clickpad press before Start+RB took Select and LB out of inputMap.
        if (NightjarInputJni.shouldArmMenuCombo(ControllerHandler.heldButtons(context))) {
            context.pendingExit = true;
        }

        NightjarInputJni.applyEmulatedCombosOnDown(
                context.inputMap, context.emulatingButtonFlags,
                context.hasSelect, context.hasMode, context.needsClickpadEmulation,
                event.getEventTime(), context.lastLbUpTime, context.lastRbUpTime, comboOut);
        context.inputMap = comboOut[0];
        context.emulatingButtonFlags = comboOut[1];

        // We don't need to send repeat key down events, but the platform
        // sends us events that claim to be repeats but they're from different
        // devices, so we just send them all and deal with some duplicates.
        handler.sendControllerInputPacket(context);
        return true;
    }

    private boolean holdGuide(ControllerHandler.GenericControllerContext context, boolean repeat) {
        boolean wasActive = context.guideHold.active();
        if (!context.guideHold.down(handler.prefConfig.guideMenuHoldMs, repeat)) {
            handler.mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
            return false;
        }
        if (!(repeat && wasActive)) {
            handler.mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
            handler.mainThreadHandler.postDelayed(context.guideHoldRunnable, handler.prefConfig.guideMenuHoldMs);
        }
        return true;
    }

    private GuideHold.Release releaseGuide(ControllerHandler.GenericControllerContext context) {
        handler.mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
        return context.guideHold.up();
    }

    private void tapGuide(ControllerHandler.InputDeviceContext context, KeyEvent up) {
        KeyEvent release = new KeyEvent(up);
        replayGuide(context, KeyEvent.changeAction(up, KeyEvent.ACTION_DOWN), true);
        Runnable end = new Runnable() {
            @Override
            public void run() {
                // Only this tap's own release: a newer tap has its own.
                if (context.guideTapEnd != this) return;
                context.guideTapEnd = null;
                if (!handler.stopped) replayGuide(context, release, false);
            }
        };
        context.guideTapEnd = end;
        handler.mainThreadHandler.postDelayed(end, ControllerHandler.MINIMUM_BUTTON_DOWN_TIME_MS);
    }

    private void replayGuide(ControllerHandler.InputDeviceContext context, KeyEvent event, boolean down) {
        context.guideTapping = true;
        try {
            if (down) handler.handleButtonDown(event);
            else handler.handleButtonUp(event);
        } finally {
            context.guideTapping = false;
        }
    }

    private void tapDefaultGuide(ControllerHandler.InputDeviceContext context) {
        context.inputMap |= ControllerPacket.SPECIAL_BUTTON_FLAG;
        handler.sendControllerInputPacket(context);
        scheduleDefaultGuideRelease(context, ControllerHandler.MINIMUM_BUTTON_DOWN_TIME_MS);
    }

    private void scheduleDefaultGuideRelease(ControllerHandler.InputDeviceContext context, long delayMs) {
        Runnable end = new Runnable() {
            @Override
            public void run() {
                if (context.guideTapEnd != this) return;
                context.guideTapEnd = null;
                if (handler.stopped) return;
                context.inputMap &= ~ControllerPacket.SPECIAL_BUTTON_FLAG;
                handler.sendControllerInputPacket(context);
            }
        };
        context.guideTapEnd = end;
        handler.mainThreadHandler.postDelayed(end, delayMs);
    }

    private void endGuideTap(ControllerHandler.GenericControllerContext context) {
        Runnable end = context.guideTapEnd;
        if (end != null) {
            handler.mainThreadHandler.removeCallbacks(end);
            end.run();
        }
    }

    void sendHeldGuide(ControllerHandler.InputDeviceContext context) {
        sendHeldGuideOn(context);
        if (context != handler.handheldContext) sendHeldGuideOn(handler.handheldContext);
    }

    private void sendHeldGuideOn(ControllerHandler.InputDeviceContext context) {
        if (!context.guideHold.chord()) return;
        handler.mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
        KeyEvent down = context.guideHeldDown;
        context.guideHeldDown = null;
        if (down != null) {
            replayGuide(context, down, true);
        }
        else {
            // A handheld's system Back as Guide, which has no mapping
            context.inputMap |= ControllerPacket.SPECIAL_BUTTON_FLAG;
            handler.sendControllerInputPacket(context);
        }
    }

    void holdQuitCombo(ControllerHandler.GenericControllerContext context, int buttons) {
        context.quitComboPending = NightjarInputJni.holdQuitComboStep(
                context.quitComboPending, buttons, handler.prefConfig.gamepadQuitCombo);
    }

    // Like the menu combo, the stream ends once every button is up, so none stays held on the PC.
    void releaseQuitCombo(ControllerHandler.GenericControllerContext context, int buttons) {
        int res = NightjarInputJni.releaseQuitComboStep(context.quitComboPending, buttons);
        context.quitComboPending = (res & 1) != 0;
        if ((res & 2) != 0 && handler.activityContext instanceof Game) {
            Game game = (Game) handler.activityContext;
            if (android.os.Looper.myLooper() == android.os.Looper.getMainLooper()) {
                game.leaveStreamFromController();
            } else {
                handler.mainThreadHandler.post(game::leaveStreamFromController);
            }
        }
    }
}
