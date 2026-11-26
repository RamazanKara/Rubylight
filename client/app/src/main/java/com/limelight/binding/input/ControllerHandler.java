package com.limelight.binding.input;

import android.annotation.SuppressLint;
import android.annotation.TargetApi;
import android.app.Activity;
import android.content.Context;
import android.hardware.SensorManager;
import android.hardware.input.InputManager;
import android.hardware.lights.LightsManager;
import android.os.Build;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.os.Vibrator;
import android.os.VibratorManager;
import android.view.InputDevice;
import android.view.InputEvent;
import android.view.KeyEvent;
import android.view.MotionEvent;

import com.limelight.Game;
import com.limelight.binding.audio.AndroidAudioRenderer;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.binding.input.driver.UsbDriverListener;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarInputJni;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.GameGestures;
import com.limelight.utils.Vector2d;

import org.cgutman.shieldcontrollerextensions.SceManager;

import java.util.HashMap;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicIntegerArray;

/**
 * Coordinates physical Android gamepads, USB-driver controllers, on-screen gamepad fusion,
 * rumble/haptics, motion sensors, and launch controller preparation during a stream.
 */
public class ControllerHandler implements InputManager.InputDeviceListener, UsbDriverListener {

    public static final int MINIMUM_BUTTON_DOWN_TIME_MS = 25;
    static final int EMULATING_SPECIAL = 0x1;
    static final int EMULATING_SELECT = 0x2;
    static final int EMULATING_TOUCHPAD = 0x4;
    static final short MAX_GAMEPADS = 16; // Limited by bits in activeGamepadMask
    static final int BATTERY_RECHECK_INTERVAL_MS = 120 * 1000;
    static final double MIN_STICK_DEADZONE = 0.01;
    static final float QUIRK_TRIGGER_DEADZONE = 0.30f;

    /** The buttons the on-screen controller has, for its arrival. */
    static final int ONSCREEN_PAD_BUTTONS = ControllerPacket.A_FLAG | ControllerPacket.B_FLAG |
            ControllerPacket.X_FLAG | ControllerPacket.Y_FLAG | ControllerPacket.UP_FLAG |
            ControllerPacket.DOWN_FLAG | ControllerPacket.LEFT_FLAG | ControllerPacket.RIGHT_FLAG |
            ControllerPacket.LB_FLAG | ControllerPacket.RB_FLAG | ControllerPacket.LS_CLK_FLAG |
            ControllerPacket.RS_CLK_FLAG | ControllerPacket.BACK_FLAG | ControllerPacket.PLAY_FLAG |
            ControllerPacket.SPECIAL_BUTTON_FLAG | ControllerPacket.TOUCHPAD_FLAG;

    private static volatile int[] androidButtonKeycodes;
    private static volatile int[] capturedButtonFlags;

    static int[] androidButtonKeycodes() {
        int[] codes = androidButtonKeycodes;
        if (codes == null) androidButtonKeycodes = codes = NightjarInputJni.androidButtonKeycodes();
        return codes;
    }

    static int[] capturedButtonFlags() {
        int[] flags = capturedButtonFlags;
        if (flags == null) capturedButtonFlags = flags = NightjarInputJni.capturedButtonFlags();
        return flags;
    }

    final Object fuseLock = new Object();
    long slotStateHandle, usbInputStateHandle, rumbleStateHandle;

    final ConcurrentHashMap<Integer, InputDeviceContext> inputDeviceContexts = new ConcurrentHashMap<>();
    final ConcurrentHashMap<Integer, UsbDeviceContext> usbDeviceContexts = new ConcurrentHashMap<>();
    final ConcurrentHashMap<Short, AbstractController> waveformControllers = new ConcurrentHashMap<>();
    final Set<Integer> failedUsbDevices = ConcurrentHashMap.newKeySet();
    final AtomicIntegerArray controllerSlotsWithInput = new AtomicIntegerArray(MAX_GAMEPADS);

    final NvConnection conn;
    final Activity activityContext;
    final double stickDeadzone;
    final float triggerDeadzone;
    final InputDeviceContext defaultContext = new InputDeviceContext();
    volatile boolean layoutGamepadUsed;
    int layoutInputMap;
    short layoutLeftStickX, layoutLeftStickY, layoutRightStickX, layoutRightStickY;
    byte layoutLeftTrigger, layoutRightTrigger;
    final GameGestures gestures;
    final InputManager inputManager;
    final Vibrator deviceVibrator;
    volatile boolean deviceVibratorForPlayerOne;
    final Object rumbleLock = new Object();
    final Object usbRumbleLock = new Object();
    final Map<Short, short[]> rumbleLevels = new ConcurrentHashMap<>();
    final VibratorManager deviceVibratorManager;
    final SensorManager deviceSensorManager;
    final SceManager sceManager;
    final Handler mainThreadHandler;
    private final HandlerThread backgroundHandlerThread;
    final Handler backgroundThreadHandler;
    final MotionSensorThread motionSensorThread = new MotionSensorThread();
    final Object motionSendLock = new Object();
    boolean hasGameController, connectionStarted;
    volatile boolean stopped = false;
    volatile boolean sensorsEnabled = true;
    volatile boolean inputEnabled = true;
    private boolean sensorsOffForFocus;

    final PreferenceConfiguration prefConfig;
    final boolean touchpadButtonSession, onscreenOwnPlayerSession, steamControlsSession;
    volatile float steamPadTouchX = Float.NaN;
    final short deviceFallbackSlot;
    final InputDeviceContext handheldContext;
    final boolean rumbleAllowed;
    final RumblePolicy.Feel rumbleFeel;
    short currentControllers, initialControllers;

    PresenceListener presenceListener;
    boolean externalPresent;
    Runnable controllerAddedListener;
    volatile PadCapture padCapture;
    final AtomicInteger captureGeneration = new AtomicInteger();

    final ControllerDeviceClassifier classifier = new ControllerDeviceClassifier(this);
    final ControllerMotionBridge motion = new ControllerMotionBridge(this);
    final ControllerRumbleRouter rumble = new ControllerRumbleRouter(this);
    final ControllerSlotRouter slots = new ControllerSlotRouter(this);
    final ControllerContextLifecycle lifecycle = new ControllerContextLifecycle(this);
    final ControllerGamepadRouter gamepad = new ControllerGamepadRouter(this);
    final ControllerButtonRouter buttons = new ControllerButtonRouter(this);
    final ControllerUsbRouter usb = new ControllerUsbRouter(this);

    public ControllerHandler(Activity activityContext, NvConnection conn, GameGestures gestures, PreferenceConfiguration prefConfig) {
        this.activityContext = activityContext;
        this.conn = conn;
        this.gestures = gestures;
        this.prefConfig = prefConfig;
        this.touchpadButtonSession = prefConfig.showTouchpadButton && !prefConfig.onlyL3R3;
        this.onscreenOwnPlayerSession = onscreenOwnsPlayerOne(prefConfig);
        this.steamControlsSession = prefConfig.oscSteamControls;
        this.deviceFallbackSlot = (short) (onscreenOwnPlayerSession ? 1 : 0);
        this.rumbleAllowed = RumblePolicy.allows(prefConfig.disableRumble);
        this.rumbleFeel = RumblePolicy.Feel.fromKey(prefConfig.rumbleFeel);

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            this.deviceVibratorManager = (VibratorManager) activityContext.getSystemService(Context.VIBRATOR_MANAGER_SERVICE);
            this.deviceVibrator = this.deviceVibratorManager.getDefaultVibrator();
        } else {
            this.deviceVibratorManager = null;
            this.deviceVibrator = (Vibrator) activityContext.getSystemService(Context.VIBRATOR_SERVICE);
        }

        this.deviceSensorManager = (SensorManager) activityContext.getSystemService(Context.SENSOR_SERVICE);
        this.inputManager = (InputManager) activityContext.getSystemService(Context.INPUT_SERVICE);
        this.mainThreadHandler = new Handler(Looper.getMainLooper());
        this.backgroundHandlerThread = new HandlerThread("ControllerHandler");
        this.backgroundHandlerThread.start();
        this.backgroundThreadHandler = new Handler(backgroundHandlerThread.getLooper());
        this.sceManager = new SceManager(activityContext);
        this.sceManager.start();

        lifecycle.initAttachedGamepadFlag();

        // Negative values are an anti-deadzone, see applyStickDeadzone()
        this.stickDeadzone = (double) prefConfig.deadzonePercentage / 100.0;
        this.triggerDeadzone = prefConfig.triggerDeadzonePercentage / 100f;

        lifecycle.initNoDeviceContext(defaultContext, (short) 0);
        handheldContext = onscreenOwnPlayerSession || !prefConfig.sendControllers ? new InputDeviceContext() : defaultContext;
        if (handheldContext != defaultContext) lifecycle.initNoDeviceContext(handheldContext, deviceFallbackSlot);

        initialControllers = getAttachedControllerMask(activityContext);
        if (onscreenOwnPlayerSession) currentControllers |= 1;
        slotStateHandle = NightjarInputJni.slotStateCreate(
                initialControllers, onscreenOwnPlayerSession, touchpadButtonSession, steamControlsSession);
        usbInputStateHandle = NightjarInputJni.usbInputStateCreate();
        rumbleStateHandle = NightjarInputJni.rumbleStateCreate(
                rumbleAllowed, rumbleFeel.ordinal(), deviceFallbackSlot);

        inputManager.registerInputDeviceListener(this, null);
    }

    void quitBackgroundThread() { backgroundHandlerThread.quit(); }

    /** Tells a listener when a physical controller comes or goes, so the on-screen controls can step aside. */
    public interface PresenceListener {
        void onControllerPresenceChanged(boolean connected);
    }

    /** Takes over the buttons and left stick of USB-driver controllers while the on-screen PC keyboard is open. */
    public interface PadCapture {
        void button(int flag, boolean down);
        void stick(int controllerId, float x, float y);
    }

    /** What a controller's battery reported, in BatteryState's terms. */
    static final class BatteryReading {
        final int status;
        final float capacity;

        BatteryReading(int status, float capacity) {
            this.status = status;
            this.capacity = capacity;
        }
    }

    @Override public synchronized void onInputDeviceAdded(int deviceId) { slots.onInputDeviceAdded(deviceId); }
    @Override public synchronized void onInputDeviceRemoved(int deviceId) { slots.onInputDeviceRemoved(deviceId); }
    @Override public synchronized void onInputDeviceChanged(int deviceId) { slots.onInputDeviceChanged(deviceId); }

    public synchronized void setPresenceListener(PresenceListener listener) {
        presenceListener = listener;
        externalPresent = hasExternalControllers();
    }

    public synchronized boolean hasExternalControllers() { return classifier.hasExternalControllers(); }
    public synchronized void stop() { lifecycle.stopHandler(); }
    public void destroy() { lifecycle.destroyHandler(); }
    public boolean hasUsbControllers() { return !usbDeviceContexts.isEmpty(); }
    public void resyncPadCapture() { captureGeneration.incrementAndGet(); }

    public void setPadCapture(PadCapture capture) {
        padCapture = capture;
        if (capture != null) gamepad.releaseInput(false);
    }

    public void setInputEnabled(boolean enabled) {
        inputEnabled = enabled;
        if (!enabled) {
            releaseInput();
            if (sensorsEnabled && !stopped) {
                motion.stopSensors(false);
                sensorsOffForFocus = true;
            }
        } else if (sensorsOffForFocus) {
            sensorsOffForFocus = false;
            enableSensors();
        }
        backgroundThreadHandler.post(() -> pauseRumble(!enabled));
    }

    @SuppressWarnings("unused") // Invoked via reflection by unit tests
    private void pauseRumble(boolean paused) { rumble.pauseRumble(paused); }

    public void releaseInput() { gamepad.releaseInput(true); }

    public void disableSensors() {
        sensorsOffForFocus = false;
        motion.stopSensors(true);
    }

    public void enableSensors() { motion.enableSensors(); }
    public synchronized void setControllerAddedListener(Runnable listener) { controllerAddedListener = listener; }
    protected boolean[] findInternalGamepadAndSelect() { return classifier.findInternalGamepadAndSelect(); }
    public boolean handleHandheldBackAsGuide(KeyEvent event) { return buttons.handleHandheldBackAsGuide(event); }
    InputDeviceContext createInputDeviceContextForDevice(InputDevice dev) { return classifier.createInputDeviceContextForDevice(dev); }
    InputDeviceContext getContextForEvent(InputEvent event) { return gamepad.getContextForEvent(event); }
    void assignControllerNumberIfNeeded(GenericControllerContext context) { slots.assignControllerNumberIfNeeded(context); }
    synchronized short getActiveControllerMask() { return slots.getActiveControllerMask(); }
    @SuppressLint("InlinedApi")
    void sendControllerBatteryPacket(InputDeviceContext context, BatteryReading battery) { lifecycle.sendControllerBatteryPacket(context, battery); }
    void sendControllerInputPacket(GenericControllerContext originalContext) { gamepad.sendControllerInputPacket(originalContext); }

    public synchronized int userMappedKey(KeyEvent event) {
        InputDeviceContext context = inputDeviceContexts.get(event.getDeviceId());
        return context == null ? ControllerButtonMap.UNMAPPED :
                context.buttonMap.mapEvent(event.getKeyCode(), event.getScanCode());
    }

    public boolean tryHandleTouchpadEvent(MotionEvent event) { return gamepad.tryHandleTouchpadEvent(event); }
    public boolean handleMotionEvent(MotionEvent event) { return gamepad.handleMotionEvent(event); }
    public void handleRumble(short controllerNumber, short lowFreqMotor, short highFreqMotor) { rumble.handleRumble(controllerNumber, lowFreqMotor, highFreqMotor); }
    public void handleRumbleTriggers(short controllerNumber, short leftTrigger, short rightTrigger) { rumble.handleRumbleTriggers(controllerNumber, leftTrigger, rightTrigger); }
    public String prepareControllersForLaunch() { return slots.prepareControllersForLaunch(); }
    public synchronized void resendControllerArrivals() { slots.resendControllerArrivals(); }
    public void handleControllerAudio(int kind, byte[] payload) { usb.handleControllerAudio(kind, payload); }
    public AndroidAudioRenderer.HeadphoneSink headphoneAudioSink() { return usb.headphoneAudioSink(); }
    public boolean routeHeadphoneAudio(short[] samples, int length, float volume) { return usb.routeHeadphoneAudio(samples, length, volume); }
    public void handleControllerHaptics(short controllerNumber, int sequence, byte[] pcm) { usb.handleControllerHaptics(controllerNumber, sequence, pcm); }

    public synchronized void handleSetAdaptiveTriggers(short controllerNumber, byte eventFlags, byte typeLeft,
                                                       byte typeRight, byte[] left, byte[] right) {
        usb.handleSetAdaptiveTriggers(controllerNumber, eventFlags, typeLeft, typeRight, left, right);
    }

    boolean checksGyroLiveness(short controllerNumber, SensorManager controllerSensors) { return motion.checksGyroLiveness(controllerNumber, controllerSensors); }
    void fallBackToDeviceSensors(InputDeviceContext context) { motion.fallBackToDeviceSensors(context); }
    public void handleSetMotionEventState(final short controllerNumber, final byte motionType, short reportRateHz) { motion.handleSetMotionEventState(controllerNumber, motionType, reportRateHz); }
    public synchronized void handleSetControllerLED(short controllerNumber, byte r, byte g, byte b) { usb.handleSetControllerLED(controllerNumber, r, g, b); }
    public boolean handleButtonUp(KeyEvent event) { return buttons.handleButtonUp(event); }
    public boolean handleButtonDown(KeyEvent event) { return buttons.handleButtonDown(event); }

    public void reportOscState(int buttonFlags, short leftStickX, short leftStickY,
                               short rightStickX, short rightStickY, byte leftTrigger, byte rightTrigger) {
        if (stopped) return;
        defaultContext.leftStickX = leftStickX;
        defaultContext.leftStickY = leftStickY;
        defaultContext.rightStickX = rightStickX;
        defaultContext.rightStickY = rightStickY;
        defaultContext.leftTrigger = leftTrigger;
        defaultContext.rightTrigger = rightTrigger;
        defaultContext.inputMap = buttonFlags;
        sendControllerInputPacket(defaultContext);
    }

    public void reportOscTouch(byte eventType, int pointerId, float x, float y, float pressure) {
        if (stopped) return;
        conn.sendControllerTouchEvent((byte) defaultContext.controllerNumber, eventType, pointerId, x, y, pressure);
    }

    public void reportLayoutGamepadState(int buttonFlags, short leftStickX, short leftStickY,
                                         short rightStickX, short rightStickY, byte leftTrigger, byte rightTrigger) {
        if (stopped) return;
        layoutGamepadUsed = true;
        layoutInputMap = buttonFlags;
        layoutLeftStickX = leftStickX;
        layoutLeftStickY = leftStickY;
        layoutRightStickX = rightStickX;
        layoutRightStickY = rightStickY;
        layoutLeftTrigger = leftTrigger;
        layoutRightTrigger = rightTrigger;
        sendControllerInputPacket(defaultContext);
    }

    public boolean isMouseEmulationActive() { return gamepad.isMouseEmulationActive(); }

    public boolean hasControllers() {
        return (!inputDeviceContexts.isEmpty() || !usbDeviceContexts.isEmpty())
                && (prefConfig.sendControllers || isMouseEmulationActive());
    }

    /**
     * The stream menu's controller mouse switch, as in Moonlight V+ PC: every active controller
     * mouse turns off, or the first controller's turns on. It works with the Start long press off.
     */
    public void toggleMouseEmulationFromMenu() { gamepad.toggleMouseEmulationFromMenu(); }

    @Override
    public void reportControllerState(int controllerId, int buttonFlags, float leftStickX, float leftStickY,
                                      float rightStickX, float rightStickY, float leftTrigger, float rightTrigger) {
        usb.reportControllerState(controllerId, buttonFlags, leftStickX, leftStickY, rightStickX, rightStickY, leftTrigger, rightTrigger);
    }

    @Override public void reportControllerTouch(int controllerId, byte eventType, int pointerId, float x, float y, float pressure) { usb.reportControllerTouch(controllerId, eventType, pointerId, x, y, pressure); }
    @Override public void reportControllerMotion(int controllerId, byte motionType, float x, float y, float z) { usb.reportControllerMotion(controllerId, motionType, x, y, z); }
    @Override public void reportControllerBattery(int controllerId, byte state, byte percentage) { usb.reportControllerBattery(controllerId, state, percentage); }
    @Override public void deviceStartFailed(int usbDeviceId) { failedUsbDevices.add(usbDeviceId); }
    @Override public synchronized void deviceRemoved(AbstractController controller) { usb.deviceRemoved(controller); }
    @Override public synchronized void deviceAdded(AbstractController controller) { usb.deviceAdded(controller); }

    static boolean countsAsExternalController(boolean external, boolean gamepad, boolean remote, boolean keyboard, boolean handheldControls) {
        return NightjarInputJni.countsAsExternalController(external, gamepad, remote, keyboard, handheldControls);
    }

    static boolean isController(InputDevice device) { return ControllerDeviceClassifier.isController(device); }
    public static boolean isRemoteControlEvent(KeyEvent event) { return ControllerDeviceClassifier.isRemoteControlEvent(event); }
    public static boolean isGameControllerDevice(InputDevice device) { return ControllerDeviceClassifier.isGameControllerDevice(device); }
    public static short getAttachedControllerMask(Context context) { return ControllerDeviceClassifier.getAttachedControllerMask(context); }
    public static short getAttachedPlayStationMask(Context context) { return ControllerDeviceClassifier.getAttachedPlayStationMask(context); }

    static boolean onscreenOwnsPlayerOne(PreferenceConfiguration config) {
        return NightjarInputJni.onscreenOwnsPlayerOne(config.onscreenOwnPlayer, config.multiController, config.onscreenController, config.onlyL3R3);
    }

    static boolean isBackAsGuide(boolean backAsGuide, int keyCode, int flags) { return NightjarInputJni.isBackAsGuide(backAsGuide, keyCode, flags); }
    static boolean isPadBackAsGuide(boolean backAsGuide, int keyCode, int flags, boolean backIsStart) { return NightjarInputJni.isPadBackAsGuide(backAsGuide, keyCode, flags, backIsStart); }

    public static boolean isHandheldSystemBack(int keyCode, int flags, int source, boolean virtualDevice,
                                               boolean external, boolean alphabeticKeyboard, boolean gamepad,
                                               boolean backIsSpareButton) {
        return NightjarInputJni.isHandheldSystemBack(keyCode, flags, source, virtualDevice, external, alphabeticKeyboard, gamepad, backIsSpareButton);
    }

    static boolean backIsSpareButton(boolean internalGamepad, boolean internalSelect, boolean anyGamepad) {
        return NightjarInputJni.backIsSpareButton(internalGamepad, internalSelect, anyGamepad);
    }

    @SuppressWarnings("unused") // Invoked via reflection by unit tests
    private static byte maxByMagnitude(byte a, byte b) { return NightjarInputJni.maxByMagnitudeByte(a, b); }

    @SuppressWarnings("unused") // Invoked via reflection by unit tests
    private static short maxByMagnitude(short a, short b) { return NightjarInputJni.maxByMagnitudeShort(a, b); }

    static boolean looksLikeHandheldControls(String name, int vendorId, boolean onlyGamepad, boolean attachedUsb) { return NightjarInputJni.looksLikeHandheldControls(name, vendorId, onlyGamepad, attachedUsb); }
    static int onscreenPadButtons(boolean touchpadButton, boolean steamControls) { return NightjarInputJni.onscreenPadButtons(touchpadButton, steamControls); }
    static short onscreenPadCapabilities(boolean steamControls) { return NightjarInputJni.onscreenPadCapabilities(steamControls); }
    static boolean carriesSteamControls(boolean steamControls, boolean onscreenOwnPlayer, short controllerNumber) { return NightjarInputJni.carriesSteamControls(steamControls, onscreenOwnPlayer, controllerNumber); }
    static byte arrivalType(byte type, ControllerButtonMap map) { return NightjarInputJni.arrivalType(type, map.connectAs(), map.targetButtonFlags()); }
    static byte inputDeviceArrivalType(byte detected, boolean hasMotion, ControllerButtonMap map) { return NightjarInputJni.inputDeviceArrivalType(detected, hasMotion, map.connectAs(), map.targetButtonFlags()); }
    static boolean dropsMotionAndTouchpad(ControllerButtonMap map) { return NightjarInputJni.dropsMotionAndTouchpad(map.connectAs()); }
    static short arrivalCapabilities(short capabilities, ControllerButtonMap map) { return NightjarInputJni.arrivalCapabilities(capabilities, map.connectAs()); }
    static boolean takesDeviceMotion(boolean fallbackToDevice, short controllerNumber, short padCapabilities, ControllerButtonMap map) { return NightjarInputJni.takesDeviceMotion(fallbackToDevice, controllerNumber, padCapabilities, map.connectAs()); }
    static boolean motionOverridesType(byte detected, boolean hasMotion, ControllerButtonMap map) { return NightjarInputJni.motionOverridesType(detected, hasMotion, map.connectAs()); }
    static boolean needsClickpadEmulation(ControllerButtonMap map) { return NightjarInputJni.needsClickpadEmulation(map.targetButtonFlags()); }

    static boolean updateMappedButton(Map<Integer, Integer> pressedButtons, int physicalKey, int target, boolean down) {
        int normalized = NightjarInputJni.normalizeMappedTarget(target);
        if (normalized == 0) return true;
        if (down) {
            boolean alreadyPressed = pressedButtons.containsValue(normalized);
            pressedButtons.put(physicalKey, normalized);
            return !alreadyPressed;
        }
        pressedButtons.remove(physicalKey);
        return !pressedButtons.containsValue(normalized);
    }

    static int heldButtons(InputDeviceContext context) {
        if ((context.emulatingButtonFlags & EMULATING_TOUCHPAD) == 0) return context.inputMap;
        return NightjarInputJni.heldButtons(
                context.inputMap, context.emulatingButtonFlags,
                context.pressedButtons.containsValue(ControllerButtonMap.TARGET_TOUCHPAD),
                context.pressedButtons.containsValue(KeyEvent.KEYCODE_BUTTON_SELECT),
                context.pressedButtons.containsValue(KeyEvent.KEYCODE_BUTTON_L1));
    }

    static float applyTriggerDeadzone(float value, float deadzone) { return NightjarInputJni.applyTriggerDeadzone(value, deadzone); }

    // Positive radius: inputs inside it are dropped. We're not normalizing here because we let the
    // computer handle the deadzones. Normalizing can make the deadzones larger than they should
    // be after the computer also evaluates the deadzone.
    // Zero or negative radius (anti-deadzone, as in Artemis): the magnitude is remapped from
    // [0.01, 1] to [|radius|, 1], so small movements jump past a controller's or game's own deadzone.
    static void applyStickDeadzone(Vector2d stickVector, float deadzoneRadius) {
        float[] out = new float[2];
        NightjarInputJni.applyStickDeadzone(stickVector.getX(), stickVector.getY(), deadzoneRadius, out);
        stickVector.initialize(out[0], out[1]);
    }

    /**
     * The buttons of the controller quit combo chosen in Settings, or 0 when it is off (the default:
     * Start+Select+LB+RB opens the stream menu here). The choices are Moonlight V+ PC's
     * (qiin2333/moonlight-qt, gamepad.cpp), less its default, which is the menu combo on Android.
     */
    static int quitComboMask(String combo) { return NightjarInputJni.quitComboMask(combo); }
    static int quitButton(int keyCode) { return NightjarInputJni.quitButtonFlag(keyCode); }

    class GenericControllerContext {
        public int id, vendorId, productId;
        public boolean external, reservedControllerNumber;
        public float leftStickDeadzoneRadius, rightStickDeadzoneRadius, triggerDeadzone;
        public volatile boolean assignedControllerNumber;
        public short controllerNumber;
        public ControllerAxisMap axisMap = new ControllerAxisMap();
        public final ControllerAxisMap.Report mappedReport = new ControllerAxisMap.Report();
        public int inputMap = 0;
        public byte leftTrigger = 0x00, rightTrigger = 0x00;
        public short rightStickX = 0x0000, rightStickY = 0x0000, leftStickX = 0x0000, leftStickY = 0x0000;
        public volatile boolean mouseEmulationActive;
        boolean quitComboPending, guideTapping;
        int quitButtons;
        final GuideHold guideHold = new GuideHold();
        final Runnable guideHoldRunnable = () -> {
            if (!stopped && guideHold.expire() && activityContext instanceof Game) {
                ((Game) activityContext).showStreamMenuFromController();
            }
        };
        Runnable guideTapEnd;
        KeyEvent guideHeldDown;
        public int mouseEmulationLastInputMap;
        public boolean mouseEmulationFixedStepHeld;
        public int mouseEmulationStep = FixedStepMouse.MIN_STEP;
        public final int mouseEmulationReportPeriod = ControllerMouseTick.REPORT_PERIOD_MS;
        final float[] mouseTickFloatOut = new float[6];
        final MouseDeltaAccumulator mouseMoveAccumX = new MouseDeltaAccumulator();
        final MouseDeltaAccumulator mouseMoveAccumY = new MouseDeltaAccumulator();
        final MouseDeltaAccumulator mouseScrollVAccum = new MouseDeltaAccumulator();
        final MouseDeltaAccumulator mouseScrollHAccum = new MouseDeltaAccumulator();
        long mouseEmulationLastTickNs;
        final int[] mousePacketOut = new int[5];
        final Object mousePacketLock = new Object();

        void resetMouseEmulationState() {
            synchronized (mousePacketLock) {
                mouseMoveAccumX.reset();
                mouseMoveAccumY.reset();
                mouseScrollVAccum.reset();
                mouseScrollHAccum.reset();
                mouseEmulationLastTickNs = 0L;
            }
        }

        public final Runnable mouseEmulationRunnable = new Runnable() {
            @Override public void run() { gamepad.runMouseEmulationTick(GenericControllerContext.this, this); }
        };

        public void toggleMouseEmulation() { gamepad.toggleMouseEmulation(this); }

        public void destroy() {
            mouseEmulationActive = false;
            resetMouseEmulationState();
            gamepad.releaseMouseButtons(this);
            mainThreadHandler.removeCallbacks(mouseEmulationRunnable);
            mainThreadHandler.removeCallbacks(guideHoldRunnable);
            guideHold.reset();
            if (guideTapEnd != null) {
                mainThreadHandler.removeCallbacks(guideTapEnd);
                guideTapEnd = null;
            }
            guideTapping = false;
        }

        public void sendControllerArrival() {}
    }

    class InputDeviceContext extends GenericControllerContext {
        public String name;
        byte reportedType;
        short reportedCapabilities;
        int supportedButtonFlags;
        public ControllerButtonMap buttonMap = new ControllerButtonMap();
        Boolean handheldControls;
        public final Map<Integer, Integer> pressedButtons = new HashMap<>();
        public final Map<Integer, Runnable> pendingButtonReleases = new HashMap<>();
        public VibratorManager vibratorManager;
        public Vibrator vibrator;
        public boolean quadVibrators, hasRgbLed, triggersIdleNegative, leftTriggerAxisUsed, rightTriggerAxisUsed, hatXAxisUsed, hatYAxisUsed;
        public short lowFreqMotor, highFreqMotor, leftTriggerMotor, rightTriggerMotor, gyroReportRateHz, accelReportRateHz;
        public SensorManager sensorManager;
        public ControllerMotionBridge.MotionListener gyroListener, accelListener;
        public InputDevice inputDevice;
        public LightsManager.LightsSession lightsSession;
        public int ledArgbValue, lastReportedBatteryStatus;
        public float lastReportedBatteryCapacity;
        public int leftStickXAxis = -1, leftStickYAxis = -1, rightStickXAxis = -1, rightStickYAxis = -1;
        public int leftTriggerAxis = -1, rightTriggerAxis = -1, hatXAxis = -1, hatYAxis = -1;
        InputDevice.MotionRange touchpadXRange, touchpadYRange, touchpadPressureRange;
        public boolean isNonStandardDualShock4, usesLinuxGamepadStandardFaceButtons, isNonStandardXboxBtController;
        public boolean isServal, backIsStart, modeIsSelect, searchIsMode, ignoreBack, hasJoystickAxes, pendingExit, isDualShockStandaloneTouchpad;
        public int emulatingButtonFlags = 0;
        public boolean hasSelect, hasMode, hasPaddles, hasShare, needsClickpadEmulation, hasHardwareDpadKeys;
        public long lastLbUpTime = 0, lastRbUpTime = 0, startDownTime = 0;
        boolean destroyed;

        public final Runnable batteryStateUpdateRunnable = new Runnable() {
            @Override public void run() { lifecycle.runBatteryStateUpdate(InputDeviceContext.this, this); }
        };

        public final Runnable enableSensorRunnable = new Runnable() {
            @Override public void run() {
                if (destroyed || stopped || !sensorsEnabled) return;
                if (accelReportRateHz != 0 && accelListener == null) handleSetMotionEventState(controllerNumber, MoonBridge.LI_MOTION_TYPE_ACCEL, accelReportRateHz);
                if (gyroReportRateHz != 0 && gyroListener == null) handleSetMotionEventState(controllerNumber, MoonBridge.LI_MOTION_TYPE_GYRO, gyroReportRateHz);
            }
        };

        public final Runnable setLedStateRunnable = new Runnable() {
            @TargetApi(Build.VERSION_CODES.S)
            @Override public void run() { lifecycle.runLedStateUpdate(InputDeviceContext.this); }
        };

        void cancelMotors() {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && vibratorManager != null) vibratorManager.cancel();
            else if (vibrator != null) vibrator.cancel();
        }

        void destroyGenericBase() { super.destroy(); }
        @Override public synchronized void destroy() { lifecycle.destroyInputDeviceContext(this); }
        @Override public void sendControllerArrival() { lifecycle.sendInputDeviceArrival(this); }
        public void migrateContext(InputDeviceContext oldContext) { lifecycle.migrateInputDeviceContext(this, oldContext); }
        public void disableSensors() { disableSensors(true); }
        void disableSensors(boolean atRest) { motion.disableInputContextSensors(this, atRest); }
        public void enableSensors() { mainThreadHandler.postDelayed(enableSensorRunnable, 1000); }
    }

    class UsbDeviceContext extends GenericControllerContext {
        public AbstractController device;
        public ControllerButtonMap buttonMap = new ControllerButtonMap();
        GyroAxisTransformer gyroMapping = GyroAxisTransformer.identity();
        final Object usbSendLock = new Object();
        final float[] mappedMotion = new float[3];
        boolean arrivalPending, menuComboPending, guideDown;
        long arrivalRetryAt;
        int clickpadSwallowed, lastRawButtons;
        final int[] usbAxesOut = new int[6], clickpadOut = new int[2], guideHoldOut = new int[5], captureOut = new int[4], steerKeysOut = new int[11];
        float lastRawLeftStickX, lastRawLeftStickY, lastRawRightStickX, lastRawRightStickY, lastRawLeftTrigger, lastRawRightTrigger;
        SensorManager deviceSensors;
        ControllerMotionBridge.MotionListener gyroListener, accelListener;
        short gyroReportRateHz, accelReportRateHz;

        byte arrivalType() { return inputDeviceArrivalType(device.getType(), deviceSensors != null, buttonMap); }

        int clickpadButtons(int buttons) {
            NightjarInputJni.applyUsbClickpad(
                    buttons, clickpadSwallowed, device.getType(), buttonMap.connectAs(),
                    deviceSensors != null, needsClickpadEmulation(buttonMap), clickpadOut);
            clickpadSwallowed = clickpadOut[1];
            return clickpadOut[0];
        }

        short capabilities() { return lifecycle.usbCapabilities(this); }

        boolean ensureControllerArrival() {
            if (arrivalPending && android.os.SystemClock.uptimeMillis() >= arrivalRetryAt) sendControllerArrival();
            return !arrivalPending;
        }

        @Override
        public void destroy() {
            super.destroy();
            motion.unregisterUsbDeviceMotion(this);
            waveformControllers.remove((short) controllerNumber, device);
        }

        @Override public void sendControllerArrival() { lifecycle.sendUsbDeviceArrival(this); }
    }
}
