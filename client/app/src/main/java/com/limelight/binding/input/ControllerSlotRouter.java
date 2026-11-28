package com.limelight.binding.input;

import android.content.Context;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbManager;
import android.os.Looper;
import android.view.InputDevice;
import android.widget.Toast;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.input.driver.DualSenseController;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarInputJni;

import java.util.Collections;

/**
 * Coordinates controller slot allocation, release, presence announcements, pre-launch pad
 * preparation, and arrival resends for {@link ControllerHandler}.
 */
final class ControllerSlotRouter {
    private final ControllerHandler handler;

    ControllerSlotRouter(ControllerHandler handler) {
        this.handler = handler;
    }

    void onInputDeviceAdded(int deviceId) {
        if (handler.connectionStarted && !handler.stopped && handler.conn.canSendControllerInput()) {
            ControllerHandler.InputDeviceContext context = prepareInputDevice(deviceId);
            if (context != null) handler.sendControllerInputPacket(context);
        }
        announcePresence();
        if (!handler.stopped && ControllerHandler.isController(handler.inputManager.getInputDevice(deviceId))) {
            controllerAdded();
        }
    }

    void onInputDeviceRemoved(int deviceId) {
        LimeLog.info("Bluetooth or Android controller detached");
        ControllerHandler.InputDeviceContext context = handler.inputDeviceContexts.get(deviceId);
        if (context != null) {
            LimeLog.info("Removed controller: " + context.name + " (" + deviceId + ")");
            // Out of the map first, so a built-in device no longer holds player 1 in the mask.
            handler.inputDeviceContexts.remove(deviceId);
            releaseControllerNumber(context);
            context.destroy();
            if (context.external && context.assignedControllerNumber) announceBuiltInControls(context.controllerNumber);
        }
        announcePresence();
    }

    // This can happen when gaining/losing input focus with some devices.
    // Input devices that have a trackpad may gain/lose AXIS_RELATIVE_X/Y.
    void onInputDeviceChanged(int deviceId) {
        InputDevice device = InputDevice.getDevice(deviceId);
        if (device == null) {
            return;
        }

        // If we don't have a context for this device, we don't need to update anything
        ControllerHandler.InputDeviceContext existingContext = handler.inputDeviceContexts.get(deviceId);
        if (existingContext == null) {
            return;
        }

        LimeLog.info("Device changed: " + existingContext.name + " (" + deviceId + ")");

        // Migrate the existing context into this new one by moving any stateful elements
        ControllerHandler.InputDeviceContext newContext = handler.createInputDeviceContextForDevice(device);
        newContext.migrateContext(existingContext);
        handler.inputDeviceContexts.put(deviceId, newContext);
    }

    void announcePresence() {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            handler.mainThreadHandler.post(this::announcePresence);
            return;
        }
        // Which controllers first, so the presence listener already knows whether one of them brings its own layout.
        if (handler.modelListener != null) {
            java.util.List<String[]> models = handler.classifier.externalControllerModels();
            if (!sameModels(models, handler.controllerModels)) {
                handler.controllerModels = models;
                handler.modelListener.onControllerModelsChanged(models);
            }
        }
        boolean now = handler.hasExternalControllers();
        if (now == handler.externalPresent) return;
        handler.externalPresent = now;
        if (handler.presenceListener != null) handler.presenceListener.onControllerPresenceChanged(now);
    }

    /** Whether two {model, name} lists name the same models in the same order. */
    static boolean sameModels(java.util.List<String[]> a, java.util.List<String[]> b) {
        if (a.size() != b.size()) return false;
        for (int i = 0; i < a.size(); i++) if (!a.get(i)[0].equals(b.get(i)[0])) return false;
        return true;
    }

    void controllerAdded() {
        Runnable listener = handler.controllerAddedListener;
        if (listener == null) return;
        // Input devices are reported on the main thread; the USB driver reports from its own.
        if (Looper.myLooper() == Looper.getMainLooper()) listener.run();
        else handler.mainThreadHandler.post(listener);
    }

    /**
     * The built-in pad, the built-in device with sticks, numbered; null without one. While the on-screen
     * controls have controller 1, the handheld's other built-in devices and its system Back join it. With
     * {@code prepare}, a built-in pad without a number gets one first.
     */
    ControllerHandler.InputDeviceContext builtInPad(boolean prepare) {
        for (ControllerHandler.InputDeviceContext context : handler.inputDeviceContexts.values()) {
            if (!context.external && context.hasJoystickAxes && context.assignedControllerNumber) return context;
        }
        if (prepare) {
            for (int id : InputDevice.getDeviceIds()) {
                InputDevice device = InputDevice.getDevice(id);
                if (device == null || ControllerDeviceClassifier.isExternal(device) ||
                        !ControllerDeviceClassifier.hasJoystickAxes(device)) continue;
                ControllerHandler.InputDeviceContext context = prepareInputDevice(id);
                if (context != null && context.assignedControllerNumber) return context;
            }
        }
        return null;
    }

    /**
     * The context for input with no pad of its own. With the on-screen controls on controller 1 it takes
     * the number of the built-in pad, or of any built-in device without one, or else the first a pad can
     * have; it changes only while nothing is held on it, so a press never splits.
     */
    ControllerHandler.InputDeviceContext handheldContext() {
        synchronized (handler) {
            if (handler.handheldContext != handler.defaultContext &&
                    handler.handheldContext.inputMap == 0 &&
                    handler.handheldContext.guideTapEnd == null) {
                ControllerHandler.InputDeviceContext builtIn = builtInPad(false);
                for (ControllerHandler.InputDeviceContext context : handler.inputDeviceContexts.values()) {
                    if (builtIn == null && !context.external && context.assignedControllerNumber) builtIn = context;
                }
                handler.handheldContext.controllerNumber = builtIn != null ? builtIn.controllerNumber : handler.deviceFallbackSlot;
            }
            return handler.handheldContext;
        }
    }

    void releaseControllerNumber(ControllerHandler.GenericControllerContext context) {
        boolean stillAssigned = context.assignedControllerNumber && slotStillAssigned(context, context.controllerNumber);
        int rumbleRelease = NightjarInputJni.rumbleReleaseSlot(
                handler.rumbleStateHandle, context.controllerNumber, context.assignedControllerNumber,
                stillAssigned, handler.deviceVibratorForPlayerOne);
        if ((rumbleRelease & 1) != 0) {
            handler.deviceVibratorForPlayerOne = false;
            handler.deviceVibrator.cancel();
        }

        // If we reserved a controller number, remove that reservation
        if (context.reservedControllerNumber) {
            LimeLog.info("Controller number " + context.controllerNumber + " is now available");
            handler.currentControllers &= ~(1 << context.controllerNumber);
        }

        int released = NightjarInputJni.releaseReservedSlot(
                handler.slotStateHandle, context.controllerNumber, context.reservedControllerNumber,
                context.assignedControllerNumber, handler.prefConfig.sendControllers, handler.prefConfig.multiController,
                handler.prefConfig.onscreenController, handler.layoutGamepadUsed, hasBuiltInPlayerOne(),
                handler.gamepad.hasPendingControllerArrival(context.controllerNumber));
        short activeControllerMask = (short) (released & 0xFFFF);
        if ((released & (1 << 16)) != 0) {
            // A cleared slot can retire a pre-launch prepared pad even before arrival.
            // A new active shared slot waits for its capabilities, without retrying on removal.
            handler.conn.sendControllerInput(context.controllerNumber, activeControllerMask,
                    (short) 0,
                    (byte) 0, (byte) 0,
                    (short) 0, (short) 0,
                    (short) 0, (short) 0);
        }
        if ((released & (1 << 17)) != 0) {
            handler.controllerSlotsWithInput.set(context.controllerNumber, 0);
        }
        if ((rumbleRelease & 2) != 0) {
            // The next pad in this slot must not play the old pad's levels when rumble resumes.
            handler.rumbleLevels.remove(context.controllerNumber);
        }
    }

    /** Whether another pad still plays as this controller slot, after {@code released} goes. */
    private boolean slotStillAssigned(ControllerHandler.GenericControllerContext released, int controllerNumber) {
        for (ControllerHandler.InputDeviceContext other : handler.inputDeviceContexts.values()) {
            if (other != released && other.assignedControllerNumber && other.controllerNumber == controllerNumber) {
                return true;
            }
        }
        for (ControllerHandler.UsbDeviceContext other : handler.usbDeviceContexts.values()) {
            if (other != released && other.assignedControllerNumber && other.controllerNumber == controllerNumber) {
                return true;
            }
        }
        return false;
    }

    // Called before sending input but after we've determined that this
    // is definitely a controller (not a keyboard, mouse, or something else)
    void assignControllerNumberIfNeeded(ControllerHandler.GenericControllerContext context) {
        if (context.assignedControllerNumber) {
            return;
        }
        // Perform InputDevice/Binder queries before taking synchronized(handler).
        boolean hasGamepadBtns = false;
        ControllerHandler.InputDeviceContext associatedContext = null;
        if (context instanceof ControllerHandler.InputDeviceContext) {
            ControllerHandler.InputDeviceContext devContext = (ControllerHandler.InputDeviceContext) context;
            hasGamepadBtns = devContext.inputDevice != null && ControllerDeviceClassifier.hasGamepadButtons(devContext.inputDevice);
            if (!devContext.hasJoystickAxes && hasGamepadBtns && handler.prefConfig.multiController) {
                InputDevice associatedDevice = InputDevice.getDevice(devContext.id + 1);
                if (!ControllerDeviceClassifier.isAssociatedJoystick(devContext.inputDevice, associatedDevice)) {
                    associatedDevice = InputDevice.getDevice(devContext.id - 1);
                    if (!ControllerDeviceClassifier.isAssociatedJoystick(devContext.inputDevice, associatedDevice)) {
                        LimeLog.info("No associated joystick device found");
                        associatedDevice = null;
                    }
                }
                if (associatedDevice != null) {
                    associatedContext = getOrCreateInputDeviceContext(associatedDevice.getId(), associatedDevice);
                    if (!associatedContext.assignedControllerNumber) {
                        assignControllerNumberIfNeeded(associatedContext);
                    }
                }
            }
        }
        boolean notifyTypeChanged = false;
        // Input on the main thread and launch preparation on the connection worker can both
        // assign a pad. Without one lock a pad could take two slots, or two pads share one.
        synchronized (handler) {
            if (!context.assignedControllerNumber) {
                if (assignControllerNumber(context, hasGamepadBtns, associatedContext)) {
                    if (context instanceof ControllerHandler.UsbDeviceContext) {
                        ControllerHandler.UsbDeviceContext usbContext = (ControllerHandler.UsbDeviceContext) context;
                        if (handler.deviceSensorManager != null && !handler.motion.deviceSensorsInUse() &&
                                ControllerHandler.takesDeviceMotion(handler.prefConfig.gamepadMotionSensorsFallbackToDevice,
                                        (short) (context.controllerNumber - handler.deviceFallbackSlot),
                                        usbContext.device.getCapabilities(), usbContext.buttonMap)) {
                            usbContext.deviceSensors = handler.deviceSensorManager;
                            notifyTypeChanged = ControllerHandler.motionOverridesType(usbContext.device.getType(), true, usbContext.buttonMap);
                        }
                    }

                    // Report attributes of this new controller to the host before releasing the lock
                    // so concurrent arrival/input readers never observe half-assigned slot state.
                    context.sendControllerArrival();
                }
            }
        }
        if (notifyTypeChanged) {
            handler.mainThreadHandler.post(() -> Toast.makeText(handler.activityContext,
                    R.string.toast_controller_type_changed, Toast.LENGTH_LONG).show());
        }
    }

    private boolean assignControllerNumber(ControllerHandler.GenericControllerContext context,
                                           boolean hasGamepadBtns,
                                           ControllerHandler.InputDeviceContext associatedDeviceContext) {
        if (context instanceof ControllerHandler.InputDeviceContext) {
            ControllerHandler.InputDeviceContext devContext = (ControllerHandler.InputDeviceContext) context;

            LimeLog.info(devContext.name + " (" + context.id + ") needs a controller number assigned");
            ControllerHandler.InputDeviceContext builtInPad = handler.onscreenOwnPlayerSession && !devContext.external && !devContext.hasJoystickAxes ?
                    builtInPad(true) : null;
            int builtInSlot = builtInPad != null ? builtInPad.controllerNumber : -1;
            boolean hasAdjacentAssoc = handler.prefConfig.multiController && !devContext.hasJoystickAxes && hasGamepadBtns &&
                    associatedDeviceContext != null;
            int decision = NightjarInputJni.assignInputDeviceSlot(
                    handler.slotStateHandle, devContext.external, devContext.hasJoystickAxes,
                    hasGamepadBtns, hasAdjacentAssoc, handler.prefConfig.multiController, builtInSlot);
            if ((decision & (1 << 10)) != 0) {
                context.controllerNumber = 0;
                if (associatedDeviceContext != null) {
                    if (!associatedDeviceContext.assignedControllerNumber) {
                        return false;
                    }
                    context.controllerNumber = associatedDeviceContext.controllerNumber;
                    LimeLog.info("Propagated controller number from " + associatedDeviceContext.name);
                }
                else {
                    int fb = NightjarInputJni.resolveAssociatedFallbackSlot(handler.slotStateHandle);
                    if ((fb & (1 << 9)) == 0) {
                        return false;
                    }
                    context.controllerNumber = (short) (fb & 0xFF);
                    if ((fb & (1 << 8)) != 0) {
                        context.reservedControllerNumber = true;
                        handler.currentControllers |= (1 << context.controllerNumber);
                        handler.initialControllers &= ~(1 << context.controllerNumber);
                    }
                }
            }
            else if ((decision & (1 << 9)) == 0) {
                return false;
            }
            else {
                context.controllerNumber = (short) (decision & 0xFF);
                if ((decision & (1 << 8)) != 0) {
                    context.reservedControllerNumber = true;
                    handler.currentControllers |= (1 << context.controllerNumber);
                    handler.initialControllers &= ~(1 << context.controllerNumber);
                }
            }

            // If the gamepad doesn't have motion sensors, use the on-device sensors as a fallback for player 1
            if (handler.prefConfig.gamepadMotionSensorsFallbackToDevice && context.controllerNumber == handler.deviceFallbackSlot && devContext.sensorManager == null &&
                    !ControllerHandler.dropsMotionAndTouchpad(devContext.buttonMap) && !handler.motion.deviceSensorsInUse()) {
                devContext.sensorManager = handler.deviceSensorManager;
            }
        }
        else {
            int decision = NightjarInputJni.assignUsbDeviceSlot(handler.slotStateHandle, handler.prefConfig.multiController);
            if ((decision & (1 << 9)) == 0) {
                return false;
            }
            context.controllerNumber = (short) (decision & 0xFF);
            if ((decision & (1 << 8)) != 0) {
                context.reservedControllerNumber = true;
                handler.currentControllers |= (1 << context.controllerNumber);
                handler.initialControllers &= ~(1 << context.controllerNumber);
            }
        }

        LimeLog.info("Assigned as controller " + context.controllerNumber);
        context.assignedControllerNumber = true;
        return true;
    }

    // Slot assignment changes both masks under this lock, on the main thread or the connection
    // worker. Read them under it too, so input never carries a mask from mid-assignment.
    boolean sharesSlotWithExternalPad(ControllerHandler.GenericControllerContext builtIn) {
        for (ControllerHandler.InputDeviceContext context : handler.inputDeviceContexts.values()) {
            if (context != builtIn && context.external && context.assignedControllerNumber &&
                    context.controllerNumber == builtIn.controllerNumber) return true;
        }
        for (ControllerHandler.UsbDeviceContext context : handler.usbDeviceContexts.values()) {
            if (context.assignedControllerNumber && context.controllerNumber == builtIn.controllerNumber) return true;
        }
        return false;
    }

    // An external pad sharing the slot left: built-in controls there take over the slot's profile.
    void announceBuiltInControls(short slot) {
        if (!handler.connectionStarted || handler.stopped || !handler.conn.canSendControllerInput()) return;
        for (ControllerHandler.InputDeviceContext context : handler.inputDeviceContexts.values()) {
            if (!context.external && context.assignedControllerNumber && context.controllerNumber == slot) {
                context.sendControllerArrival();
                return;
            }
        }
    }

    boolean hasBuiltInPlayerOne() {
        for (ControllerHandler.InputDeviceContext context : handler.inputDeviceContexts.values()) {
            if (context.assignedControllerNumber && !context.external && context.controllerNumber == 0) return true;
        }
        return false;
    }

    boolean carriesTouchpadButton(ControllerHandler.InputDeviceContext context) {
        if (!handler.touchpadButtonSession || context.controllerNumber != 0 || context.buttonMap.isConnectAsPinned()) return false;
        if (!context.external) return true;
        // Decided once, so the pad's type does not flip when another pad joins later.
        if (context.handheldControls == null) context.handheldControls = handler.classifier.isHandheldControls(context.inputDevice);
        return context.handheldControls;
    }

    boolean announcesOnscreenPad() {
        boolean hasSlotZeroDevice = false;
        if (handler.prefConfig.sendControllers) {
            for (ControllerHandler.InputDeviceContext context : handler.inputDeviceContexts.values()) {
                if (context.assignedControllerNumber && context.controllerNumber == 0) {
                    hasSlotZeroDevice = true;
                    break;
                }
            }
            if (!hasSlotZeroDevice) {
                for (ControllerHandler.UsbDeviceContext context : handler.usbDeviceContexts.values()) {
                    if (context.assignedControllerNumber && context.controllerNumber == 0) {
                        hasSlotZeroDevice = true;
                        break;
                    }
                }
            }
        }
        return NightjarInputJni.announcesOnscreenPad(
                handler.slotStateHandle, handler.prefConfig.sendControllers, handler.prefConfig.onscreenController,
                handler.layoutGamepadUsed, hasSlotZeroDevice);
    }

    void sendOnscreenPadArrival() {
        handler.conn.sendControllerArrivalEvent((byte) 0, handler.getActiveControllerMask(), onscreenPadType(),
                onscreenPadButtons(), ControllerHandler.onscreenPadCapabilities(handler.steamControlsSession));
    }

    // Its own player without the Touchpad button: the host's default pad, announced for its number.
    byte onscreenPadType() {
        return NightjarInputJni.onscreenPadType(handler.touchpadButtonSession);
    }

    int onscreenPadButtons() {
        return ControllerHandler.onscreenPadButtons(handler.touchpadButtonSession, handler.steamControlsSession);
    }

    /** A pad's arrival capabilities with the on-screen Steam Controller controls it carries; a pin to Xbox still drops the touchpad. */
    short withSteamControls(short controllerNumber, short capabilities, ControllerButtonMap map) {
        return NightjarInputJni.withSteamControlsCapabilities(
                handler.steamControlsSession, handler.onscreenOwnPlayerSession, controllerNumber, capabilities, map.connectAs());
    }

    int withSteamControlButtons(short controllerNumber, int buttons) {
        return NightjarInputJni.withSteamControlsButtons(
                handler.steamControlsSession, handler.onscreenOwnPlayerSession, controllerNumber, buttons);
    }

    short getActiveControllerMask() {
        return NightjarInputJni.activeControllerMask(
                handler.slotStateHandle, handler.prefConfig.sendControllers, handler.prefConfig.multiController,
                handler.prefConfig.onscreenController, handler.layoutGamepadUsed, hasBuiltInPlayerOne());
    }

    // Called only on the connection worker. Give the USB service time to finish its
    // permission/driver handover before the host starts a game that caches devices.
    String prepareControllersForLaunch() {
        if (!handler.conn.canSendControllerInput()) return "";
        long deadline = android.os.SystemClock.uptimeMillis() + 2500;
        do {
            int attachedDualSenses = handler.prefConfig.usbDualSenseHaptics ? countAttachedDualSenses() : 0;
            synchronized (handler) {
                if (handler.stopped || !handler.conn.canSendControllerInput()) return "";
                int handedOver = 0;
                for (ControllerHandler.UsbDeviceContext context : handler.usbDeviceContexts.values()) {
                    if (ControllerDeviceClassifier.isDualSense(context.vendorId, context.productId)) handedOver++;
                }
                // Only a DualSense the USB service has not handed over yet can still become ready.
                if (handedOver >= attachedDualSenses) break;
            }
            try { Thread.sleep(50); }
            catch (InterruptedException e) { Thread.currentThread().interrupt(); return ""; }
        } while (android.os.SystemClock.uptimeMillis() < deadline);
        // Enumerate and prepare devices outside synchronized(handler) to avoid Binder calls under lock.
        prepareInputDevices();
        synchronized (handler) {
            if (handler.stopped || !handler.conn.canSendControllerInput()) return "";
            String[] controllers = new String[ControllerHandler.MAX_GAMEPADS];
            // Controllers Off: no physical pad is announced, so the PC makes no pad that never sends.
            boolean physical = handler.prefConfig.sendControllers;
            for (ControllerHandler.InputDeviceContext context : physical ? handler.inputDeviceContexts.values() : Collections.<ControllerHandler.InputDeviceContext>emptyList()) {
                // Built-in controls share player 1 with an external pad; the external pad's type wins.
                if (context.assignedControllerNumber &&
                        (context.external || controllers[context.controllerNumber] == null)) {
                    controllers[context.controllerNumber] = launchController(context.controllerNumber,
                            context.reportedType, context.reportedCapabilities, context.supportedButtonFlags);
                }
            }
            for (ControllerHandler.UsbDeviceContext context : handler.usbDeviceContexts.values()) {
                handler.assignControllerNumberIfNeeded(context);
                if (!physical || !context.assignedControllerNumber) continue;
                short caps = context.capabilities();
                if (!handler.prefConfig.gamepadMotionSensors) caps &= ~(MoonBridge.LI_CCAP_ACCEL | MoonBridge.LI_CCAP_GYRO);
                controllers[context.controllerNumber] = launchController(context.controllerNumber,
                        context.arrivalType(), withSteamControls(context.controllerNumber, caps, context.buttonMap),
                        withSteamControlButtons(context.controllerNumber,
                                context.device.getSupportedButtonFlags() | context.buttonMap.targetButtonFlags()));
            }
            if (controllers[0] == null && announcesOnscreenPad()) {
                controllers[0] = launchController((short) 0, onscreenPadType(),
                        ControllerHandler.onscreenPadCapabilities(handler.steamControlsSession), onscreenPadButtons());
            }
            StringBuilder result = new StringBuilder();
            for (String controller : controllers) {
                if (controller == null) continue;
                if (result.length() != 0) result.append(',');
                result.append(controller);
            }
            return result.toString();
        }
    }

    private static String launchController(short slot, byte type, short capabilities, int buttons) {
        return NightjarInputJni.formatLaunchController(slot, type, capabilities, buttons);
    }

    ControllerHandler.InputDeviceContext getOrCreateInputDeviceContext(int deviceId, InputDevice device) {
        ControllerHandler.InputDeviceContext existing = handler.inputDeviceContexts.get(deviceId);
        if (existing != null) {
            return existing;
        }
        // Build outside the map bin lock so Binder calls do not stall concurrent map readers.
        ControllerHandler.InputDeviceContext created = handler.createInputDeviceContextForDevice(device);
        ControllerHandler.InputDeviceContext raced = handler.inputDeviceContexts.putIfAbsent(deviceId, created);
        return raced != null ? raced : created;
    }

    ControllerHandler.InputDeviceContext prepareInputDevice(int deviceId) {
        InputDevice device = InputDevice.getDevice(deviceId);
        if (device == null || (!ControllerDeviceClassifier.hasJoystickAxes(device) && !ControllerDeviceClassifier.hasGamepadButtons(device))) return null;
        // Built-in or virtual devices that merely advertise gamepad keys (#29, #30) get a
        // number when they first send gamepad input, never ahead of the real pads.
        if (!ControllerDeviceClassifier.hasJoystickAxes(device) && !ControllerDeviceClassifier.isExternal(device)) return null;
        ControllerHandler.InputDeviceContext context = getOrCreateInputDeviceContext(deviceId, device);
        handler.assignControllerNumberIfNeeded(context);
        return context;
    }

    void prepareInputDevices() {
        // External pads with sticks first, then button-only ones (a remote advertising gamepad
        // keys lands after any real pad), then USB pads, then built-in controls, which share
        // player 1 and so must find the external pads already in place.
        for (int pass = 0; pass < 3; pass++) {
            if (pass == 2) {
                for (ControllerHandler.UsbDeviceContext context : handler.usbDeviceContexts.values()) {
                    handler.assignControllerNumberIfNeeded(context);
                }
            }
            for (int id : InputDevice.getDeviceIds()) {
                InputDevice device = InputDevice.getDevice(id);
                if (device == null) continue;
                boolean external = ControllerDeviceClassifier.isExternal(device);
                boolean sticks = ControllerDeviceClassifier.hasJoystickAxes(device);
                if (NightjarInputJni.launchPassForInputDevice(external, sticks) == pass) {
                    prepareInputDevice(id);
                }
            }
        }
    }

    private int countAttachedDualSenses() {
        UsbManager usbManager = (UsbManager) handler.activityContext.getSystemService(Context.USB_SERVICE);
        if (usbManager == null) return 0;
        int count = 0;
        for (UsbDevice device : usbManager.getDeviceList().values()) {
            // A pad still at the USB permission prompt, or whose driver failed, cannot arrive in time.
            if (DualSenseController.canClaimDevice(device) && usbManager.hasPermission(device) &&
                    !handler.failedUsbDevices.contains(device.getDeviceId())) count++;
        }
        return count;
    }

    void resendControllerArrivals() {
        if (handler.stopped || !handler.conn.canSendControllerInput()) return;
        handler.connectionStarted = true;
        if (handler.onscreenOwnPlayerSession) {
            // Before any pad, so the PC's own numbering starts with it too. The neutral state is
            // for hosts that plug a controller at its first input instead of at its arrival.
            sendOnscreenPadArrival();
            handler.sendControllerInputPacket(handler.defaultContext);
        }
        for (ControllerHandler.InputDeviceContext context : handler.inputDeviceContexts.values()) {
            if (context.assignedControllerNumber) context.sendControllerArrival();
        }
        for (ControllerHandler.UsbDeviceContext context : handler.usbDeviceContexts.values()) {
            if (context.assignedControllerNumber) context.sendControllerArrival();
        }
        // Newly assigned pads announce themselves here.
        prepareInputDevices();
        if (!handler.onscreenOwnPlayerSession && announcesOnscreenPad()) sendOnscreenPadArrival();
        for (ControllerHandler.InputDeviceContext context : handler.inputDeviceContexts.values()) {
            if (context.assignedControllerNumber) handler.sendControllerInputPacket(context);
        }
        for (ControllerHandler.UsbDeviceContext context : handler.usbDeviceContexts.values()) {
            handler.sendControllerInputPacket(context);
        }
    }
}
