package com.limelight.binding.input;

import android.annotation.TargetApi;
import android.content.Context;
import android.hardware.Sensor;
import android.hardware.input.InputManager;
import android.hardware.lights.Light;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbManager;
import android.os.Build;
import android.os.VibratorManager;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;

import com.limelight.LimeLog;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.binding.input.driver.UsbDriverService;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarInputJni;
import com.limelight.preferences.PreferenceConfiguration;

import java.lang.reflect.InvocationTargetException;

/**
 * Classifies Android {@link InputDevice} and USB controller hardware, probes joystick axes,
 * vibrators, sensors, and quirks, and builds {@link ControllerHandler.InputDeviceContext} and
 * {@link ControllerHandler.UsbDeviceContext} instances for {@link ControllerHandler}.
 */
final class ControllerDeviceClassifier {
    private final ControllerHandler handler;

    ControllerDeviceClassifier(ControllerHandler handler) {
        this.handler = handler;
    }

    static InputDevice.MotionRange getMotionRangeForJoystickAxis(InputDevice dev, int axis) {
        InputDevice.MotionRange range;

        // First get the axis for SOURCE_JOYSTICK
        range = dev.getMotionRange(axis, InputDevice.SOURCE_JOYSTICK);
        if (range == null) {
            // Now try the axis for SOURCE_GAMEPAD
            range = dev.getMotionRange(axis, InputDevice.SOURCE_GAMEPAD);
        }

        return range;
    }

    static boolean isController(InputDevice device) {
        return device != null && hasJoystickAxes(device);
    }

    static boolean hasJoystickAxes(InputDevice device) {
        return (device.getSources() & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK &&
                getMotionRangeForJoystickAxis(device, MotionEvent.AXIS_X) != null &&
                getMotionRangeForJoystickAxis(device, MotionEvent.AXIS_Y) != null;
    }

    static boolean hasGamepadButtons(InputDevice device) {
        return (device.getSources() & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD;
    }

    /**
     * Whether the device is a controller at all: gamepad buttons or sticks. A fight stick or an NES-style pad with
     * only a hat and buttons has no X/Y sticks and still counts.
     */
    static boolean isGamepadDevice(InputDevice device) {
        return hasJoystickAxes(device) || hasGamepadButtons(device);
    }

    /**
     * Whether this is an ordinary keyboard rather than a pad that also types: its layout is alphabetic and it has
     * neither gamepad buttons nor the gamepad's A button.
     */
    static boolean isAlphabeticKeyboard(InputDevice device) {
        return device.getKeyboardType() == InputDevice.KEYBOARD_TYPE_ALPHABETIC &&
                !hasGamepadButtons(device) && !device.hasKeys(KeyEvent.KEYCODE_BUTTON_A)[0];
    }

    /** Fire TV and similar remotes have a D-pad, but no gamepad buttons or joystick axes. */
    static boolean isRemoteControlDevice(InputDevice device) {
        return device != null && device.supportsSource(InputDevice.SOURCE_DPAD) &&
                device.getKeyboardType() != InputDevice.KEYBOARD_TYPE_ALPHABETIC &&
                !hasGamepadButtons(device) && !hasJoystickAxes(device);
    }

    static boolean isRemoteControlEvent(KeyEvent event) {
        InputDevice device = event.getDevice();
        if (device != null) return isRemoteControlDevice(device);
        // A remote can disappear between the press and release; preserve its event source.
        return event.isFromSource(InputDevice.SOURCE_DPAD) &&
                !event.isFromSource(InputDevice.SOURCE_GAMEPAD) &&
                !event.isFromSource(InputDevice.SOURCE_JOYSTICK);
    }

    static boolean isGameControllerDevice(InputDevice device) {
        if (device == null) {
            return true;
        }

        if (isRemoteControlDevice(device)) return false;

        if (hasJoystickAxes(device) || hasGamepadButtons(device)) {
            // Has real joystick axes or gamepad buttons
            return true;
        }

        // HACK for https://issuetracker.google.com/issues/163120692
        if (Build.VERSION.SDK_INT == Build.VERSION_CODES.R) {
            if (device.getId() == -1) {
                // This "virtual" device could be input from any of the attached devices.
                // Look to see if any gamepads are connected.
                int[] ids = InputDevice.getDeviceIds();
                for (int id : ids) {
                    InputDevice dev = InputDevice.getDevice(id);
                    if (dev == null) {
                        // This device was removed during enumeration
                        continue;
                    }

                    // If there are any gamepad devices connected, we'll
                    // report that this virtual device is a gamepad.
                    if (hasJoystickAxes(dev) || hasGamepadButtons(dev)) {
                        return true;
                    }
                }
            }
        }

        // Otherwise, we'll try anything that claims to be a non-alphabetic keyboard
        return device.getKeyboardType() != InputDevice.KEYBOARD_TYPE_ALPHABETIC;
    }

    static short getAttachedControllerMask(Context context) {
        return attachedMasks(context)[0];
    }

    /** The slots of {@link #getAttachedControllerMask} that hold PlayStation controllers (psmap). */
    static short getAttachedPlayStationMask(Context context) {
        return attachedMasks(context)[1];
    }

    private static boolean isPlayStation(int vendorId, int productId) {
        return vendorId == 0x054c || MoonBridge.guessControllerType(vendorId, productId) == MoonBridge.LI_CTYPE_PS;
    }

    /** {every counted controller's slot, the PlayStation ones among them}. */
    private static short[] attachedMasks(Context context) {
        int count = 0;
        short mask = 0;
        short playStation = 0;

        // Count all input devices that are gamepads
        InputManager im = (InputManager) context.getSystemService(Context.INPUT_SERVICE);
        for (int id : im.getInputDeviceIds()) {
            InputDevice dev = im.getInputDevice(id);
            if (dev == null) {
                continue;
            }

            if (hasJoystickAxes(dev)) {
                LimeLog.info("Counting InputDevice: " + dev.getName());
                if (isPlayStation(dev.getVendorId(), dev.getProductId())) {
                    playStation |= 1 << count;
                }
                mask |= 1 << count++;
            }
        }

        // Count all USB devices that match our drivers
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(context);
        if (config.usbDriver || config.usbDualSense || config.usbSwitchPro) {
            UsbManager usbManager = (UsbManager) context.getSystemService(Context.USB_SERVICE);
            if (usbManager != null) {
                for (UsbDevice dev : usbManager.getDeviceList().values()) {
                    // We explicitly check not to claim devices that appear as InputDevices
                    // otherwise we will double count them.
                    if (UsbDriverService.shouldClaimDevice(dev, config) &&
                            !UsbDriverService.isRecognizedInputDevice(dev)) {
                        LimeLog.info("Counting UsbDevice: " + dev.getDeviceName());
                        if (isPlayStation(dev.getVendorId(), dev.getProductId())) {
                            playStation |= 1 << count;
                        }
                        mask |= 1 << count++;
                    }
                }
            }
        }

        if (ControllerHandler.onscreenOwnsPlayerOne(config)) {
            // The pads counted above arrive from controller 2 up.
            mask = (short) (mask << 1);
            playStation = (short) (playStation << 1);
        }

        if (config.onscreenController) {
            LimeLog.info("Counting OSC gamepad");
            mask |= 1;
        }

        LimeLog.info("Enumerated " + count + " gamepads");
        return new short[] {mask, playStation};
    }

    static boolean isAssociatedJoystick(InputDevice originalDevice, InputDevice possibleAssociatedJoystick) {
        if (possibleAssociatedJoystick == null) {
            return false;
        }

        // This can't be an associated joystick if it's not a joystick
        if ((possibleAssociatedJoystick.getSources() & InputDevice.SOURCE_JOYSTICK) != InputDevice.SOURCE_JOYSTICK) {
            return false;
        }

        // Make sure the device names *don't* match in order to prevent us from accidentally matching
        // on another of the exact same device.
        if (possibleAssociatedJoystick.getName().equals(originalDevice.getName())) {
            return false;
        }

        // Make sure the descriptor matches. This can match in cases where two of the exact same
        // input device are connected, so we perform the name check to exclude that case.
        if (!possibleAssociatedJoystick.getDescriptor().equals(originalDevice.getDescriptor())) {
            return false;
        }

        return true;
    }

    static boolean hasButtonUnderTouchpad(InputDevice dev, byte type) {
        // It has to have a touchpad to have a button under it
        if ((dev.getSources() & InputDevice.SOURCE_TOUCHPAD) != InputDevice.SOURCE_TOUCHPAD) {
            return false;
        }

        // Landroid/view/InputDevice;->hasButtonUnderPad()Z is blocked after O
        if (Build.VERSION.SDK_INT <= Build.VERSION_CODES.O) {
            try {
                return (Boolean) dev.getClass().getMethod("hasButtonUnderPad").invoke(dev);
            } catch (NoSuchMethodException e) {
                e.printStackTrace();
            } catch (IllegalAccessException e) {
                e.printStackTrace();
            } catch (InvocationTargetException e) {
                e.printStackTrace();
            } catch (ClassCastException e) {
                e.printStackTrace();
            }
        }

        // We can't use the platform API, so we'll have to just guess based on the gamepad type.
        // If this is a PlayStation controller with a touchpad, we know it has a clickpad.
        return type == MoonBridge.LI_CTYPE_PS;
    }

    static boolean isExternal(InputDevice dev) {
        int override = NightjarInputJni.isExternalOverride(Build.MODEL, dev.getName());
        if (override >= 0) {
            return override == 1;
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            // Landroid/view/InputDevice;->isExternal()Z is officially public on Android Q
            return dev.isExternal();
        }
        else {
            try {
                // Landroid/view/InputDevice;->isExternal()Z is on the light graylist in Android P
                return (Boolean) dev.getClass().getMethod("isExternal").invoke(dev);
            } catch (NoSuchMethodException e) {
                e.printStackTrace();
            } catch (IllegalAccessException e) {
                e.printStackTrace();
            } catch (InvocationTargetException e) {
                e.printStackTrace();
            } catch (ClassCastException e) {
                e.printStackTrace();
            }
        }

        // Answer true if we don't know
        return true;
    }

    static boolean isDualSense(int vendorId, int productId) {
        return vendorId == 0x054c && (productId == 0x0ce6 || productId == 0x0df2);
    }

    @TargetApi(31)
    static boolean hasDualAmplitudeControlledRumbleVibrators(VibratorManager vm) {
        int[] vibratorIds = vm.getVibratorIds();

        // There must be exactly 2 vibrators on this device
        if (vibratorIds.length != 2) {
            return false;
        }

        // Both vibrators must have amplitude control
        for (int vid : vibratorIds) {
            if (!vm.getVibrator(vid).hasAmplitudeControl()) {
                return false;
            }
        }

        return true;
    }

    @TargetApi(31)
    static boolean hasQuadAmplitudeControlledRumbleVibrators(VibratorManager vm) {
        int[] vibratorIds = vm.getVibratorIds();

        // There must be exactly 4 vibrators on this device
        if (vibratorIds.length != 4) {
            return false;
        }

        // All vibrators must have amplitude control
        for (int vid : vibratorIds) {
            if (!vm.getVibrator(vid).hasAmplitudeControl()) {
                return false;
            }
        }

        return true;
    }

    boolean hasExternalControllers() {
        for (ControllerHandler.InputDeviceContext context : handler.inputDeviceContexts.values()) {
            // A handheld's own pad is always there; counting it would keep the touch controls off for good. A remote
            // or an ordinary keyboard is not a controller either.
            InputDevice device = context.inputDevice;
            if (device != null && ControllerHandler.countsAsExternalController(context.external, isGamepadDevice(device),
                    isRemoteControlDevice(device), isAlphabeticKeyboard(device), isHandheldControls(device))) {
                return true;
            }
        }
        if (!handler.usbDeviceContexts.isEmpty()) return true;
        // Before the first input event, look at what is attached rather than waiting for it to send one.
        if (handler.inputManager == null) return false;
        for (int id : handler.inputManager.getInputDeviceIds()) {
            InputDevice device = handler.inputManager.getInputDevice(id);
            if (device != null && ControllerHandler.countsAsExternalController(isExternal(device), isGamepadDevice(device),
                    isRemoteControlDevice(device), isAlphabeticKeyboard(device), isHandheldControls(device))) {
                return true;
            }
        }
        return false;
    }

    boolean shouldIgnoreBack(InputDevice dev) {
        boolean external = isExternal(dev);
        boolean internalGamepad = false;
        boolean internalSelect = false;
        if (!external) {
            boolean[] internal = handler.findInternalGamepadAndSelect();
            internalGamepad = internal[0];
            internalSelect = internal[1];
        }
        return NightjarInputJni.shouldIgnoreBack(
                dev.getName(), hasJoystickAxes(dev), hasGamepadButtons(dev),
                external, internalGamepad, internalSelect);
    }

    boolean[] findInternalGamepadAndSelect() {
        InputManager im = (InputManager) handler.activityContext.getSystemService(Context.INPUT_SERVICE);

        boolean foundInternalGamepad = false;
        boolean foundInternalSelect = false;
        boolean foundGamepad = false;
        for (int id : im.getInputDeviceIds()) {
            InputDevice currentDev = im.getInputDevice(id);
            if (currentDev == null) {
                continue;
            }
            if (!currentDev.isVirtual() && hasGamepadButtons(currentDev)) {
                foundGamepad = true;
            }

            // Ignore external devices
            if (isExternal(currentDev)) {
                continue;
            }

            // Note that we are explicitly NOT excluding the device a Back came from here,
            // since the other gamepad buttons may be on that device and that's fine.
            if (currentDev.hasKeys(KeyEvent.KEYCODE_BUTTON_SELECT)[0]) {
                foundInternalSelect = true;
            }

            // We don't check KEYCODE_BUTTON_A here, since the Shield Android TV has a
            // virtual mouse device that claims to have KEYCODE_BUTTON_A. Instead, we rely
            // on the SOURCE_GAMEPAD flag to be set on gamepad devices.
            if (hasGamepadButtons(currentDev)) {
                foundInternalGamepad = true;
            }
        }
        return new boolean[] {foundInternalGamepad, foundInternalSelect, foundGamepad};
    }

    boolean backIsSpareButton() {
        boolean[] found = handler.findInternalGamepadAndSelect();
        return ControllerHandler.backIsSpareButton(found[0], found[1], found[2]);
    }

    /** Whether a gamepad Android lists as external is the handheld's own controls. */
    boolean isHandheldControls(InputDevice device) {
        if (device == null) return false;
        int gamepads = 0;
        for (int id : InputDevice.getDeviceIds()) {
            InputDevice other = InputDevice.getDevice(id);
            if (other != null && !other.isVirtual() && hasJoystickAxes(other)) gamepads++;
        }
        boolean attachedUsb = false;
        UsbManager usbManager = (UsbManager) handler.activityContext.getSystemService(Context.USB_SERVICE);
        if (usbManager != null) {
            for (UsbDevice usb : usbManager.getDeviceList().values()) {
                if (usb.getVendorId() == device.getVendorId() && usb.getProductId() == device.getProductId()) {
                    attachedUsb = true;
                }
            }
        }
        return ControllerHandler.looksLikeHandheldControls(device.getName(), device.getVendorId(), gamepads == 1, attachedUsb);
    }

    ControllerHandler.UsbDeviceContext createUsbDeviceContextForDevice(AbstractController device) {
        ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();

        context.id = device.getControllerId();
        context.device = device;
        context.external = true;

        context.vendorId = device.getVendorId();
        context.productId = device.getProductId();
        context.buttonMap = ControllerButtonMap.load(handler.activityContext,
                ControllerButtonMap.deviceKey(context.vendorId, context.productId, null));
        context.axisMap = ControllerAxisMap.load(handler.activityContext,
                ControllerButtonMap.deviceKey(context.vendorId, context.productId, null));
        context.gyroMapping = GyroMappingPreferences.transformerForModel(handler.activityContext,
                ControllerButtonMap.deviceKey(context.vendorId, context.productId, null));

        context.leftStickDeadzoneRadius = (float) handler.stickDeadzone;
        context.rightStickDeadzoneRadius = (float) handler.stickDeadzone;
        context.triggerDeadzone = handler.triggerDeadzone;

        return context;
    }

    ControllerHandler.InputDeviceContext createInputDeviceContextForDevice(InputDevice dev) {
        ControllerHandler.InputDeviceContext context = handler.new InputDeviceContext();
        String devName = dev.getName();

        LimeLog.info("Creating controller context for device: " + devName);
        LimeLog.info("Vendor ID: " + dev.getVendorId());
        LimeLog.info("Product ID: " + dev.getProductId());
        LimeLog.info(dev.toString());

        context.inputDevice = dev;
        context.name = devName;
        context.id = dev.getId();
        context.external = isExternal(dev);

        context.vendorId = dev.getVendorId();
        context.productId = dev.getProductId();
        context.buttonMap = ControllerButtonMap.load(handler.activityContext,
                ControllerButtonMap.deviceKey(context.vendorId, context.productId, devName));
        context.axisMap = ControllerAxisMap.load(handler.activityContext,
                ControllerButtonMap.deviceKey(context.vendorId, context.productId, devName));

        // These aren't always present in the Android key layout files, so they won't show up
        // in our normal InputDevice.hasKeys() probing.
        context.hasPaddles = MoonBridge.guessControllerHasPaddles(context.vendorId, context.productId);
        context.hasShare = MoonBridge.guessControllerHasShareButton(context.vendorId, context.productId);

        configureVibrators(context, dev);
        configureMotionSensorsAndLights(context, dev);
        configureAxesAndQuirks(context, dev, devName);

        return context;
    }

    private void configureVibrators(ControllerHandler.InputDeviceContext context, InputDevice dev) {
        // Try to use the InputDevice's associated vibrators first
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && hasQuadAmplitudeControlledRumbleVibrators(dev.getVibratorManager())) {
            context.vibratorManager = dev.getVibratorManager();
            context.quadVibrators = true;
        }
        else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && hasDualAmplitudeControlledRumbleVibrators(dev.getVibratorManager())) {
            context.vibratorManager = dev.getVibratorManager();
            context.quadVibrators = false;
        }
        else if (dev.getVibrator().hasVibrator()) {
            context.vibrator = dev.getVibrator();
        }
        else if (!context.external) {
            // If this is an internal controller, try to use the device's vibrator
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && hasQuadAmplitudeControlledRumbleVibrators(handler.deviceVibratorManager)) {
                context.vibratorManager = handler.deviceVibratorManager;
                context.quadVibrators = true;
            }
            else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && hasDualAmplitudeControlledRumbleVibrators(handler.deviceVibratorManager)) {
                context.vibratorManager = handler.deviceVibratorManager;
                context.quadVibrators = false;
            }
            else if (handler.deviceVibrator.hasVibrator()) {
                context.vibrator = handler.deviceVibrator;
            }
        }
    }

    private void configureMotionSensorsAndLights(ControllerHandler.InputDeviceContext context, InputDevice dev) {
        // On Android 12, we can try to use the InputDevice's sensors. This may not work if the
        // Linux kernel version doesn't have motion sensor support, which is common for third-party
        // gamepads.
        //
        // Android 12 has a bug that causes InputDeviceSensorManager to cause a NPE on a background
        // thread due to bad error checking in InputListener callbacks. InputDeviceSensorManager is
        // created upon the first call to InputDevice.getSensorManager(), so we avoid calling this
        // on Android 12 unless we have a gamepad that could plausibly have motion sensors.
        // https://cs.android.com/android/_/android/platform/frameworks/base/+/8970010a5e9f3dc5c069f56b4147552accfcbbeb
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            if ((Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU || context.vendorId == 0x054c || context.vendorId == 0x057e) && // Sony or Nintendo
                    handler.prefConfig.gamepadMotionSensors && !ControllerHandler.dropsMotionAndTouchpad(context.buttonMap)) {
                if (dev.getSensorManager().getDefaultSensor(Sensor.TYPE_ACCELEROMETER) != null || dev.getSensorManager().getDefaultSensor(Sensor.TYPE_GYROSCOPE) != null) {
                    context.sensorManager = dev.getSensorManager();
                }
            }
        }

        // Check if this device has a usable RGB LED and cache that result
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            for (Light light : dev.getLightsManager().getLights()) {
                if (light.hasRgbControl()) {
                    context.hasRgbLed = true;
                    break;
                }
            }

            // Light.hasRgbControl() was totally broken prior to Android 14.
            // It always returned true because LIGHT_CAPABILITY_RGB was defined as 0,
            // so we will just guess RGB is supported if it's a PlayStation controller.
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE && context.vendorId != 0x054c) {
                context.hasRgbLed = false;
            }
        }
    }

    private void configureAxesAndQuirks(ControllerHandler.InputDeviceContext context, InputDevice dev, String devName) {
        // Detect if the gamepad has Mode and Select buttons according to the Android key layouts.
        // We do this first because other codepaths below may override these defaults.
        boolean[] buttons = dev.hasKeys(KeyEvent.KEYCODE_BUTTON_MODE, KeyEvent.KEYCODE_BUTTON_SELECT, KeyEvent.KEYCODE_BACK, 0);
        context.hasMode = buttons[0];
        context.hasSelect = buttons[1] || buttons[2];
        boolean[] dpadButtons = dev.hasKeys(KeyEvent.KEYCODE_DPAD_UP, KeyEvent.KEYCODE_DPAD_DOWN,
                KeyEvent.KEYCODE_DPAD_LEFT, KeyEvent.KEYCODE_DPAD_RIGHT);
        context.hasHardwareDpadKeys = dpadButtons[0] || dpadButtons[1] || dpadButtons[2] || dpadButtons[3];

        context.touchpadXRange = dev.getMotionRange(MotionEvent.AXIS_X, InputDevice.SOURCE_TOUCHPAD);
        context.touchpadYRange = dev.getMotionRange(MotionEvent.AXIS_Y, InputDevice.SOURCE_TOUCHPAD);
        context.touchpadPressureRange = dev.getMotionRange(MotionEvent.AXIS_PRESSURE, InputDevice.SOURCE_TOUCHPAD);

        context.leftStickXAxis = MotionEvent.AXIS_X;
        context.leftStickYAxis = MotionEvent.AXIS_Y;
        if (getMotionRangeForJoystickAxis(dev, context.leftStickXAxis) != null &&
                getMotionRangeForJoystickAxis(dev, context.leftStickYAxis) != null) {
            // This is a gamepad
            handler.hasGameController = true;
            context.hasJoystickAxes = true;
        }

        InputDevice.MotionRange leftTriggerRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_LTRIGGER);
        InputDevice.MotionRange rightTriggerRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_RTRIGGER);
        InputDevice.MotionRange brakeRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_BRAKE);
        InputDevice.MotionRange gasRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_GAS);
        InputDevice.MotionRange throttleRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_THROTTLE);
        InputDevice.MotionRange rxRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_RX);
        InputDevice.MotionRange ryRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_RY);
        InputDevice.MotionRange zRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_Z);
        InputDevice.MotionRange rzRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_RZ);
        InputDevice.MotionRange hatXRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_HAT_X);
        InputDevice.MotionRange hatYRange = getMotionRangeForJoystickAxis(dev, MotionEvent.AXIS_HAT_Y);
        if (hatXRange != null && hatYRange != null) {
            context.hatXAxis = MotionEvent.AXIS_HAT_X;
            context.hatYAxis = MotionEvent.AXIS_HAT_Y;
        }
        boolean hasRxRy = rxRange != null && ryRange != null;
        boolean hasButtonC = hasRxRy && devName != null && dev.getVendorId() == 0x054c &&
                dev.hasKeys(KeyEvent.KEYCODE_BUTTON_C)[0];
        boolean hasStartOrMenu = true;
        if (devName != null && devName.contains("ASUS Gamepad")) {
            boolean[] hasStartKey = dev.hasKeys(KeyEvent.KEYCODE_BUTTON_START, KeyEvent.KEYCODE_MENU, 0);
            hasStartOrMenu = hasStartKey[0] || hasStartKey[1];
        }
        context.ignoreBack = shouldIgnoreBack(dev);

        int[] classifyInts = new int[6];
        float[] classifyFloat = new float[1];
        NightjarInputJni.classifyInputDevice(
                context.vendorId, context.productId, devName,
                dev.getSources() == (InputDevice.SOURCE_KEYBOARD | InputDevice.SOURCE_MOUSE),
                leftTriggerRange != null && rightTriggerRange != null,
                brakeRange != null && gasRange != null,
                brakeRange != null && throttleRange != null,
                gasRange != null,
                hasRxRy,
                zRange != null && rzRange != null,
                hasButtonC,
                context.hatXAxis != -1 && context.hatYAxis != -1,
                hasStartOrMenu,
                context.hasSelect,
                context.hasMode,
                context.ignoreBack,
                handler.triggerDeadzone,
                classifyInts,
                classifyFloat);
        context.rightStickXAxis = classifyInts[0];
        context.rightStickYAxis = classifyInts[1];
        context.leftTriggerAxis = classifyInts[2];
        context.rightTriggerAxis = classifyInts[3];
        int quirks = classifyInts[4];
        context.isDualShockStandaloneTouchpad = (quirks & NightjarInputJni.QUIRK_DS4_STANDALONE_TOUCHPAD) != 0;
        context.usesLinuxGamepadStandardFaceButtons = (quirks & NightjarInputJni.QUIRK_LINUX_STD_FACE) != 0;
        context.isNonStandardDualShock4 = (quirks & NightjarInputJni.QUIRK_NON_STD_DS4) != 0;
        context.isServal = (quirks & NightjarInputJni.QUIRK_SERVAL) != 0;
        context.isNonStandardXboxBtController = (quirks & NightjarInputJni.QUIRK_NON_STD_XBOX_BT) != 0;
        context.backIsStart = (quirks & NightjarInputJni.QUIRK_BACK_IS_START) != 0;
        context.modeIsSelect = (quirks & NightjarInputJni.QUIRK_MODE_IS_SELECT) != 0;
        context.searchIsMode = (quirks & NightjarInputJni.QUIRK_SEARCH_IS_MODE) != 0;
        int boolBits = classifyInts[5];
        context.triggersIdleNegative = (boolBits & 1) != 0;
        context.hasSelect = (boolBits & 2) != 0;
        context.hasMode = (boolBits & 4) != 0;
        context.triggerDeadzone = classifyFloat[0];

        if (context.leftStickXAxis != -1 && context.leftStickYAxis != -1) {
            context.leftStickDeadzoneRadius = (float) handler.stickDeadzone;
        }
        if (context.rightStickXAxis != -1 && context.rightStickYAxis != -1) {
            context.rightStickDeadzoneRadius = (float) handler.stickDeadzone;
        }

        LimeLog.info("Analog stick deadzone: " + context.leftStickDeadzoneRadius + " " + context.rightStickDeadzoneRadius);
        LimeLog.info("Trigger deadzone: " + context.triggerDeadzone);
    }
}
