package com.limelight.preferences;

import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Typeface;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.hardware.input.InputManager;
import android.hardware.lights.Light;
import android.hardware.lights.LightState;
import android.hardware.lights.LightsManager;
import android.hardware.lights.LightsRequest;
import android.os.Build;
import android.os.Bundle;
import android.os.CombinedVibration;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.VibrationEffect;
import android.os.Vibrator;
import android.os.VibratorManager;
import android.util.AttributeSet;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.TextView;

import androidx.annotation.RequiresApi;
import com.limelight.ui.NightjarActivity;

import com.google.android.material.chip.Chip;
import com.google.android.material.chip.ChipGroup;
import com.google.android.material.color.MaterialColors;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.input.ControllerTestModel;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.binding.input.driver.UsbDriverListener;
import com.limelight.binding.input.driver.UsbDriverService;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.UiHelper;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Set;
import java.util.TreeSet;

/**
 * Shows what a controller sends and tries what it can receive: rumble, lights, motion and touch.
 * Leaving the page stops rumble and gives the lights and the USB driver back.
 */
public class ControllerTestActivity extends NightjarActivity implements UsbDriverListener,
        SensorEventListener, InputManager.InputDeviceListener {
    private static final long HOLD_TO_LEAVE_MS = 1000;
    private static final int RUMBLE_MS = 600;
    private static final int[] LIGHT_COLORS = {0xFFFF0000, 0xFF00FF00, 0xFF0000FF};
    private static final int[] LIGHT_NAMES = {R.string.controller_test_light_red,
            R.string.controller_test_light_green, R.string.controller_test_light_blue};

    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Runnable leave = this::finish;
    private final Runnable rumbleTimeout = this::stopRumble;

    private TextView deviceView, pressedView, stickValuesView, axesView, rumbleNote, lightsNote, motionView, touchpadView;
    private ChipGroup buttonChips, rumbleActions, lightActions;
    private final List<Chip> chips = new ArrayList<>();
    private SticksView sticks;
    private boolean rumbleDisabledForStreams;

    // The controller under test: an Android input device, or one Nightjar's USB driver owns.
    private InputDevice device;
    private AbstractController usbController;
    private ControllerTestModel.Layout layout = new ControllerTestModel.Layout();
    private final Set<Integer> axes = new TreeSet<>();
    private final StringBuilder axesText = new StringBuilder();
    private int keyButtons, hatButtons, usbButtons, lastHat;
    private float leftX, leftY, rightX, rightY, leftTrigger, rightTrigger;
    private float[] gyro, accel;
    private String touch;
    private boolean hasTouchpad, hasMotion, resumed;

    private VibratorManager vibratorManager;
    private Vibrator vibrator;
    private LightsManager.LightsSession lightsSession;
    private final List<Light> rgbLights = new ArrayList<>();
    private SensorManager sensorManager;
    private Sensor gyroSensor, accelSensor;

    private InputManager inputManager;
    private boolean usbBound;
    private UsbDriverService.UsbDriverBinder usbBinder;
    private final ServiceConnection usbConnection = new ServiceConnection() {
        @Override
        public void onServiceConnected(ComponentName name, IBinder service) {
            usbBinder = (UsbDriverService.UsbDriverBinder) service;
            usbBinder.setListener(ControllerTestActivity.this);
            usbBinder.start();
        }

        @Override
        public void onServiceDisconnected(ComponentName name) {
            usbBinder = null;
        }
    };

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        UiHelper.setLocale(this);
        super.onCreate(savedInstanceState);
        FrameLayout page = TopAppBar.setUpPage(this, R.string.controller_test_title, this::finish);
        View body = getLayoutInflater().inflate(R.layout.controller_test, page, false);
        TopAppBar.addScrollingBody(page, body);
        deviceView = body.findViewById(R.id.controller_test_device);
        pressedView = body.findViewById(R.id.controller_test_pressed);
        stickValuesView = body.findViewById(R.id.controller_test_stick_values);
        axesView = body.findViewById(R.id.controller_test_axes);
        rumbleNote = body.findViewById(R.id.controller_test_rumble_note);
        lightsNote = body.findViewById(R.id.controller_test_lights_note);
        motionView = body.findViewById(R.id.controller_test_motion_values);
        touchpadView = body.findViewById(R.id.controller_test_touchpad_values);
        buttonChips = body.findViewById(R.id.controller_test_buttons);
        rumbleActions = body.findViewById(R.id.controller_test_rumble_actions);
        lightActions = body.findViewById(R.id.controller_test_light_actions);
        sticks = body.findViewById(R.id.controller_test_sticks);
        for (int flag : ControllerTestModel.BUTTONS) {
            Chip chip = new Chip(this);
            chip.setText(buttonName(flag));
            chip.setCheckable(true);
            chip.setCheckedIconVisible(false);
            chip.setClickable(false);
            chip.setFocusable(false);
            chip.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
            chip.setTag(flag);
            chips.add(chip);
            buttonChips.addView(chip);
        }
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(this);
        rumbleDisabledForStreams = config.disableRumble;
        usbBound = (config.usbDriver || config.usbDualSense || config.usbSwitchPro) &&
                bindService(new Intent(this, UsbDriverService.class), usbConnection, Context.BIND_AUTO_CREATE);
        inputManager = (InputManager) getSystemService(Context.INPUT_SERVICE);
        inputManager.registerInputDeviceListener(this, handler);
        UiHelper.notifyNewRootView(this);
        selectSingleConnectedController();
        buildActions();
        render();
    }

    @Override
    protected void onResume() {
        super.onResume();
        resumed = true;
        registerSensors();
    }

    @Override
    protected void onPause() {
        resumed = false;
        handler.removeCallbacks(leave);
        releaseOutputs();
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        inputManager.unregisterInputDeviceListener(this);
        if (usbBinder != null) {
            usbBinder.setListener(null);
            usbBinder.stop();
            usbBinder = null;
        }
        if (usbBound) {
            unbindService(usbConnection);
            usbBound = false;
        }
        handler.removeCallbacksAndMessages(null);
        super.onDestroy();
    }

    private void selectSingleConnectedController() {
        for (int id : InputDevice.getDeviceIds()) {
            InputDevice candidate = InputDevice.getDevice(id);
            if (candidate != null && !candidate.isVirtual() &&
                    (candidate.getSources() & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD) {
                select(candidate);
                return;
            }
        }
    }

    // Leaves the controller as it was found: no rumble, the system's lights, no sensor listeners.
    private void releaseOutputs() {
        stopRumble();
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && lightsSession != null) {
            lightsSession.close();
            lightsSession = null;
        }
        if (sensorManager != null) {
            sensorManager.unregisterListener(this);
        }
        if (usbController != null) {
            usbController.setMotionEventState(MoonBridge.LI_MOTION_TYPE_ACCEL, (short) 0);
            usbController.setMotionEventState(MoonBridge.LI_MOTION_TYPE_GYRO, (short) 0);
        }
    }

    private void resetReadings() {
        keyButtons = hatButtons = usbButtons = lastHat = 0;
        leftX = leftY = rightX = rightY = leftTrigger = rightTrigger = 0;
        gyro = accel = null;
        touch = null;
        axes.clear();
        vibratorManager = null;
        vibrator = null;
        rgbLights.clear();
        sensorManager = null;
        gyroSensor = accelSensor = null;
        hasTouchpad = hasMotion = false;
    }

    private void select(InputDevice dev) {
        releaseOutputs();
        resetReadings();
        usbController = null;
        device = dev;
        for (InputDevice.MotionRange range : dev.getMotionRanges()) {
            if ((range.getSource() & InputDevice.SOURCE_CLASS_JOYSTICK) != 0) {
                axes.add(range.getAxis());
            }
        }
        boolean nonStandardDs4 = dev.getVendorId() == 0x054c && dev.hasKeys(KeyEvent.KEYCODE_BUTTON_C)[0];
        layout = ControllerTestModel.layout(axes, nonStandardDs4);

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            selectOutputs(dev);
        }
        else if (dev.getVibrator().hasVibrator()) {
            vibrator = dev.getVibrator();
        }
        hasTouchpad = hasTouchpad(dev);
        if (resumed) {
            registerSensors();
        }
        buildActions();
        render();
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

    private static boolean hasTouchpad(InputDevice dev) {
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

    private void registerSensors() {
        if (sensorManager == null) return;
        if (gyroSensor != null) sensorManager.registerListener(this, gyroSensor, SensorManager.SENSOR_DELAY_UI, handler);
        if (accelSensor != null) sensorManager.registerListener(this, accelSensor, SensorManager.SENSOR_DELAY_UI, handler);
    }

    private void buildActions() {
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
        String rumbleText = !anyController ? getString(R.string.controller_test_waiting_short) :
                canRumble ? null : getString(R.string.controller_test_not_available);
        if (canRumble && rumbleDisabledForStreams) {
            rumbleText = getString(R.string.controller_test_rumble_disabled_note);
        }
        setNote(rumbleNote, rumbleText);
        boolean lightsApi = usbController != null || Build.VERSION.SDK_INT >= Build.VERSION_CODES.S;
        setNote(lightsNote, lightActions.getChildCount() > 0 ? null : !anyController ?
                getString(R.string.controller_test_waiting_short) : lightsApi ?
                getString(R.string.controller_test_not_available) : getString(R.string.controller_test_needs_android_12));
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
        Chip chip = new Chip(this);
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
        usbController.rumble(strong, weak);
        handler.removeCallbacks(rumbleTimeout);
        handler.postDelayed(rumbleTimeout, RUMBLE_MS);
    }

    private void usbTriggerRumble(short left, short right) {
        usbController.rumbleTriggers(left, right);
        handler.removeCallbacks(rumbleTimeout);
        handler.postDelayed(rumbleTimeout, RUMBLE_MS);
    }

    private void stopRumble() {
        handler.removeCallbacks(rumbleTimeout);
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

    private static boolean isController(InputDevice dev, int source) {
        if (dev == null || dev.isVirtual()) return false;
        int sources = dev.getSources();
        return (source & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD ||
                (source & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK ||
                ((source & InputDevice.SOURCE_DPAD) == InputDevice.SOURCE_DPAD &&
                        (sources & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD);
    }

    private static boolean isDpadKey(int keyCode) {
        return keyCode == KeyEvent.KEYCODE_DPAD_UP || keyCode == KeyEvent.KEYCODE_DPAD_DOWN ||
                keyCode == KeyEvent.KEYCODE_DPAD_LEFT || keyCode == KeyEvent.KEYCODE_DPAD_RIGHT ||
                keyCode == KeyEvent.KEYCODE_DPAD_CENTER;
    }

    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        InputDevice dev = event.getDevice();
        if (!isController(dev, event.getSource()) || (event.getFlags() & KeyEvent.FLAG_FALLBACK) != 0) {
            return super.dispatchKeyEvent(event);
        }
        if (usbController == null && (device == null || dev.getId() != device.getId())) {
            select(dev);
        }
        int keyCode = event.getKeyCode();
        boolean down = event.getAction() == KeyEvent.ACTION_DOWN;
        int flag = ControllerTestModel.buttonFlag(keyCode);
        if (flag != 0 && event.getAction() != KeyEvent.ACTION_MULTIPLE) {
            keyButtons = down ? keyButtons | flag : keyButtons & ~flag;
            renderButtons();
        }
        if (keyCode == KeyEvent.KEYCODE_BUTTON_B || keyCode == KeyEvent.KEYCODE_BACK) {
            if (down && event.getRepeatCount() == 0) {
                handler.postDelayed(leave, HOLD_TO_LEAVE_MS);
            }
            else if (!down) {
                handler.removeCallbacks(leave);
            }
            return true;
        }
        if (isDpadKey(keyCode)) {
            return super.dispatchKeyEvent(event);
        }
        if (keyCode == KeyEvent.KEYCODE_BUTTON_A && !down) {
            View focused = getCurrentFocus();
            if (focused != null && focused.isClickable()) {
                focused.performClick();
            }
        }
        return true;
    }

    @Override
    public boolean dispatchGenericMotionEvent(MotionEvent event) {
        InputDevice dev = event.getDevice();
        if ((event.getSource() & InputDevice.SOURCE_CLASS_JOYSTICK) != 0 && isController(dev, event.getSource()) &&
                event.getActionMasked() == MotionEvent.ACTION_MOVE) {
            if (usbController == null && (device == null || dev.getId() != device.getId())) {
                select(dev);
            }
            readAxes(event);
            return true;
        }
        if (event.getSource() == InputDevice.SOURCE_TOUCHPAD && dev != null) {
            readTouchpad(event, dev);
            return true;
        }
        return super.dispatchGenericMotionEvent(event);
    }

    private void readAxes(MotionEvent event) {
        leftX = event.getAxisValue(MotionEvent.AXIS_X);
        leftY = event.getAxisValue(MotionEvent.AXIS_Y);
        rightX = layout.rightStickX == -1 ? 0 : event.getAxisValue(layout.rightStickX);
        rightY = layout.rightStickY == -1 ? 0 : event.getAxisValue(layout.rightStickY);
        leftTrigger = layout.leftTrigger == -1 ? 0 : layout.trigger(event.getAxisValue(layout.leftTrigger));
        rightTrigger = layout.rightTrigger == -1 ? 0 : layout.trigger(event.getAxisValue(layout.rightTrigger));
        int hat = ControllerTestModel.hatFlags(event.getAxisValue(MotionEvent.AXIS_HAT_X),
                event.getAxisValue(MotionEvent.AXIS_HAT_Y));
        if (hat != lastHat) {
            moveFocusForHat(hat & ~lastHat);
            lastHat = hat;
        }
        hatButtons = hat;
        axesText.setLength(0);
        int column = 0;
        for (int axis : axes) {
            if (column > 0) axesText.append(column % 2 == 0 ? '\n' : ' ');
            String name = MotionEvent.axisToString(axis).replace("AXIS_", "");
            axesText.append(String.format(Locale.ROOT, "%-9s%+.2f", name, event.getAxisValue(axis)));
            column++;
        }
        renderSticks();
        renderButtons();
    }

    // A hat-only D-pad sends no keys, so it moves through the page here as a D-pad would.
    private void moveFocusForHat(int newlyPressed) {
        int direction;
        if ((newlyPressed & ControllerPacket.UP_FLAG) != 0) direction = View.FOCUS_UP;
        else if ((newlyPressed & ControllerPacket.DOWN_FLAG) != 0) direction = View.FOCUS_DOWN;
        else if ((newlyPressed & ControllerPacket.LEFT_FLAG) != 0) direction = View.FOCUS_LEFT;
        else if ((newlyPressed & ControllerPacket.RIGHT_FLAG) != 0) direction = View.FOCUS_RIGHT;
        else return;
        View focused = getCurrentFocus();
        if (focused == null) {
            getWindow().getDecorView().requestFocus(direction);
            return;
        }
        View next = focused.focusSearch(direction);
        if (next != null) {
            next.requestFocus(direction);
        }
    }

    private void readTouchpad(MotionEvent event, InputDevice dev) {
        int action = event.getActionMasked();
        if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) {
            touch = null;
        }
        else {
            InputDevice.MotionRange x = dev.getMotionRange(MotionEvent.AXIS_X, InputDevice.SOURCE_TOUCHPAD);
            InputDevice.MotionRange y = dev.getMotionRange(MotionEvent.AXIS_Y, InputDevice.SOURCE_TOUCHPAD);
            if (x != null && y != null && x.getRange() > 0 && y.getRange() > 0) {
                showTouch((event.getX() - x.getMin()) / x.getRange(), (event.getY() - y.getMin()) / y.getRange());
                return;
            }
        }
        renderTouch();
    }

    private void showTouch(float x, float y) {
        hasTouchpad = true;
        touch = getString(R.string.controller_test_touch_at,
                Math.round(Math.max(0, Math.min(1, x)) * 100), Math.round(Math.max(0, Math.min(1, y)) * 100));
        renderTouch();
    }

    @Override
    public void onSensorChanged(SensorEvent event) {
        if (event.sensor.getType() == Sensor.TYPE_GYROSCOPE) {
            gyro = event.values.clone();
        }
        else if (event.sensor.getType() == Sensor.TYPE_ACCELEROMETER) {
            accel = event.values.clone();
        }
        renderMotion();
    }

    @Override
    public void onAccuracyChanged(Sensor sensor, int accuracy) {}

    @Override
    public void onInputDeviceAdded(int deviceId) {
        if (device == null && usbController == null) {
            selectSingleConnectedController();
        }
    }

    @Override
    public void onInputDeviceRemoved(int deviceId) {
        if (device != null && device.getId() == deviceId) {
            releaseOutputs();
            resetReadings();
            device = null;
            buildActions();
            render();
            selectSingleConnectedController();
        }
    }

    @Override
    public void onInputDeviceChanged(int deviceId) {}

    // Nightjar's USB driver: Android sees no input from a pad the driver owns, so it reports here.
    @Override
    public void reportControllerState(int controllerId, int buttonFlags, float leftStickX, float leftStickY,
                                      float rightStickX, float rightStickY, float leftTrigger, float rightTrigger) {
        if (usbController == null || usbController.getControllerId() != controllerId) return;
        usbButtons = buttonFlags;
        leftX = leftStickX;
        leftY = leftStickY;
        rightX = rightStickX;
        rightY = rightStickY;
        this.leftTrigger = leftTrigger;
        this.rightTrigger = rightTrigger;
        renderSticks();
        renderButtons();
    }

    @Override
    public void reportControllerTouch(int controllerId, byte eventType, int pointerId, float x, float y, float pressure) {
        if (usbController == null || usbController.getControllerId() != controllerId) return;
        if (eventType == MoonBridge.LI_TOUCH_EVENT_UP || eventType == MoonBridge.LI_TOUCH_EVENT_CANCEL) {
            touch = null;
            renderTouch();
        }
        else {
            showTouch(x, y);
        }
    }

    @Override
    public void reportControllerMotion(int controllerId, byte motionType, float x, float y, float z) {
        if (usbController == null || usbController.getControllerId() != controllerId) return;
        if (motionType == MoonBridge.LI_MOTION_TYPE_GYRO) gyro = new float[] {x, y, z};
        else if (motionType == MoonBridge.LI_MOTION_TYPE_ACCEL) accel = new float[] {x, y, z};
        renderMotion();
    }

    @Override
    public void reportControllerBattery(int controllerId, byte state, byte percentage) {}

    @Override
    public void deviceAdded(AbstractController controller) {
        releaseOutputs();
        resetReadings();
        device = null;
        usbController = controller;
        short caps = controller.getCapabilities();
        hasTouchpad = (caps & MoonBridge.LI_CCAP_TOUCHPAD) != 0;
        hasMotion = (caps & (MoonBridge.LI_CCAP_ACCEL | MoonBridge.LI_CCAP_GYRO)) != 0;
        if (hasMotion) {
            controller.setMotionEventState(MoonBridge.LI_MOTION_TYPE_ACCEL, (short) 60);
            controller.setMotionEventState(MoonBridge.LI_MOTION_TYPE_GYRO, (short) 60);
        }
        buildActions();
        render();
    }

    @Override
    public void deviceRemoved(AbstractController controller) {
        if (usbController != controller) return;
        handler.removeCallbacks(rumbleTimeout);
        resetReadings();
        usbController = null;
        buildActions();
        render();
        selectSingleConnectedController();
    }

    private void render() {
        if (usbController != null) {
            deviceView.setText(getString(R.string.controller_test_usb_device,
                    String.format(Locale.ROOT, "%04x:%04x", usbController.getVendorId(), usbController.getProductId())));
        }
        else if (device != null) {
            String name = device.getName() == null || device.getName().trim().isEmpty() ?
                    getString(R.string.controller_mapping_unnamed) : device.getName().trim();
            deviceView.setText(name);
        }
        else {
            deviceView.setText(R.string.controller_test_waiting);
        }
        renderButtons();
        renderSticks();
        renderMotion();
        renderTouch();
    }

    private void renderButtons() {
        int pressed = keyButtons | hatButtons | usbButtons | ControllerTestModel.triggerFlags(leftTrigger, rightTrigger);
        StringBuilder names = new StringBuilder();
        for (Chip chip : chips) {
            int flag = (Integer) chip.getTag();
            boolean on = (pressed & flag) != 0;
            if (chip.isChecked() != on) chip.setChecked(on);
            if (on) {
                if (names.length() > 0) names.append(", ");
                names.append(chip.getText());
            }
        }
        String text = names.length() == 0 ? getString(R.string.controller_test_pressed_none) :
                getString(R.string.controller_test_pressed, names);
        if (!text.contentEquals(pressedView.getText())) pressedView.setText(text);
    }

    private void renderSticks() {
        sticks.set(leftX, leftY, rightX, rightY, leftTrigger, rightTrigger);
        stickValuesView.setText(getString(R.string.controller_test_stick_values, leftX, leftY, rightX, rightY,
                Math.round(leftTrigger * 100), Math.round(rightTrigger * 100)));
        axesView.setText(axesText);
        axesView.setVisibility(axesText.length() == 0 ? View.GONE : View.VISIBLE);
    }

    private void renderMotion() {
        motionView.setTypeface(hasMotion ? Typeface.MONOSPACE : Typeface.DEFAULT);
        if (!hasMotion) {
            boolean api = usbController != null || Build.VERSION.SDK_INT >= Build.VERSION_CODES.S;
            motionView.setText(device == null && usbController == null ? R.string.controller_test_waiting_short :
                    api ? R.string.controller_test_not_available : R.string.controller_test_needs_android_12);
            return;
        }
        motionView.setText(getString(R.string.controller_test_motion_values, vector(gyro), vector(accel)));
    }

    private String vector(float[] values) {
        if (values == null || values.length < 3) return getString(R.string.controller_test_waiting_motion);
        return String.format(Locale.ROOT, "%+7.2f %+7.2f %+7.2f", values[0], values[1], values[2]);
    }

    private void renderTouch() {
        touchpadView.setText(device == null && usbController == null ? getString(R.string.controller_test_waiting_short) :
                !hasTouchpad ? getString(R.string.controller_test_not_available) :
                touch != null ? touch : getString(R.string.controller_test_touch_none));
    }

    private String buttonName(int flag) {
        switch (flag) {
            case ControllerPacket.A_FLAG: return "A";
            case ControllerPacket.B_FLAG: return "B";
            case ControllerPacket.X_FLAG: return "X";
            case ControllerPacket.Y_FLAG: return "Y";
            case ControllerPacket.LB_FLAG: return "LB";
            case ControllerPacket.RB_FLAG: return "RB";
            case ControllerTestModel.LT_FLAG: return "LT";
            case ControllerTestModel.RT_FLAG: return "RT";
            case ControllerPacket.LS_CLK_FLAG: return "LS";
            case ControllerPacket.RS_CLK_FLAG: return "RS";
            case ControllerPacket.BACK_FLAG: return getString(R.string.controller_button_select);
            case ControllerPacket.PLAY_FLAG: return getString(R.string.controller_button_start);
            case ControllerPacket.SPECIAL_BUTTON_FLAG: return getString(R.string.controller_button_guide);
            case ControllerPacket.UP_FLAG: return "↑";
            case ControllerPacket.DOWN_FLAG: return "↓";
            case ControllerPacket.LEFT_FLAG: return "←";
            case ControllerPacket.RIGHT_FLAG: return "→";
            case ControllerPacket.TOUCHPAD_FLAG: return getString(R.string.controller_test_touchpad);
            case ControllerPacket.MISC_FLAG: return getString(R.string.controller_test_misc);
            case ControllerPacket.PADDLE1_FLAG: return "P1";
            case ControllerPacket.PADDLE2_FLAG: return "P2";
            case ControllerPacket.PADDLE3_FLAG: return "P3";
            default: return "P4";
        }
    }

    /** Both sticks as a dot in a circle, each with its trigger as a bar underneath. */
    public static class SticksView extends View {
        private final Paint outline = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint track = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final float density;
        private float lx, ly, rx, ry, lt, rt;

        public SticksView(Context context, AttributeSet attrs) {
            super(context, attrs);
            density = getResources().getDisplayMetrics().density;
            outline.setStyle(Paint.Style.STROKE);
            outline.setStrokeWidth(2 * density);
            outline.setColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOutline, 0xFF888888));
            fill.setColor(MaterialColors.getColor(context, androidx.appcompat.R.attr.colorPrimary, 0xFFFFB300));
            track.setColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorSurfaceVariant, 0xFF444444));
        }

        void set(float lx, float ly, float rx, float ry, float lt, float rt) {
            if (lx == this.lx && ly == this.ly && rx == this.rx && ry == this.ry && lt == this.lt && rt == this.rt) return;
            this.lx = lx; this.ly = ly; this.rx = rx; this.ry = ry; this.lt = lt; this.rt = rt;
            invalidate();
        }

        @Override
        protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
            int width = MeasureSpec.getSize(widthMeasureSpec);
            float circle = Math.min(width / 2f - 16 * density, 140 * density);
            setMeasuredDimension(width, Math.round(circle + 28 * density));
        }

        @Override
        protected void onDraw(Canvas canvas) {
            float half = getWidth() / 2f;
            float diameter = Math.min(half - 16 * density, 140 * density);
            drawStick(canvas, half / 2, diameter, lx, ly, lt);
            drawStick(canvas, half + half / 2, diameter, rx, ry, rt);
        }

        private void drawStick(Canvas canvas, float centerX, float diameter, float x, float y, float trigger) {
            float radius = diameter / 2;
            float centerY = radius + outline.getStrokeWidth();
            canvas.drawCircle(centerX, centerY, radius - outline.getStrokeWidth(), outline);
            canvas.drawLine(centerX - radius, centerY, centerX + radius, centerY, track);
            canvas.drawLine(centerX, centerY - radius, centerX, centerY + radius, track);
            float dot = 7 * density;
            float reach = radius - dot - outline.getStrokeWidth();
            canvas.drawCircle(centerX + clamp(x) * reach, centerY + clamp(y) * reach, dot, fill);
            float top = diameter + 12 * density, bottom = top + 8 * density;
            canvas.drawRect(centerX - radius, top, centerX + radius, bottom, track);
            canvas.drawRect(centerX - radius, top, centerX - radius + diameter * Math.max(0, Math.min(1, trigger)), bottom, fill);
        }

        private static float clamp(float value) {
            return Math.max(-1, Math.min(1, value));
        }
    }
}
