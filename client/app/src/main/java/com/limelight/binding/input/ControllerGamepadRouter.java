package com.limelight.binding.input;

import android.os.Build;
import android.view.InputDevice;
import android.view.InputEvent;
import android.view.MotionEvent;
import android.widget.Toast;

import com.limelight.R;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarInputJni;
import com.limelight.preferences.PreferenceConfiguration;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * Handles slot input fusion, packet dispatch, analog axis and controller touchpad events,
 * controller mouse emulation, and input release for {@link ControllerHandler}.
 */
final class ControllerGamepadRouter {
    private final ControllerHandler handler;
    private final int[] axisSetOut = new int[8];
    private int[] fuseScratch = new int[ControllerHandler.MAX_GAMEPADS * 6];
    private final int[] fuseOut = new int[7];

    ControllerGamepadRouter(ControllerHandler handler) {
        this.handler = handler;
    }

    void releaseInput(boolean includeUsbPads) {
        if (handler.stopped) {
            return;
        }
        for (ControllerHandler.InputDeviceContext context : handler.inputDeviceContexts.values()) {
            releaseInput(context);
            cancelInputTouchpad(context);
            if (context.assignedControllerNumber && context.gyroListener != null) {
                // Motion stops with the rest of the input; a held angular velocity keeps turning the camera.
                handler.motion.sendGyroAtRest(context.controllerNumber);
            }
        }
        if (includeUsbPads) {
            for (ControllerHandler.UsbDeviceContext context : handler.usbDeviceContexts.values()) {
                releaseUsbInput(context);
            }
        }
        handler.layoutInputMap = 0;
        handler.layoutLeftTrigger = handler.layoutRightTrigger = 0;
        handler.layoutLeftStickX = handler.layoutLeftStickY = handler.layoutRightStickX = handler.layoutRightStickY = 0;
        releaseInput(handler.defaultContext);
        cancelInputTouchpad(handler.defaultContext);
        if (handler.handheldContext != handler.defaultContext) {
            releaseInput(handler.handheldContext);
            cancelInputTouchpad(handler.handheldContext);
        }
    }

    private void cancelInputTouchpad(ControllerHandler.InputDeviceContext context) {
        if (!handler.prefConfig.sendControllers || !context.assignedControllerNumber) {
            return;
        }
        if ((context.reportedCapabilities & MoonBridge.LI_CCAP_TOUCHPAD) != 0 || context.isDualShockStandaloneTouchpad) {
            handler.conn.sendControllerTouchEvent((byte) context.controllerNumber, MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL,
                    0, 0, 0, 0);
        }
    }

    void releaseUsbInput(ControllerHandler.UsbDeviceContext context) {
        releaseInput(context);
        // A pad whose capabilities have not reached the host yet has nothing to release there,
        // nor has any physical pad with Controllers Off.
        if (!handler.prefConfig.sendControllers || !context.assignedControllerNumber || context.arrivalPending) {
            return;
        }
        if ((context.capabilities() & MoonBridge.LI_CCAP_TOUCHPAD) != 0) {
            handler.conn.sendControllerTouchEvent((byte) context.controllerNumber, MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL,
                    0, 0, 0, 0);
        }
        if (handler.prefConfig.gamepadMotionSensors && (context.capabilities() & MoonBridge.LI_CCAP_GYRO) != 0) {
            handler.motion.sendGyroAtRest(context.controllerNumber);
        }
    }

    private void releaseInput(ControllerHandler.GenericControllerContext context) {
        context.inputMap = 0;
        context.leftTrigger = context.rightTrigger = 0;
        context.leftStickX = context.leftStickY = 0;
        context.rightStickX = context.rightStickY = 0;
        releaseMouseButtons(context);
        // A quit combo held as input went away (focus lost, the menu opened) is no longer held.
        context.quitComboPending = false;
        context.quitButtons = 0;
        // So is a Guide waiting to open the stream menu: no menu or tap follows, and like one that opened
        // the menu it stays held back until let go.
        if (context.guideHold.cancelPending()) {
            handler.mainThreadHandler.removeCallbacks(context.guideHoldRunnable);
        }
        if (context.guideTapEnd != null) {
            handler.mainThreadHandler.removeCallbacks(context.guideTapEnd);
            context.guideTapEnd = null;
        }
        context.guideTapping = false;
        if (context instanceof ControllerHandler.InputDeviceContext) {
            ControllerHandler.InputDeviceContext inputContext = (ControllerHandler.InputDeviceContext) context;
            inputContext.pressedButtons.clear();
            for (Runnable r : inputContext.pendingButtonReleases.values()) {
                handler.mainThreadHandler.removeCallbacks(r);
            }
            inputContext.pendingButtonReleases.clear();
            inputContext.emulatingButtonFlags = 0;
            inputContext.pendingExit = false;
        }
        if (context.assignedControllerNumber && NightjarInputJni.slotHasInput(handler.slotStateHandle, context.controllerNumber) &&
                (handler.prefConfig.sendControllers || context == handler.defaultContext)) {
            // Bypass fusion while the other contexts sharing this number are being cleared.
            // Only a slot the host has seen input on can hold anything: a neutral report for
            // any other slot makes the host create a default pad there, which then ignores
            // the real controller's arrival (type, touchpad and motion) for the session.
            handler.conn.sendControllerInput(context.controllerNumber, handler.getActiveControllerMask(),
                    0, (byte) 0, (byte) 0, (short) 0, (short) 0, (short) 0, (short) 0);
        }
    }

    boolean hasPendingControllerArrival(short controllerNumber) {
        for (ControllerHandler.UsbDeviceContext context : handler.usbDeviceContexts.values()) {
            if (context.assignedControllerNumber && context.controllerNumber == controllerNumber && context.arrivalPending) {
                return true;
            }
        }
        return false;
    }

    ControllerHandler.InputDeviceContext getContextForEvent(InputEvent event) {
        // Don't return a context if we're stopped
        if (handler.stopped) {
            return null;
        }
        else if (event.getDeviceId() == 0) {
            // Unknown devices use the default context, or the built-in pad's with the on-screen controls on controller 1
            return handler.slots.handheldContext();
        }
        else if (event.getDevice() == null) {
            // During device removal, sometimes we can get events after the
            // input device has been destroyed. In this case we'll see a
            // != 0 device ID but no device attached.
            return null;
        }

        // HACK for https://issuetracker.google.com/issues/163120692
        if (Build.VERSION.SDK_INT == Build.VERSION_CODES.R) {
            if (event.getDeviceId() == -1) {
                return handler.slots.handheldContext();
            }
        }

        // Return or atomically register one context per device ID so launch prep and input never claim two slots.
        return handler.slots.getOrCreateInputDeviceContext(event.getDeviceId(), event.getDevice());
    }

    private int packFusionContext(int count, int buttons, byte lt, byte rt, short lx, short ly, short rx, short ry) {
        int base = count * 6;
        if (base + 6 > fuseScratch.length) {
            fuseScratch = java.util.Arrays.copyOf(fuseScratch, fuseScratch.length * 2);
        }
        fuseScratch[base] = buttons;
        fuseScratch[base + 1] = (lt & 0xFF) | ((rt & 0xFF) << 8);
        fuseScratch[base + 2] = lx;
        fuseScratch[base + 3] = ly;
        fuseScratch[base + 4] = rx;
        fuseScratch[base + 5] = ry;
        return count + 1;
    }

    void sendControllerInputPacket(ControllerHandler.GenericControllerContext originalContext) {
        // Controllers set to Off: no physical controller reaches the PC (the on-screen gamepad still does).
        boolean physical = handler.prefConfig.sendControllers;
        if (!physical && originalContext != handler.defaultContext) return;
        handler.assignControllerNumberIfNeeded(originalContext);
        if (!originalContext.assignedControllerNumber) {
            return;
        }
        for (ControllerHandler.UsbDeviceContext context : physical ? handler.usbDeviceContexts.values() : Collections.<ControllerHandler.UsbDeviceContext>emptyList()) {
            if (context.assignedControllerNumber && context.controllerNumber == originalContext.controllerNumber &&
                    !context.ensureControllerArrival() && (context == originalContext ||
                    !NightjarInputJni.slotHasInput(handler.slotStateHandle, originalContext.controllerNumber))) {
                // A new slot must wait for its USB capabilities. A context already
                // using a shared slot must still be able to release its held input.
                return;
            }
        }

        // Take the context's controller number and fuse all inputs with the same number
        short controllerNumber = originalContext.controllerNumber;
        int inputMap;
        byte leftTrigger;
        byte rightTrigger;
        short leftStickX;
        short leftStickY;
        short rightStickX;
        short rightStickY;

        synchronized (handler.fuseLock) {
            int count = 0;
            for (ControllerHandler.GenericControllerContext context : physical ? handler.inputDeviceContexts.values() : Collections.<ControllerHandler.GenericControllerContext>emptyList()) {
                if (context.assignedControllerNumber &&
                        context.controllerNumber == controllerNumber &&
                        context.mouseEmulationActive == originalContext.mouseEmulationActive) {
                    context.axisMap.apply(context.inputMap, context.leftTrigger, context.rightTrigger,
                            context.leftStickX, context.leftStickY, context.rightStickX, context.rightStickY, context.mappedReport);
                    ControllerAxisMap.Report mapped = context.mappedReport;
                    count = packFusionContext(count, mapped.buttons, mapped.lt, mapped.rt,
                            mapped.lx, mapped.ly, mapped.rx, mapped.ry);
                }
            }
            for (ControllerHandler.UsbDeviceContext context : physical ? handler.usbDeviceContexts.values() : Collections.<ControllerHandler.UsbDeviceContext>emptyList()) {
                if (context.assignedControllerNumber && !context.arrivalPending &&
                        context.controllerNumber == controllerNumber &&
                        context.mouseEmulationActive == originalContext.mouseEmulationActive) {
                    context.axisMap.apply(context.inputMap, context.leftTrigger, context.rightTrigger,
                            context.leftStickX, context.leftStickY, context.rightStickX, context.rightStickY, context.mappedReport);
                    ControllerAxisMap.Report mapped = context.mappedReport;
                    count = packFusionContext(count, mapped.buttons, mapped.lt, mapped.rt,
                            mapped.lx, mapped.ly, mapped.rx, mapped.ry);
                }
            }
            if (handler.defaultContext.controllerNumber == controllerNumber) {
                count = packFusionContext(count, handler.defaultContext.inputMap, handler.defaultContext.leftTrigger, handler.defaultContext.rightTrigger,
                        handler.defaultContext.leftStickX, handler.defaultContext.leftStickY, handler.defaultContext.rightStickX, handler.defaultContext.rightStickY);
                count = packFusionContext(count, handler.layoutInputMap, handler.layoutLeftTrigger, handler.layoutRightTrigger,
                        handler.layoutLeftStickX, handler.layoutLeftStickY, handler.layoutRightStickX, handler.layoutRightStickY);
            }
            if (physical && handler.handheldContext != handler.defaultContext && handler.handheldContext.controllerNumber == controllerNumber) {
                count = packFusionContext(count, handler.handheldContext.inputMap, handler.handheldContext.leftTrigger, handler.handheldContext.rightTrigger,
                        handler.handheldContext.leftStickX, handler.handheldContext.leftStickY, handler.handheldContext.rightStickX, handler.handheldContext.rightStickY);
            }
            NightjarInputJni.fuseControllerInputs(
                    fuseScratch, count, handler.steamControlsSession, controllerNumber,
                    handler.steamPadTouchX, originalContext.mouseEmulationActive, fuseOut);
            inputMap = fuseOut[0];
            leftTrigger = (byte) fuseOut[1];
            rightTrigger = (byte) fuseOut[2];
            leftStickX = (short) fuseOut[3];
            leftStickY = (short) fuseOut[4];
            rightStickX = (short) fuseOut[5];
            rightStickY = (short) fuseOut[6];
        }

        short activeControllerMask = handler.getActiveControllerMask();
        if (originalContext.mouseEmulationActive) {
            int flags;
            int mouseStep;
            int leftBtn;
            int rightBtn;
            synchronized (originalContext.mousePacketLock) {
                NightjarInputJni.mousePacketStep(
                        inputMap, originalContext.mouseEmulationLastInputMap,
                        handler.prefConfig.mouseEmulationFixedStep, originalContext.mouseEmulationStep,
                        originalContext.mousePacketOut);
                originalContext.mouseEmulationLastInputMap = originalContext.mousePacketOut[0];
                mouseStep = originalContext.mousePacketOut[1];
                originalContext.mouseEmulationStep = mouseStep;
                flags = originalContext.mousePacketOut[2];
                originalContext.mouseEmulationFixedStepHeld = (flags & 1) != 0;
                leftBtn = originalContext.mousePacketOut[3];
                rightBtn = originalContext.mousePacketOut[4];
            }
            if ((flags & (1 << 1)) != 0) {
                int step = mouseStep;
                Runnable showStep = () -> Toast.makeText(handler.activityContext,
                        handler.activityContext.getString(R.string.stream_mouse_step, step), Toast.LENGTH_SHORT).show();
                if (android.os.Looper.myLooper() == android.os.Looper.getMainLooper()) {
                    showStep.run();
                } else {
                    handler.mainThreadHandler.post(showStep);
                }
            }
            if (leftBtn > 0) handler.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_LEFT);
            else if (leftBtn < 0) handler.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_LEFT);
            if (rightBtn > 0) handler.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_RIGHT);
            else if (rightBtn < 0) handler.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
            if ((flags & (1 << 2)) != 0) handler.conn.sendMouseScroll((byte) 1);
            if ((flags & (1 << 3)) != 0) handler.conn.sendMouseScroll((byte) -1);
            if ((flags & (1 << 4)) != 0) handler.conn.sendMouseHScroll((byte) 1);
            if ((flags & (1 << 5)) != 0) handler.conn.sendMouseHScroll((byte) -1);

            handler.conn.sendControllerInput(controllerNumber, activeControllerMask,
                    (short) 0, (byte) 0, (byte) 0, (short) 0, (short) 0, (short) 0, (short) 0);
        }
        else {
            handler.conn.sendControllerInput(controllerNumber, activeControllerMask,
                    inputMap,
                    leftTrigger, rightTrigger,
                    leftStickX, leftStickY,
                    rightStickX, rightStickY);
        }
        if (handler.conn.canSendControllerInput() && (activeControllerMask & (1 << controllerNumber)) != 0) {
            NightjarInputJni.markSlotHasInput(handler.slotStateHandle, controllerNumber);
            handler.controllerSlotsWithInput.set(controllerNumber, 1);
        }
    }

    private void handleAxisSet(ControllerHandler.InputDeviceContext context, float lsX, float lsY, float rsX,
                               float rsY, float lt, float rt, float hatX, float hatY) {
        NightjarInputJni.normalizeAxisSet(
                lsX, lsY, rsX, rsY, lt, rt, hatX, hatY,
                context.leftStickXAxis != -1 && context.leftStickYAxis != -1,
                context.rightStickXAxis != -1 && context.rightStickYAxis != -1,
                context.leftTriggerAxis != -1 && context.rightTriggerAxis != -1,
                context.hatXAxis != -1 && context.hatYAxis != -1,
                context.leftStickDeadzoneRadius,
                context.rightStickDeadzoneRadius,
                context.triggerDeadzone,
                context.triggersIdleNegative,
                context.leftTriggerAxisUsed,
                context.rightTriggerAxisUsed,
                context.hatXAxisUsed,
                context.hatYAxisUsed,
                context.inputMap,
                axisSetOut);
        context.inputMap = axisSetOut[0];
        context.leftTrigger = (byte) axisSetOut[1];
        context.rightTrigger = (byte) axisSetOut[2];
        context.leftStickX = (short) axisSetOut[3];
        context.leftStickY = (short) axisSetOut[4];
        context.rightStickX = (short) axisSetOut[5];
        context.rightStickY = (short) axisSetOut[6];
        int usedBits = axisSetOut[7];
        context.leftTriggerAxisUsed = (usedBits & 1) != 0;
        context.rightTriggerAxisUsed = (usedBits & 2) != 0;
        context.hatXAxisUsed = (usedBits & 4) != 0;
        context.hatYAxisUsed = (usedBits & 8) != 0;

        handler.sendControllerInputPacket(context);
    }

    // Normalize the given raw float value into a 0.0-1.0f range
    private static float normalizeRawValueWithRange(float value, InputDevice.MotionRange range) {
        value = Math.max(value, range.getMin());
        value = Math.min(value, range.getMax());

        value -= range.getMin();

        return value / range.getRange();
    }

    private boolean sendTouchpadEventForPointer(ControllerHandler.InputDeviceContext context, MotionEvent event, byte touchType, int pointerIndex) {
        float normalizedX = normalizeRawValueWithRange(event.getX(pointerIndex), context.touchpadXRange);
        float normalizedY = normalizeRawValueWithRange(event.getY(pointerIndex), context.touchpadYRange);
        float normalizedPressure = context.touchpadPressureRange != null ?
                normalizeRawValueWithRange(event.getPressure(pointerIndex), context.touchpadPressureRange)
                : 0;

        if (handler.steamControlsSession && context.controllerNumber == 0) handler.steamPadTouchX = normalizedX;
        return handler.conn.sendControllerTouchEvent((byte) context.controllerNumber, touchType,
                event.getPointerId(pointerIndex),
                normalizedX, normalizedY, normalizedPressure) != MoonBridge.LI_ERR_UNSUPPORTED;
    }

    boolean tryHandleTouchpadEvent(MotionEvent event) {
        // Bail if this is not a touchpad or mouse event
        if (event.getSource() != InputDevice.SOURCE_TOUCHPAD &&
                event.getSource() != InputDevice.SOURCE_MOUSE) {
            return false;
        }

        // Only get a context if one already exists. We want to ensure we don't report non-gamepads.
        ControllerHandler.InputDeviceContext context = handler.inputDeviceContexts.get(event.getDeviceId());
        if (context == null) {
            return false;
        }

        // When we're working with a mouse source instead of a touchpad, we're quite limited in
        // what useful input we can provide via the controller API. The ABS_X/ABS_Y values are
        // screen coordinates rather than touchpad coordinates. For now, we will just support
        // the clickpad button and nothing else.
        if (event.getSource() == InputDevice.SOURCE_MOUSE) {
            switch (event.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    context.inputMap |= ControllerPacket.TOUCHPAD_FLAG;
                    handler.sendControllerInputPacket(context);
                    break;
                case MotionEvent.ACTION_UP:
                case MotionEvent.ACTION_CANCEL:
                    context.inputMap &= ~ControllerPacket.TOUCHPAD_FLAG;
                    handler.sendControllerInputPacket(context);
                    break;
                default:
                    break;
            }

            return !handler.prefConfig.gamepadTouchpadAsMouse;
        }

        byte touchType;
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN:
                touchType = MoonBridge.LI_TOUCH_EVENT_DOWN;
                break;

            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP:
                if ((event.getFlags() & MotionEvent.FLAG_CANCELED) != 0) {
                    touchType = MoonBridge.LI_TOUCH_EVENT_CANCEL;
                }
                else {
                    touchType = MoonBridge.LI_TOUCH_EVENT_UP;
                }
                break;

            case MotionEvent.ACTION_MOVE:
                touchType = MoonBridge.LI_TOUCH_EVENT_MOVE;
                break;

            case MotionEvent.ACTION_CANCEL:
                touchType = MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL;
                break;

            case MotionEvent.ACTION_BUTTON_PRESS:
                if (event.getActionButton() == MotionEvent.BUTTON_PRIMARY) {
                    context.inputMap |= ControllerPacket.TOUCHPAD_FLAG;
                    handler.sendControllerInputPacket(context);
                    return !handler.prefConfig.gamepadTouchpadAsMouse;
                }
                return false;

            case MotionEvent.ACTION_BUTTON_RELEASE:
                if (event.getActionButton() == MotionEvent.BUTTON_PRIMARY) {
                    context.inputMap &= ~ControllerPacket.TOUCHPAD_FLAG;
                    handler.sendControllerInputPacket(context);
                    return !handler.prefConfig.gamepadTouchpadAsMouse;
                }
                return false;

            default:
                return false;
        }

        // Bail if the user wants gamepad touchpads to control the mouse
        if (handler.prefConfig.gamepadTouchpadAsMouse) {
            return false;
        }

        // A pad pinned as Xbox told the host it has no touchpad
        if (ControllerHandler.dropsMotionAndTouchpad(context.buttonMap) || !handler.prefConfig.sendControllers) {
            return true;
        }

        // If we don't have X and Y ranges, we can't process this event
        if (context.touchpadXRange == null || context.touchpadYRange == null) {
            return false;
        }

        if (event.getActionMasked() == MotionEvent.ACTION_MOVE) {
            for (int i = 0; i < event.getPointerCount(); i++) {
                if (!sendTouchpadEventForPointer(context, event, touchType, i)) {
                    return false;
                }
            }
            return true;
        }
        else if (event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
            return handler.conn.sendControllerTouchEvent((byte) context.controllerNumber, MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL,
                    0, 0, 0, 0) != MoonBridge.LI_ERR_UNSUPPORTED;
        }
        else {
            return sendTouchpadEventForPointer(context, event, touchType, event.getActionIndex());
        }
    }

    boolean handleMotionEvent(MotionEvent event) {
        ControllerHandler.InputDeviceContext context = handler.getContextForEvent(event);
        if (context == null) {
            return true;
        }

        float lsX = 0, lsY = 0, rsX = 0, rsY = 0, rt = 0, lt = 0, hatX = 0, hatY = 0;

        if (context.leftStickXAxis != -1 && context.leftStickYAxis != -1) {
            lsX = event.getAxisValue(context.leftStickXAxis);
            lsY = event.getAxisValue(context.leftStickYAxis);
        }

        if (context.rightStickXAxis != -1 && context.rightStickYAxis != -1) {
            rsX = event.getAxisValue(context.rightStickXAxis);
            rsY = event.getAxisValue(context.rightStickYAxis);
        }

        if (context.leftTriggerAxis != -1 && context.rightTriggerAxis != -1) {
            lt = event.getAxisValue(context.leftTriggerAxis);
            rt = event.getAxisValue(context.rightTriggerAxis);
        }

        if (context.hatXAxis != -1 && context.hatYAxis != -1) {
            hatX = event.getAxisValue(MotionEvent.AXIS_HAT_X);
            hatY = event.getAxisValue(MotionEvent.AXIS_HAT_Y);
        }

        if (hatX != 0 || hatY != 0) {
            // A D-pad chord with a waiting Guide (Guide+D-pad) sends Guide first
            handler.buttons.sendHeldGuide(context);
        }

        handleAxisSet(context, lsX, lsY, rsX, rsY, lt, rt, hatX, hatY);

        return true;
    }

    // Every controller context the mouse switch covers: the default one and each pad.
    private List<ControllerHandler.GenericControllerContext> mouseContexts() {
        List<ControllerHandler.GenericControllerContext> contexts = new ArrayList<>();
        contexts.add(handler.defaultContext);
        if (handler.handheldContext != handler.defaultContext) contexts.add(handler.handheldContext);
        contexts.addAll(handler.inputDeviceContexts.values());
        contexts.addAll(handler.usbDeviceContexts.values());
        return contexts;
    }

    boolean isMouseEmulationActive() {
        for (ControllerHandler.GenericControllerContext context : mouseContexts()) {
            if (context.mouseEmulationActive) return true;
        }
        return false;
    }

    /**
     * The stream menu's controller mouse switch, as in Moonlight V+ PC: every active controller
     * mouse turns off, or the first controller's turns on. It works with the Start long press off.
     */
    void toggleMouseEmulationFromMenu() {
        List<ControllerHandler.GenericControllerContext> contexts = mouseContexts();
        int n = contexts.size();
        boolean[] eligible = new boolean[n];
        short[] numbers = new short[n];
        boolean[] active = new boolean[n];
        for (int i = 0; i < n; i++) {
            ControllerHandler.GenericControllerContext c = contexts.get(i);
            eligible[i] = c != handler.defaultContext && c != handler.handheldContext && c.assignedControllerNumber;
            numbers[i] = c.controllerNumber;
            active[i] = c.mouseEmulationActive;
        }
        int target = NightjarInputJni.pickMenuMouseTarget(eligible, numbers, active, n);
        if (target == -1) {
            for (ControllerHandler.GenericControllerContext context : contexts) {
                if (context.mouseEmulationActive) context.toggleMouseEmulation();
            }
        }
        else if (target >= 0) {
            contexts.get(target).toggleMouseEmulation();
        }
    }

    boolean mouseSilenced(ControllerHandler.GenericControllerContext context) {
        return NightjarInputJni.mouseSilenced(handler.prefConfig.sendControllers, context == handler.defaultContext);
    }

    private java.util.function.LongSupplier nanoClock = System::nanoTime;

    void setNanoClockForTest(java.util.function.LongSupplier clock) {
        this.nanoClock = clock != null ? clock : System::nanoTime;
    }

    void runMouseEmulationTick(ControllerHandler.GenericControllerContext context, Runnable self) {
        if (!context.mouseEmulationActive) {
            return;
        }
        if (mouseSilenced(context)) {
            return;
        }

        int fixedStep;
        float dtScale;
        synchronized (context.mousePacketLock) {
            fixedStep = context.mouseEmulationFixedStepHeld ? context.mouseEmulationStep : 0;
            long nowNs = nanoClock.getAsLong();
            dtScale = ControllerMouseTick.dtCompensation(
                    context.mouseEmulationLastTickNs, nowNs, context.mouseEmulationReportPeriod);
            context.mouseEmulationLastTickNs = nowNs;
        }
        int scrollStickMode = handler.prefConfig.analogStickForScrolling == PreferenceConfiguration.AnalogStickForScrolling.RIGHT ? 1
                : handler.prefConfig.analogStickForScrolling == PreferenceConfiguration.AnalogStickForScrolling.LEFT ? 2 : 0;
        int flags = ControllerMouseTick.compute(
                context.leftStickX, context.leftStickY, context.rightStickX, context.rightStickY,
                scrollStickMode, fixedStep, handler.prefConfig.mouseEmulationSensitivity,
                context.mouseTickFloatOut);
        if ((flags & 1) != 0) {
            short dx;
            short dy;
            synchronized (context.mousePacketLock) {
                dx = (short) context.mouseMoveAccumX.scale(context.mouseTickFloatOut[0], dtScale);
                dy = (short) context.mouseMoveAccumY.scale(context.mouseTickFloatOut[1], dtScale);
            }
            if (dx != 0 || dy != 0) {
                handler.conn.sendMouseMove(dx, dy);
            }
        }
        if ((flags & 2) != 0) {
            short dx;
            short dy;
            synchronized (context.mousePacketLock) {
                dx = (short) context.mouseMoveAccumX.scale(context.mouseTickFloatOut[2], dtScale);
                dy = (short) context.mouseMoveAccumY.scale(context.mouseTickFloatOut[3], dtScale);
            }
            if (dx != 0 || dy != 0) {
                handler.conn.sendMouseMove(dx, dy);
            }
        }
        if ((flags & 4) != 0) {
            short sv;
            short sh;
            synchronized (context.mousePacketLock) {
                sv = (short) context.mouseScrollVAccum.scale(context.mouseTickFloatOut[4], dtScale);
                sh = (short) context.mouseScrollHAccum.scale(context.mouseTickFloatOut[5], dtScale);
            }
            if (sv != 0) {
                handler.conn.sendMouseHighResScroll(sv);
            }
            if (sh != 0) {
                handler.conn.sendMouseHighResHScroll(sh);
            }
        }

        // Requeue the callback
        handler.mainThreadHandler.postDelayed(self, context.mouseEmulationReportPeriod);
    }

    void toggleMouseEmulation(ControllerHandler.GenericControllerContext context) {
        // Mouse mode does not turn on for a pad whose mouse is silenced; turning it off always works.
        if (!context.mouseEmulationActive && mouseSilenced(context)) {
            return;
        }
        handler.mainThreadHandler.removeCallbacks(context.mouseEmulationRunnable);
        boolean active = !context.mouseEmulationActive;
        context.mouseEmulationActive = active;
        context.resetMouseEmulationState();
        releaseMouseButtons(context);
        Toast.makeText(handler.activityContext, active ? R.string.stream_mouse_mode_on :
                R.string.stream_mouse_mode_off, Toast.LENGTH_SHORT).show();

        if (active) {
            synchronized (context.mousePacketLock) {
                context.mouseEmulationLastTickNs = nanoClock.getAsLong();
            }
            handler.mainThreadHandler.postDelayed(context.mouseEmulationRunnable, context.mouseEmulationReportPeriod);
        }
    }

    void releaseMouseButtons(ControllerHandler.GenericControllerContext context) {
        int lastInputMap;
        synchronized (context.mousePacketLock) {
            lastInputMap = context.mouseEmulationLastInputMap;
            context.mouseEmulationLastInputMap = 0;
            context.mouseEmulationFixedStepHeld = false;
        }
        if ((lastInputMap & ControllerPacket.A_FLAG) != 0) {
            handler.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_LEFT);
        }
        if ((lastInputMap & ControllerPacket.B_FLAG) != 0) {
            handler.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
        }
    }
}
