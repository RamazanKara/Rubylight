package com.limelight.binding.input;

import android.os.Build;

import com.limelight.Game;
import com.limelight.LimeLog;
import com.limelight.binding.audio.AndroidAudioRenderer;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarInputJni;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * Routes USB-driver controller reports, pad capture, menu steering, controller audio, waveform
 * haptics, adaptive triggers, RGB LEDs, and attach/detach events for {@link ControllerHandler}.
 */
final class ControllerUsbRouter {
    private final ControllerHandler handler;

    ControllerUsbRouter(ControllerHandler handler) {
        this.handler = handler;
    }

    /** The USB pads that play as this controller, or null once the handler has stopped. */
    List<AbstractController> usbPadsOf(short controllerNumber) {
        synchronized (handler) {
            if (handler.stopped) {
                return null;
            }
            List<AbstractController> pads = Collections.emptyList();
            for (ControllerHandler.UsbDeviceContext deviceContext : handler.usbDeviceContexts.values()) {
                if (deviceContext.assignedControllerNumber && deviceContext.controllerNumber == controllerNumber) {
                    if (pads.isEmpty()) {
                        pads = new ArrayList<>(1);
                    }
                    pads.add(deviceContext.device);
                }
            }
            return pads;
        }
    }

    void handleControllerAudio(int kind, byte[] payload) {
        if (handler.stopped || !handler.conn.canSendControllerInput() || payload == null || payload.length < 4) return;
        short id = (short) ((payload[2] & 255) | ((payload[3] & 255) << 8));
        AbstractController controller = handler.waveformControllers.get(id);
        if (controller != null) controller.controllerAudio(kind, payload);
    }

    // The audio renderer's headset route; it reports itself inactive until a DualSense with USB
    // audio is announced, so ordinary streams skip the volume lookups entirely.
    AndroidAudioRenderer.HeadphoneSink headphoneAudioSink() {
        return new AndroidAudioRenderer.HeadphoneSink() {
            @Override public boolean write(short[] pcm, int length, float volume) {
                return routeHeadphoneAudio(pcm, length, volume);
            }

            @Override public boolean active() {
                return !handler.stopped && !handler.waveformControllers.isEmpty() && handler.conn.canSendControllerInput();
            }
        };
    }

    boolean routeHeadphoneAudio(short[] samples, int length, float volume) {
        if (handler.stopped || !handler.conn.canSendControllerInput()) return false;
        // A single headset owns normal game sound; controller-specific effects remain per-pad.
        for (short id = 0; id < 16; id++) {
            AbstractController controller = handler.waveformControllers.get(id);
            if (controller != null && controller.headphoneAudio(samples, length, volume)) return true;
        }
        return false;
    }

    void handleControllerHaptics(short controllerNumber, int sequence, byte[] pcm) {
        if (handler.stopped || !handler.rumbleAllowed || !handler.conn.canSendControllerInput()) return;
        // Avoid the input monitor and HID transfers on the control receive thread.
        AbstractController controller = handler.waveformControllers.get(controllerNumber);
        if (controller != null) controller.controllerHaptics(sequence, pcm);
    }

    void handleSetAdaptiveTriggers(short controllerNumber, byte eventFlags, byte typeLeft,
                                   byte typeRight, byte[] left, byte[] right) {
        if (handler.stopped) {
            return;
        }
        for (ControllerHandler.UsbDeviceContext context : handler.usbDeviceContexts.values()) {
            if (context.assignedControllerNumber && context.controllerNumber == controllerNumber) {
                context.device.setAdaptiveTriggers(eventFlags, typeLeft, typeRight, left, right);
            }
        }
    }

    void handleSetControllerLED(short controllerNumber, byte r, byte g, byte b) {
        if (handler.stopped) {
            return;
        }

        for (ControllerHandler.UsbDeviceContext context : handler.usbDeviceContexts.values()) {
            if (context.assignedControllerNumber && context.controllerNumber == controllerNumber) {
                context.device.setLed(r, g, b);
            }
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            for (ControllerHandler.InputDeviceContext deviceContext : handler.inputDeviceContexts.values()) {

                // Ignore input devices without an RGB LED
                if (deviceContext.assignedControllerNumber && deviceContext.controllerNumber == controllerNumber && deviceContext.hasRgbLed) {
                    // Convert the RGB components into the integer value that LightState uses
                    deviceContext.ledArgbValue = 0xFF000000 | ((r << 16) & 0xFF0000) | ((g << 8) & 0xFF00) | (b & 0xFF);

                    handler.backgroundThreadHandler.removeCallbacks(deviceContext.setLedStateRunnable);
                    handler.backgroundThreadHandler.post(deviceContext.setLedStateRunnable);
                }
            }
        }
    }

    /**
     * Hold Guide to open the stream menu on a pad the USB driver owns, whose reports carry Guide as
     * a flag: Guide stays out of them while held back, and a short press goes out as a tap. Not while
     * the stream menu or a capture has the pad: there Guide does what it always did.
     */
    private int guideHoldButtons(ControllerHandler.UsbDeviceContext context, int buttonFlags) {
        int actionBits;
        int outButtons;
        synchronized (context) {
            NightjarInputJni.usbGuideHoldStep(
                    context.guideDown, context.guideHold.stateBits(), context.guideTapping,
                    buttonFlags, handler.prefConfig.guideMenuHoldMs, handler.inputEnabled && handler.padCapture == null,
                    context.guideHoldOut);
            context.guideDown = context.guideHoldOut[0] != 0;
            context.guideHold.setStateBits(context.guideHoldOut[1]);
            context.guideTapping = context.guideHoldOut[2] != 0;
            outButtons = context.guideHoldOut[3];
            actionBits = context.guideHoldOut[4];
        }
        if ((actionBits & 2) != 0) {
            handler.mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
        }
        if ((actionBits & 1) != 0) {
            handler.mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
            handler.mainThreadHandler.postDelayed(context.guideHoldRunnable, handler.prefConfig.guideMenuHoldMs);
        }
        return outButtons;
    }

    void reportControllerState(int controllerId, int buttonFlags,
                               float leftStickX, float leftStickY,
                               float rightStickX, float rightStickY,
                               float leftTrigger, float rightTrigger) {
        ControllerHandler.UsbDeviceContext context = handler.usbDeviceContexts.get(controllerId);
        if (context == null) {
            return;
        }
        synchronized (context.usbSendLock) {
            boolean sendCancelPreviousTap = false;
            boolean sendChordGuide = false;
            int chordQuitButtons = 0;
            Runnable canceledTapEnd = null;
            boolean cancelHoldRunnable = false;
            synchronized (context) {
                if (context.guideTapping && (buttonFlags & ControllerPacket.SPECIAL_BUTTON_FLAG) != 0) {
                    canceledTapEnd = context.guideTapEnd;
                    context.guideTapEnd = null;
                    context.guideTapping = false;
                    sendCancelPreviousTap = true;
                }
                if (context.guideDown && (buttonFlags & ControllerPacket.SPECIAL_BUTTON_FLAG) != 0 &&
                        (buttonFlags & ~ControllerPacket.SPECIAL_BUTTON_FLAG) != 0 && context.guideHold.chord()) {
                    cancelHoldRunnable = true;
                    sendChordGuide = true;
                    chordQuitButtons = context.quitButtons;
                }
            }
            if (canceledTapEnd != null) {
                handler.mainThreadHandler.removeCallbacks(canceledTapEnd);
            }
            if (sendCancelPreviousTap) {
                reportUsbControllerState(controllerId, buttonFlags & ~ControllerPacket.SPECIAL_BUTTON_FLAG,
                        leftStickX, leftStickY, rightStickX, rightStickY, leftTrigger, rightTrigger);
            }
            if (cancelHoldRunnable) {
                handler.mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
            }
            if (sendChordGuide) {
                reportUsbControllerState(controllerId, chordQuitButtons, leftStickX, leftStickY,
                        rightStickX, rightStickY, leftTrigger, rightTrigger);
            }
            reportUsbControllerState(controllerId, buttonFlags, leftStickX, leftStickY, rightStickX, rightStickY,
                    leftTrigger, rightTrigger);
            Runnable scheduleEnd = null;
            synchronized (context) {
                if (context.guideTapping && context.guideTapEnd == null && !handler.stopped) {
                    scheduleEnd = new Runnable() {
                        @Override
                        public void run() {
                            synchronized (context.usbSendLock) {
                                int rawButtons;
                                float lsX, lsY, rsX, rsY, lt, rt;
                                synchronized (context) {
                                    if (context.guideTapEnd != this) {
                                        return;
                                    }
                                    context.guideTapEnd = null;
                                    context.guideTapping = false;
                                    if (handler.stopped || handler.usbDeviceContexts.get(controllerId) != context) {
                                        return;
                                    }
                                    rawButtons = context.lastRawButtons;
                                    lsX = context.lastRawLeftStickX;
                                    lsY = context.lastRawLeftStickY;
                                    rsX = context.lastRawRightStickX;
                                    rsY = context.lastRawRightStickY;
                                    lt = context.lastRawLeftTrigger;
                                    rt = context.lastRawRightTrigger;
                                }
                                reportUsbControllerState(controllerId, rawButtons, lsX, lsY, rsX, rsY, lt, rt);
                            }
                        }
                    };
                    context.guideTapEnd = scheduleEnd;
                }
            }
            if (scheduleEnd != null) {
                handler.mainThreadHandler.postDelayed(scheduleEnd, ControllerHandler.MINIMUM_BUTTON_DOWN_TIME_MS);
            }
        }
    }

    private void reportUsbControllerState(int controllerId, int buttonFlags,
                                          float leftStickX, float leftStickY,
                                          float rightStickX, float rightStickY,
                                          float leftTrigger, float rightTrigger) {
        ControllerHandler.UsbDeviceContext context = handler.usbDeviceContexts.get(controllerId);
        if (handler.stopped || context == null) {
            return;
        }
        boolean steerMenuNow = false;
        ControllerHandler.PadCapture captureNow = null;
        int capturedButtons = 0;
        boolean sendGameInput = false;
        boolean quitChanged = false;
        boolean openMenuNow = false;
        synchronized (context) {
            if (handler.stopped) {
                return;
            }
            context.lastRawButtons = buttonFlags;
            context.lastRawLeftStickX = leftStickX;
            context.lastRawLeftStickY = leftStickY;
            context.lastRawRightStickX = rightStickX;
            context.lastRawRightStickY = rightStickY;
            context.lastRawLeftTrigger = leftTrigger;
            context.lastRawRightTrigger = rightTrigger;
            quitChanged = buttonFlags != context.quitButtons;
            context.quitButtons = buttonFlags;
            buttonFlags = guideHoldButtons(context, buttonFlags);
            int buttons = context.buttonMap.mapButtonFlags(buttonFlags, handler.prefConfig.flipFaceButtons);
            if (!handler.inputEnabled) {
                steerMenuNow = true;
                capturedButtons = buttons;
            } else {
                captureNow = handler.padCapture;
                if (captureNow != null) {
                    capturedButtons = buttons;
                } else {
                    buttons = NightjarInputJni.usbFilterGameButtons(handler.usbInputStateHandle, controllerId, buttons);
                    NightjarInputJni.normalizeUsbAxes(
                            leftStickX, leftStickY, rightStickX, rightStickY, leftTrigger, rightTrigger,
                            context.leftStickDeadzoneRadius, context.rightStickDeadzoneRadius,
                            context.triggerDeadzone, context.usbAxesOut);
                    context.leftStickX = (short) context.usbAxesOut[0];
                    context.leftStickY = (short) context.usbAxesOut[1];
                    context.rightStickX = (short) context.usbAxesOut[2];
                    context.rightStickY = (short) context.usbAxesOut[3];
                    context.leftTrigger = (byte) context.usbAxesOut[4];
                    context.rightTrigger = (byte) context.usbAxesOut[5];
                    context.inputMap = context.clickpadButtons(buttons);
                    if (NightjarInputJni.shouldArmMenuCombo(buttons)) {
                        context.menuComboPending = true;
                    }
                    if (quitChanged) handler.buttons.holdQuitCombo(context, buttonFlags);
                    if (context.menuComboPending && buttons == 0) {
                        context.menuComboPending = false;
                        openMenuNow = true;
                    }
                    sendGameInput = true;
                }
            }
        }
        // Perform network sends and UI/handler callbacks outside synchronized(context).
        if (steerMenuNow) {
            steerMenu(controllerId, context, capturedButtons, leftTrigger, rightTrigger);
            return;
        }
        if (captureNow != null) {
            capturePad(controllerId, context, captureNow, capturedButtons, leftStickX, leftStickY);
            return;
        }
        if (!sendGameInput) {
            return;
        }
        handler.sendControllerInputPacket(context);
        if (openMenuNow && handler.activityContext instanceof Game) {
            Game game = (Game) handler.activityContext;
            runOnMainThread(game::showStreamMenuFromController);
        }
        handler.buttons.releaseQuitCombo(context, buttonFlags);
    }

    private void runOnMainThread(Runnable action) {
        if (android.os.Looper.myLooper() == android.os.Looper.getMainLooper()) {
            action.run();
        } else {
            handler.mainThreadHandler.post(action);
        }
    }

    private void capturePad(int controllerId, ControllerHandler.UsbDeviceContext context, ControllerHandler.PadCapture capture, int buttons, float stickX, float stickY) {
        int entered;
        int changed;
        int newButtons;
        int stickChanged;
        synchronized (context) {
            NightjarInputJni.usbCapturePadStep(
                    handler.usbInputStateHandle, controllerId, buttons, stickX, stickY,
                    handler.captureGeneration.get(), context.captureOut);
            entered = context.captureOut[0];
            changed = context.captureOut[1];
            newButtons = context.captureOut[2];
            stickChanged = context.captureOut[3];
        }
        if (entered != 0) {
            synchronized (context) {
                context.menuComboPending = false;
                context.quitComboPending = false;
            }
            // The game's last state is cleared here, on the thread that sends this pad's reports.
            handler.gamepad.releaseUsbInput(context);
        }
        for (int flag : ControllerHandler.capturedButtonFlags()) {
            if ((changed & flag) != 0) capture.button(flag, (newButtons & flag) != 0);
        }
        // The stick is reported on every report; the keyboard only needs to hear when its direction changes.
        if (stickChanged != 0) {
            capture.stick(context.id, stickX, stickY);
        }
    }

    // While the stream menu or one of its dialogs has focus, a newly pressed D-pad direction, A, B,
    // trigger pull or menu-close gesture of a USB-driver pad steers it.
    private void steerMenu(int controllerId, ControllerHandler.UsbDeviceContext context, int buttons,
                           float leftTrigger, float rightTrigger) {
        int count;
        int[] keys;
        synchronized (context) {
            count = NightjarInputJni.usbSteerMenuStep(
                    handler.usbInputStateHandle, controllerId, buttons, leftTrigger, rightTrigger,
                    context.steerKeysOut);
            if (context.steerKeysOut[0] != 0) {
                context.menuComboPending = false;
                context.quitComboPending = false;
            }
            if (count <= 0) {
                return;
            }
            keys = new int[count];
            System.arraycopy(context.steerKeysOut, 1, keys, 0, count);
        }
        if (!(handler.activityContext instanceof Game)) {
            return;
        }
        Game game = (Game) handler.activityContext;
        runOnMainThread(() -> {
            for (int key : keys) {
                game.steerStreamMenu(key);
            }
        });
    }

    void reportControllerTouch(int controllerId, byte eventType, int pointerId, float x, float y, float pressure) {
        ControllerHandler.UsbDeviceContext context = handler.usbDeviceContexts.get(controllerId);
        if (!handler.stopped && handler.inputEnabled && handler.prefConfig.sendControllers && context != null && context.assignedControllerNumber &&
                (context.capabilities() & MoonBridge.LI_CCAP_TOUCHPAD) != 0) {
            handler.conn.sendControllerTouchEvent((byte) context.controllerNumber, eventType, pointerId, x, y, pressure);
        }
    }

    void reportControllerMotion(int controllerId, byte motionType, float x, float y, float z) {
        ControllerHandler.UsbDeviceContext context = handler.usbDeviceContexts.get(controllerId);
        if (!handler.stopped && handler.inputEnabled && handler.sensorsEnabled && handler.prefConfig.sendControllers && handler.prefConfig.gamepadMotionSensors && context != null &&
                context.assignedControllerNumber && !ControllerHandler.dropsMotionAndTouchpad(context.buttonMap)) {
            if (motionType == MoonBridge.LI_MOTION_TYPE_GYRO) {
                GyroAxisTransformer mapping = GyroDiagnosticOverlay.isFlydigiForced()
                        ? GyroMappingPreferences.FLYDIGI_TRANSFORM : context.gyroMapping;
                if (!mapping.isIdentity()) {
                    // One driver thread reports each pad, so its buffer is never shared.
                    mapping.transform(x, y, z, context.mappedMotion);
                    x = context.mappedMotion[0];
                    y = context.mappedMotion[1];
                    z = context.mappedMotion[2];
                }
            }
            if (GyroDiagnosticOverlay.isEnabled()) {
                GyroDiagnosticOverlay.report("usb", context.controllerNumber, motionType, x, y, z);
            }
            handler.conn.sendControllerMotionEvent((byte) context.controllerNumber, motionType, x, y, z);
        }
    }

    void reportControllerBattery(int controllerId, byte state, byte percentage) {
        ControllerHandler.UsbDeviceContext context = handler.usbDeviceContexts.get(controllerId);
        if (!handler.stopped && handler.prefConfig.sendControllers && context != null && context.assignedControllerNumber) {
            handler.conn.sendControllerBatteryEvent((byte) context.controllerNumber, state, percentage);
        }
    }

    void deviceRemoved(AbstractController controller) {
        // Stream teardown retains host controller identities; only a physical removal
        // during a live stream should clear the active controller mask.
        if (handler.stopped) return;
        ControllerHandler.UsbDeviceContext context = handler.usbDeviceContexts.get(controller.getControllerId());
        if (context != null) {
            LimeLog.info("Removed controller: " + controller.getControllerId());
            // Unplugged while the PC keyboard was open: nothing will report what it held being let go.
            int[] removeOut = new int[3];
            NightjarInputJni.usbRemovePadCapture(handler.usbInputStateHandle, controller.getControllerId(), removeOut);
            ControllerHandler.PadCapture capture = handler.padCapture;
            if (capture != null && removeOut[0] != 0) {
                int heldMenuButtons = removeOut[1];
                for (int flag : ControllerHandler.capturedButtonFlags()) {
                    if ((heldMenuButtons & flag) != 0) capture.button(flag, false);
                }
                if (removeOut[2] != 0) {
                    capture.stick(context.id, 0f, 0f);
                }
            }
            if (handler.prefConfig.sendControllers && context.assignedControllerNumber &&
                    (context.capabilities() & MoonBridge.LI_CCAP_TOUCHPAD) != 0) {
                handler.conn.sendControllerTouchEvent((byte) context.controllerNumber, MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL,
                        0, 0, 0, 0);
            }
            handler.slots.releaseControllerNumber(context);
            handler.usbDeviceContexts.remove(controller.getControllerId(), context);
            context.destroy();
            if (context.assignedControllerNumber) handler.slots.announceBuiltInControls(context.controllerNumber);
        }
        handler.slots.announcePresence();
    }

    void deviceAdded(AbstractController controller) {
        if (handler.stopped) {
            controller.stop();
            return;
        }
        if (handler.usbDeviceContexts.get(controller.getControllerId()) != null) {
            return;
        }

        ControllerHandler.UsbDeviceContext context = handler.classifier.createUsbDeviceContextForDevice(controller);
        handler.usbDeviceContexts.put(controller.getControllerId(), context);
        if (handler.connectionStarted && handler.conn.canSendControllerInput()) handler.sendControllerInputPacket(context);
        handler.slots.announcePresence();
        handler.slots.controllerAdded();
    }
}
