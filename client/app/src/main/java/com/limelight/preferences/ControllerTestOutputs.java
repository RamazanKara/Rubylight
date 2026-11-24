package com.limelight.preferences;

import android.content.Context;
import android.hardware.Sensor;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.hardware.lights.Light;
import android.hardware.lights.LightState;
import android.hardware.lights.LightsManager;
import android.hardware.lights.LightsRequest;
import android.os.Build;
import android.os.CombinedVibration;
import android.os.Handler;
import android.os.VibrationEffect;
import android.os.Vibrator;
import android.os.VibratorManager;
import android.view.InputDevice;
import android.view.View;
import android.widget.TextView;

import androidx.annotation.RequiresApi;

import com.google.android.material.chip.Chip;
import com.google.android.material.chip.ChipGroup;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.input.ControllerButtonMap;
import com.limelight.binding.input.ControllerTestModel;
import com.limelight.binding.input.RumblePolicy;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.nvstream.jni.MoonBridge;

import java.util.ArrayList;
import java.util.List;
import java.util.function.Supplier;

/**
 * Hardware output probing, action chip construction, rumble/light control, and motion-sensor
 * registration for {@link ControllerTestActivity}.
 */
final class ControllerTestOutputs {
    private static final int RUMBLE_MS = 600;
    private static final int[] LIGHT_COLORS = {0xFFFF0000, 0xFF00FF00, 0xFF0000FF};
    private static final int[] LIGHT_NAMES = {R.string.controller_test_light_red,
            R.string.controller_test_light_green, R.string.controller_test_light_blue};

    private final Context context;
    private final Handler handler;
    private final SensorEventListener sensorListener;
    private final Supplier<InputDevice> deviceSupplier;
    private final Supplier<AbstractController> usbSupplier;
    private final ChipGroup rumbleActions, lightActions;
    private final TextView rumbleNote, lightsNote;
    private final Runnable rumbleTimeout = this::stopRumble;

    private boolean rumbleDisabledForStreams;
    private boolean motionFallbackToDevice;
    private RumblePolicy.Feel rumbleFeel = RumblePolicy.Feel.STANDARD;

    private VibratorManager vibratorManager;
    private Vibrator vibrator;
    private LightsManager.LightsSession lightsSession;
    private final List<Light> rgbLights = new ArrayList<>();
    private SensorManager sensorManager;
    private Sensor gyroSensor, accelSensor;
    private boolean hasTouchpad, hasMotion;

    ControllerTestOutputs(Context context, Handler handler, SensorEventListener sensorListener,
                          Supplier<InputDevice> deviceSupplier, Supplier<AbstractController> usbSupplier,
                          ChipGroup rumbleActions, ChipGroup lightActions,
                          TextView rumbleNote, TextView lightsNote) {
        this.context = context;
        this.handler = handler;
        this.sensorListener = sensorListener;
        this.deviceSupplier = deviceSupplier;
        this.usbSupplier = usbSupplier;
        this.rumbleActions = rumbleActions;
        this.lightActions = lightActions;
        this.rumbleNote = rumbleNote;
        this.lightsNote = lightsNote;
    }

    void configure(PreferenceConfiguration config) {
        rumbleDisabledForStreams = config.disableRumble;
        motionFallbackToDevice = config.gamepadMotionSensorsFallbackToDevice;
        rumbleFeel = RumblePolicy.Feel.fromKey(config.rumbleFeel);
    }

    boolean hasTouchpad() {
        return hasTouchpad;
    }

    boolean hasMotion() {
        return hasMotion;
    }

    void markHasTouchpad() {
        hasTouchpad = true;
    }

    void cancelRumbleTimeout() {
        handler.removeCallbacks(rumbleTimeout);
    }

    void armUsbMotion(AbstractController usbController) {
        if (usbController == null ||
                (usbController.getCapabilities() & (MoonBridge.LI_CCAP_ACCEL | MoonBridge.LI_CCAP_GYRO)) == 0) {
            return;
        }
        usbController.setMotionEventState(MoonBridge.LI_MOTION_TYPE_ACCEL, (short) 60);
        usbController.setMotionEventState(MoonBridge.LI_MOTION_TYPE_GYRO, (short) 60);
    }

    // Leaves the controller as it was found: no rumble, the system's lights, no sensor listeners.
    void release() {
        stopRumble();
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && lightsSession != null) {
            lightsSession.close();
            lightsSession = null;
        }
        if (sensorManager != null) {
            sensorManager.unregisterListener(sensorListener);
        }
        AbstractController usbController = usbSupplier.get();
        if (usbController != null) {
            usbController.setMotionEventState(MoonBridge.LI_MOTION_TYPE_ACCEL, (short) 0);
            usbController.setMotionEventState(MoonBridge.LI_MOTION_TYPE_GYRO, (short) 0);
        }
    }

    void reset() {
        vibratorManager = null;
        vibrator = null;
        rgbLights.clear();
        if (sensorManager != null) sensorManager.unregisterListener(sensorListener);
        sensorManager = null;
        gyroSensor = accelSensor = null;
        hasTouchpad = hasMotion = false;
    }

    void selectInputDevice(InputDevice dev) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            selectOutputs(dev);
        }
        else if (dev.getVibrator().hasVibrator()) {
            vibrator = dev.getVibrator();
        }
        selectDeviceMotion(dev.getVendorId(), dev.getProductId(), dev.getName());
        hasTouchpad = detectTouchpad(dev);
    }

    void selectUsbController(AbstractController controller) {
        short caps = controller.getCapabilities();
        hasTouchpad = (caps & MoonBridge.LI_CCAP_TOUCHPAD) != 0;
        hasMotion = (caps & (MoonBridge.LI_CCAP_ACCEL | MoonBridge.LI_CCAP_GYRO)) != 0;
        armUsbMotion(controller);
        // As ControllerHandler's takesDeviceMotion for a USB pad without its own sensors.
        selectDeviceMotion(controller.getVendorId(), controller.getProductId(), null);
    }

    // A pad without motion sensors of its own streams this device's, as player 1 does in a stream
    // (the test has one pad, so it shows what that pad gets as player 1). `name` is null for a pad
    // the USB driver runs, as its button map is keyed.
    private void selectDeviceMotion(int vendorId, int productId, String name) {
        SensorManager system = (SensorManager) context.getSystemService(Context.SENSOR_SERVICE);
        if (system == null) return;
        Sensor deviceGyro = system.getDefaultSensor(Sensor.TYPE_GYROSCOPE);
        Sensor deviceAccel = system.getDefaultSensor(Sensor.TYPE_ACCELEROMETER);
        boolean pinnedXbox = ControllerButtonMap.load(context,
                ControllerButtonMap.deviceKey(vendorId, productId, name))
                .connectAs() == MoonBridge.LI_CTYPE_XBOX;
        if (ControllerTestModel.usesDeviceMotion(hasMotion, motionFallbackToDevice, pinnedXbox,
                deviceGyro != null || deviceAccel != null)) {
            sensorManager = system;
            gyroSensor = deviceGyro;
            accelSensor = deviceAccel;
            hasMotion = true;
        }
    }

    @RequiresApi(31)
    private void selectOutputs(InputDevice dev) {
        VibratorManager manager = dev.getVibratorManager();
        if (manager.getVibratorIds().length > 0) {
            vibratorManager = manager;
        }
        else if (dev.getVibrator().hasVibrator()) {
            vibrator = dev.getVibrator();
        }
        for (Light light : dev.getLightsManager().getLights()) {
            if (ControllerTestModel.trustRgbLight(Build.VERSION.SDK_INT, dev.getVendorId(), light.hasRgbControl())) {
                rgbLights.add(light);
            }
        }
        if (ControllerTestModel.canProbeSensors(Build.VERSION.SDK_INT, dev.getVendorId())) {
            try {
                SensorManager manager2 = dev.getSensorManager();
                gyroSensor = manager2.getDefaultSensor(Sensor.TYPE_GYROSCOPE);
                accelSensor = manager2.getDefaultSensor(Sensor.TYPE_ACCELEROMETER);
                if (gyroSensor != null || accelSensor != null) {
                    sensorManager = manager2;
                    hasMotion = true;
                }
            } catch (RuntimeException e) {
                LimeLog.warning("Controller sensors unavailable: " + e);
            }
        }
    }

    private static boolean detectTouchpad(InputDevice dev) {
        if ((dev.getSources() & InputDevice.SOURCE_TOUCHPAD) == InputDevice.SOURCE_TOUCHPAD) {
            return true;
        }
        // Some kernels split a PlayStation pad's touch pad into its own device.
        for (int id : InputDevice.getDeviceIds()) {
            InputDevice other = InputDevice.getDevice(id);
            if (other != null && other.getId() != dev.getId() && other.getVendorId() == dev.getVendorId() &&
                    other.getProductId() == dev.getProductId() &&
                    (other.getSources() & InputDevice.SOURCE_TOUCHPAD) == InputDevice.SOURCE_TOUCHPAD) {
                return true;
            }
        }
        return false;
    }

    void registerSensors() {
        if (sensorManager == null) return;
        if (gyroSensor != null) sensorManager.registerListener(sensorListener, gyroSensor, SensorManager.SENSOR_DELAY_UI, handler);
        if (accelSensor != null) sensorManager.registerListener(sensorListener, accelSensor, SensorManager.SENSOR_DELAY_UI, handler);
    }

    void buildActions() {
        InputDevice device = deviceSupplier.get();
        AbstractController usbController = usbSupplier.get();
        rumbleActions.removeAllViews();
        lightActions.removeAllViews();
        short caps = usbController != null ? usbController.getCapabilities() : 0;
        if (usbController != null) {
            if ((caps & MoonBridge.LI_CCAP_RUMBLE) != 0) {
                addAction(rumbleActions, R.string.controller_test_rumble_strong, v -> usbRumble((short) 0xFFFF, (short) 0));
                addAction(rumbleActions, R.string.controller_test_rumble_weak, v -> usbRumble((short) 0, (short) 0xFFFF));
                addAction(rumbleActions, R.string.controller_test_rumble_all, v -> usbRumble((short) 0xFFFF, (short) 0xFFFF));
            }
            if ((caps & MoonBridge.LI_CCAP_TRIGGER_RUMBLE) != 0) {
                addAction(rumbleActions, R.string.controller_test_rumble_left_trigger, v -> usbTriggerRumble((short) 0xFFFF, (short) 0));
                addAction(rumbleActions, R.string.controller_test_rumble_right_trigger, v -> usbTriggerRumble((short) 0, (short) 0xFFFF));
            }
            if ((caps & MoonBridge.LI_CCAP_RGB_LED) != 0) {
                for (int i = 0; i < LIGHT_COLORS.length; i++) {
                    int color = LIGHT_COLORS[i];
                    addAction(lightActions, LIGHT_NAMES[i], v -> usbController.setLed((byte) (color >> 16), (byte) (color >> 8), (byte) color));
                }
                addAction(lightActions, R.string.controller_test_light_off, v -> usbController.setLed((byte) 0, (byte) 0, (byte) 0));
            }
        }
        else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && vibratorManager != null) {
            addVibratorActions();
        }
        else if (vibrator != null) {
            addAction(rumbleActions, R.string.controller_test_rumble_any, v -> vibrate(vibrator));
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && !rgbLights.isEmpty()) {
            for (int i = 0; i < LIGHT_COLORS.length; i++) {
                int color = LIGHT_COLORS[i];
                addAction(lightActions, LIGHT_NAMES[i], v -> setLights(color));
            }
            addAction(lightActions, R.string.controller_test_light_reset, v -> resetLights());
        }
        if (rumbleActions.getChildCount() > 0) {
            addAction(rumbleActions, R.string.controller_test_rumble_stop, v -> stopRumble());
        }
        rumbleActions.setVisibility(rumbleActions.getChildCount() > 0 ? View.VISIBLE : View.GONE);
        lightActions.setVisibility(lightActions.getChildCount() > 0 ? View.VISIBLE : View.GONE);

        boolean anyController = device != null || usbController != null;
        boolean canRumble = rumbleActions.getChildCount() > 0;
        String rumbleText = !anyController ? context.getString(R.string.controller_test_waiting_short) :
                canRumble ? null : context.getString(R.string.controller_test_not_available);
        if (canRumble && rumbleDisabledForStreams) {
            rumbleText = context.getString(R.string.controller_test_rumble_disabled_note);
        }
        setNote(rumbleNote, rumbleText);
        boolean lightsApi = usbController != null || Build.VERSION.SDK_INT >= Build.VERSION_CODES.S;
        setNote(lightsNote, lightActions.getChildCount() > 0 ? null : !anyController ?
                context.getString(R.string.controller_test_waiting_short) : lightsApi ?
                context.getString(R.string.controller_test_not_available) : context.getString(R.string.controller_test_needs_android_12));
    }

    @RequiresApi(31)
    private void addVibratorActions() {
        int[] ids = vibratorManager.getVibratorIds();
        ControllerTestModel.Motor[] motors = ControllerTestModel.motors(ids.length);
        for (int i = 0; i < motors.length; i++) {
            int id = ids[i];
            addAction(rumbleActions, motorName(motors[i]), v -> vibrate(new int[] {id}));
        }
        addAction(rumbleActions, motors.length > 0 ? R.string.controller_test_rumble_all :
                R.string.controller_test_rumble_any, v -> vibrate(ids));
    }

    private static int motorName(ControllerTestModel.Motor motor) {
        switch (motor) {
            case WEAK: return R.string.controller_test_rumble_weak;
            case STRONG: return R.string.controller_test_rumble_strong;
            case LEFT_TRIGGER: return R.string.controller_test_rumble_left_trigger;
            default: return R.string.controller_test_rumble_right_trigger;
        }
    }

    private static void setNote(TextView view, String text) {
        view.setText(text);
        view.setVisibility(text == null ? View.GONE : View.VISIBLE);
    }

    private void addAction(ChipGroup group, int label, View.OnClickListener onClick) {
        Chip chip = new Chip(context);
        chip.setText(label);
        chip.setCheckable(false);
        chip.setEnsureMinTouchTargetSize(true);
        chip.setOnClickListener(onClick);
        group.addView(chip);
    }

    @RequiresApi(31)
    private void vibrate(int[] ids) {
        CombinedVibration.ParallelCombination combination = CombinedVibration.startParallel();
        for (int id : ids) {
            combination.addVibrator(id, VibrationEffect.createOneShot(RUMBLE_MS, VibrationEffect.DEFAULT_AMPLITUDE));
        }
        vibratorManager.vibrate(combination.combine());
    }

    @SuppressWarnings("deprecation")
    private static void vibrate(Vibrator vibrator) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            vibrator.vibrate(VibrationEffect.createOneShot(RUMBLE_MS, VibrationEffect.DEFAULT_AMPLITUDE));
        }
        else {
            vibrator.vibrate(RUMBLE_MS);
        }
    }

    private void usbRumble(short strong, short weak) {
        usbSupplier.get().rumble(RumblePolicy.shapeMotor(strong, rumbleFeel), RumblePolicy.shapeMotor(weak, rumbleFeel));
        handler.removeCallbacks(rumbleTimeout);
        handler.postDelayed(rumbleTimeout, RUMBLE_MS);
    }

    private void usbTriggerRumble(short left, short right) {
        usbSupplier.get().rumbleTriggers(RumblePolicy.shapeMotor(left, rumbleFeel), RumblePolicy.shapeMotor(right, rumbleFeel));
        handler.removeCallbacks(rumbleTimeout);
        handler.postDelayed(rumbleTimeout, RUMBLE_MS);
    }

    private void stopRumble() {
        handler.removeCallbacks(rumbleTimeout);
        AbstractController usbController = usbSupplier.get();
        if (usbController != null) {
            usbController.rumble((short) 0, (short) 0);
            usbController.rumbleTriggers((short) 0, (short) 0);
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && vibratorManager != null) {
            vibratorManager.cancel();
        }
        if (vibrator != null) {
            vibrator.cancel();
        }
    }

    @RequiresApi(31)
    private void setLights(int color) {
        InputDevice device = deviceSupplier.get();
        if (device == null || rgbLights.isEmpty()) return;
        try {
            LightState state = new LightState.Builder().setColor(color).build();
            LightsRequest.Builder request = new LightsRequest.Builder();
            for (Light light : rgbLights) {
                request.addLight(light, state);
            }
            if (lightsSession == null) {
                lightsSession = device.getLightsManager().openSession();
            }
            lightsSession.requestLights(request.build());
        } catch (RuntimeException e) {
            LimeLog.warning("Controller lights unavailable: " + e);
        }
    }

    // Closing the session hands the lights back to the system, which restores them.
    @RequiresApi(31)
    private void resetLights() {
        if (lightsSession != null) {
            lightsSession.close();
            lightsSession = null;
        }
    }
}
