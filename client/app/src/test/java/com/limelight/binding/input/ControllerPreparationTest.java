package com.limelight.binding.input;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.hardware.usb.UsbManager;
import android.hardware.input.InputManager;
import android.view.InputDevice;
import android.view.MotionEvent;
import com.limelight.binding.input.driver.AbstractController;
import com.limelight.binding.input.driver.UsbDriverService;
import com.limelight.binding.input.driver.UsbPads;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.android.controller.ServiceController;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.shadows.ShadowUsbManager;
import org.robolectric.shadows.ShadowInputDevice;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;
import com.limelight.nvstream.input.ControllerPacket;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {ControllerPreparationTest.ControllerBridge.class, UsbPads.Connection.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ControllerPreparationTest {
    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class ControllerBridge {
        @Implementation protected static void __staticInitializer__() { }
        @Implementation protected static boolean guessControllerHasPaddles(int vendor, int product) { return false; }
        @Implementation protected static boolean guessControllerHasShareButton(int vendor, int product) { return false; }
        @Implementation protected static byte guessControllerType(int vendor, int product) { return MoonBridge.LI_CTYPE_UNKNOWN; }
    }

    @Test public void builtInAndExternalPadsArePreparedBeforeAnyInputWithoutUsbHaptics() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 4, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 8, false, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 9, true, InputDevice.SOURCE_DPAD);
            Connection connection = new Connection(73);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertEquals(2, prepared.length);
                assertTrue(prepared[0].startsWith("0:1:"));
                assertTrue(prepared[1].startsWith("1:1:"));
                assertEquals(2, connection.arrivals);
                assertEquals(0, connection.inputs);
                handler.resendControllerArrivals();
                assertEquals(4, connection.arrivals);
                assertEquals(3, connection.inputs);
                assertEquals(3, connection.activeMask & 3);
                java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                        ReflectionHelpers.getField(handler, "inputDeviceContexts");
                // Built-in controls share player 1 with the first external pad, as upstream.
                assertEquals(0, contexts.get(8).controllerNumber);
                assertFalse(contexts.get(8).reservedControllerNumber);
                assertEquals(0, contexts.get(2).controllerNumber);
                assertEquals(1, contexts.get(4).controllerNumber);
                assertFalse(contexts.containsKey(9));
            } finally { handler.destroy(); }
        }
    }

    @Test public void builtInControlsAloneStayActiveAsPlayerOne() {
        // A handheld whose built-in controls have no sticks: player 1 must stay in the
        // active mask, or hosts drop every packet.
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 1, false, InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_DPAD);
            Connection connection = new Connection(73);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.resendControllerArrivals();
                java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                        ReflectionHelpers.getField(handler, "inputDeviceContexts");
                // What a first button press does (Robolectric's KeyEvent has no device).
                ControllerHandler.InputDeviceContext builtIn = ReflectionHelpers.callInstanceMethod(handler,
                        "createInputDeviceContextForDevice",
                        ReflectionHelpers.ClassParameter.from(InputDevice.class, InputDevice.getDevice(1)));
                contexts.put(1, builtIn);
                ReflectionHelpers.callInstanceMethod(handler, "assignControllerNumberIfNeeded",
                        ReflectionHelpers.ClassParameter.from(ControllerHandler.GenericControllerContext.class, builtIn));
                builtIn.inputMap = ControllerPacket.A_FLAG;
                ReflectionHelpers.callInstanceMethod(handler, "sendControllerInputPacket",
                        ReflectionHelpers.ClassParameter.from(ControllerHandler.GenericControllerContext.class, builtIn));
                assertEquals(0, contexts.get(1).controllerNumber);
                assertFalse(contexts.get(1).reservedControllerNumber);
                assertTrue(connection.inputs > 0);
                assertEquals(1, connection.activeMask & 1);
                handler.onInputDeviceRemoved(1);
                assertEquals(0, connection.activeMask);
            } finally { handler.destroy(); }
        }
    }

    @Test public void aHostWithoutLaunchPreparationGetsAttachedAndNewPadsAtConnectionStart() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            Connection connection = new Connection(73);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                handler.resendControllerArrivals();
                assertEquals(1, connection.arrivals);
                assertEquals(1, connection.inputs);
                addInputDevice(activity.get(), 3, true, InputDevice.SOURCE_GAMEPAD);
                handler.onInputDeviceAdded(3);
                assertEquals(2, connection.arrivals);
                assertEquals(2, connection.inputs);
                assertEquals(3, connection.activeMask);
            } finally { handler.destroy(); }
        }
    }

    @Test public void sharedPlayerOneIsPreparedOnlyOnce() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 3, true, InputDevice.SOURCE_GAMEPAD);
            ControllerHandler handler = handler(activity.get(), new Connection(73));
            try {
                handler.deviceAdded(new Pad());
                assertEquals(1, handler.prepareControllersForLaunch().split(",").length);
            } finally { handler.destroy(); }
        }
    }

    @Test public void aBuiltInDeviceWithOnlyGamepadKeysLeavesPlayerOneToTheRealPad() {
        // #29 and #30: a tablet or TV box exposes a built-in device that advertises gamepad
        // keys. It took player 1 at launch, so games ignored the real controller as player 2.
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 1, false, InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_KEYBOARD);
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            Connection connection = new Connection(73);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertEquals(1, prepared.length);
                assertTrue(prepared[0].startsWith("0:"));
                handler.resendControllerArrivals();
                assertEquals(1, connection.activeMask);
                java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                        ReflectionHelpers.getField(handler, "inputDeviceContexts");
                assertEquals(0, contexts.get(2).controllerNumber);
                assertFalse(contexts.containsKey(1));
                int arrivals = connection.arrivals;
                // Its first gamepad key mid-game shares player 1 and does not re-announce the slot.
                ControllerHandler.InputDeviceContext builtIn = ReflectionHelpers.callInstanceMethod(handler,
                        "createInputDeviceContextForDevice",
                        ReflectionHelpers.ClassParameter.from(InputDevice.class, InputDevice.getDevice(1)));
                contexts.put(1, builtIn);
                ReflectionHelpers.callInstanceMethod(handler, "assignControllerNumberIfNeeded",
                        ReflectionHelpers.ClassParameter.from(ControllerHandler.GenericControllerContext.class, builtIn));
                assertEquals(0, builtIn.controllerNumber);
                assertFalse(builtIn.reservedControllerNumber);
                assertEquals(arrivals, connection.arrivals);
            } finally { handler.destroy(); }
        }
    }

    @Test public void aVirtualBuiltInGamepadDoesNotPushTheRealPadToPlayerTwo() {
        // Even a built-in device with sticks (a vendor game-mode uinput pad) only shares player 1.
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 1, false, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            Connection connection = new Connection(73);
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                assertEquals(1, prepared.length);
                assertTrue(prepared[0].startsWith("0:"));
                assertEquals(1, connection.arrivals);
                handler.resendControllerArrivals();
                assertEquals(2, connection.arrivals);
                java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                        ReflectionHelpers.getField(handler, "inputDeviceContexts");
                assertEquals(0, contexts.get(1).controllerNumber);
                assertEquals(0, contexts.get(2).controllerNumber);
                // When the external pad leaves, the built-in controls announce player 1 again.
                handler.onInputDeviceRemoved(2);
                assertEquals(3, connection.arrivals);
                assertEquals(1, connection.activeMask & 1);
            } finally { handler.destroy(); }
        }
    }

    @Test public void aRealPadGoesAheadOfAnExternalButtonOnlyDevice() {
        // A remote advertising gamepad keys gets a slot after any real pad attached at launch.
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_GAMEPAD | InputDevice.SOURCE_DPAD);
            addInputDevice(activity.get(), 5, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), new Connection(73), null, prefs);
            try {
                handler.prepareControllersForLaunch();
                java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                        ReflectionHelpers.getField(handler, "inputDeviceContexts");
                assertEquals(0, contexts.get(5).controllerNumber);
                assertEquals(1, contexts.get(2).controllerNumber);
            } finally { handler.destroy(); }
        }
    }

    @Test public void aPinnedTypeReachesTheLaunchListAndBeatsTheMotionOverrideWithoutAToast() {
        // #53: a Razer Kishi is unknown to SDL and gets motion from the tablet, so Automatic
        // makes the host create a PlayStation pad; the user pins it.
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            addInputDevice(activity.get(), 2, true, InputDevice.SOURCE_JOYSTICK | InputDevice.SOURCE_GAMEPAD);
            ControllerButtonMap stored = new ControllerButtonMap();
            stored.setConnectAs(ControllerButtonMap.CONNECT_AS_PLAYSTATION);
            stored.save(activity.get(), ControllerButtonMap.deviceKey(0, 0, "Controller 2"));
            Connection connection = new Connection(73);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            try {
                String[] prepared = handler.prepareControllersForLaunch().split(",");
                java.util.Map<Integer, ControllerHandler.InputDeviceContext> contexts =
                        ReflectionHelpers.getField(handler, "inputDeviceContexts");
                ControllerHandler.InputDeviceContext context = contexts.get(2);
                // A pinned type with no button mappings is stored and loaded.
                assertEquals(ControllerButtonMap.CONNECT_AS_PLAYSTATION, context.buttonMap.connectAs());
                assertEquals(MoonBridge.LI_CTYPE_PS, connection.arrivalType);
                assertEquals(1, prepared.length);
                assertTrue(prepared[0], prepared[0].startsWith("0:2:"));
                // Pinned as PlayStation without a touchpad click: Select+LB stands in for it.
                assertTrue(context.needsClickpadEmulation);
                context.needsClickpadEmulation = false;
                context.sensorManager = (android.hardware.SensorManager)
                        activity.get().getSystemService(Context.SENSOR_SERVICE);
                context.buttonMap.setConnectAs(ControllerButtonMap.CONNECT_AS_XBOX);
                // Idling the main looper would also run the Shield service binding; run only what the arrival posts.
                MainThreadPosts posts = new MainThreadPosts();
                ReflectionHelpers.setField(handler, "mainThreadHandler", posts);
                org.robolectric.shadows.ShadowToast.reset();
                context.sendControllerArrival();
                posts.runAll();
                assertEquals(MoonBridge.LI_CTYPE_XBOX, connection.arrivalType);
                assertNull(org.robolectric.shadows.ShadowToast.getLatestToast());
                assertFalse(context.needsClickpadEmulation);

                // Automatic keeps the motion override and its toast.
                context.buttonMap = new ControllerButtonMap();
                context.sendControllerArrival();
                posts.runAll();
                assertEquals(MoonBridge.LI_CTYPE_UNKNOWN, connection.arrivalType);
                assertNotNull(org.robolectric.shadows.ShadowToast.getLatestToast());
                assertTrue(context.needsClickpadEmulation);
            } finally {
                handler.destroy();
                activity.get().getSharedPreferences("ControllerButtonMaps", Context.MODE_PRIVATE).edit().clear().commit();
            }
        }
    }

    /** Keeps what is posted to the main thread, to run it without idling the looper. */
    private static class MainThreadPosts extends android.os.Handler {
        final List<Runnable> posted = new ArrayList<>();
        MainThreadPosts() { super(android.os.Looper.getMainLooper()); }
        @Override public boolean sendMessageAtTime(android.os.Message message, long uptimeMillis) {
            posted.add(message.getCallback()); return true;
        }
        void runAll() {
            List<Runnable> run = new ArrayList<>(posted);
            posted.clear();
            for (Runnable runnable : run) if (runnable != null) runnable.run();
        }
    }

    private static void addInputDevice(Activity activity, int id, boolean external, int sources) {
        InputDevice device = ShadowInputDevice.makeInputDeviceNamed("Controller " + id);
        ReflectionHelpers.setField(device, "mId", id);
        ReflectionHelpers.setField(device, "mDescriptor", "controller-" + id);
        ReflectionHelpers.setField(device, "mIsExternal", external);
        ReflectionHelpers.setField(device, "mSources", sources);
        ReflectionHelpers.setField(device, "mMotionRanges", new ArrayList<InputDevice.MotionRange>());
        if ((sources & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK) {
            for (int axis : new int[] {MotionEvent.AXIS_X, MotionEvent.AXIS_Y}) {
                ReflectionHelpers.callInstanceMethod(device, "addMotionRange",
                        ReflectionHelpers.ClassParameter.from(int.class, axis),
                        ReflectionHelpers.ClassParameter.from(int.class, InputDevice.SOURCE_JOYSTICK),
                        ReflectionHelpers.ClassParameter.from(float.class, -1f),
                        ReflectionHelpers.ClassParameter.from(float.class, 1f),
                        ReflectionHelpers.ClassParameter.from(float.class, 0f),
                        ReflectionHelpers.ClassParameter.from(float.class, 0f),
                        ReflectionHelpers.ClassParameter.from(float.class, 0f));
            }
        }
        org.robolectric.Shadows.shadowOf((InputManager) activity.getSystemService(Context.INPUT_SERVICE))
                .addInputDevice(device);
    }

    @Test public void preparationDoesNotSelectTheMicrophoneBeforeAudioNegotiation() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection(73);
            ControllerHandler handler = handler(activity.get(), connection);
            Pad pad = new Pad();
            try {
                handler.deviceAdded(pad);
                assertFalse(handler.prepareControllersForLaunch().isEmpty());
                assertFalse(ReflectionHelpers.callInstanceMethod(connection, "controllerMicrophoneOwnsRoute"));
                assertEquals(1, connection.arrivals);
            } finally { handler.destroy(); }
        }
    }

    @Test public void launchDoesNotWaitWithoutAWaveformReadyDualSense() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection(73);
            ControllerHandler handler = handler(activity.get(), connection);
            ExecutorService worker = Executors.newSingleThreadExecutor();
            try {
                // No USB DualSense is attached: nothing can become ready, so launch goes ahead at once.
                assertEquals("", worker.submit(handler::prepareControllersForLaunch).get(1, TimeUnit.SECONDS));
                // A handed-over DualSense whose waveform output could not start will not gain it either.
                usbManager(activity.get()).addOrUpdateUsbDevice(UsbPads.dualSense("/dev/bus/usb/002/001"), true);
                handler.deviceAdded(new Pad((short) MoonBridge.LI_CCAP_RUMBLE));
                assertFalse(worker.submit(handler::prepareControllersForLaunch).get(1, TimeUnit.SECONDS).isEmpty());
                assertEquals(1, connection.arrivals);
            } finally {
                worker.shutdownNow();
                handler.destroy();
            }
        }
    }

    @Test public void anAttachedDualSenseNotYetHandedOverMakesLaunchWait() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            usbManager(activity.get()).addOrUpdateUsbDevice(UsbPads.dualSense("/dev/bus/usb/002/002"), true);
            Connection connection = new Connection(73);
            ControllerHandler handler = handler(activity.get(), connection);
            ExecutorService worker = Executors.newSingleThreadExecutor();
            try {
                Future<String> launch = worker.submit(handler::prepareControllersForLaunch);
                try {
                    launch.get(300, TimeUnit.MILLISECONDS);
                    fail("Launch went ahead while the USB service was still starting the attached DualSense");
                } catch (TimeoutException expected) {
                }
                // The USB service hands the started pad over with its waveform output ready.
                handler.deviceAdded(new Pad());
                assertFalse(launch.get(2, TimeUnit.SECONDS).isEmpty());
            } finally {
                worker.shutdownNow();
                handler.destroy();
            }
        }
    }

    @Test public void aDualSenseAtTheUsbPermissionPromptDoesNotHoldTheLaunch() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            // Attached, but the person has not answered Android's USB permission prompt yet.
            usbManager(activity.get()).addOrUpdateUsbDevice(UsbPads.dualSense("/dev/bus/usb/002/003"), false);
            ControllerHandler handler = handler(activity.get(), new Connection(73));
            ExecutorService worker = Executors.newSingleThreadExecutor();
            try {
                assertEquals("", worker.submit(handler::prepareControllersForLaunch).get(1, TimeUnit.SECONDS));
            } finally {
                worker.shutdownNow();
                handler.destroy();
            }
        }
    }

    // Android 13 registers the USB service's unexported receiver without a manifest permission.
    @Config(sdk = 33)
    @Test public void aDualSenseWhoseDriverFailedToStartDoesNotHoldTheLaunch() throws Exception {
        // The test runs the main looper; the SHIELD accessory service is not on this device.
        org.robolectric.Shadows.shadowOf(org.robolectric.RuntimeEnvironment.getApplication()).declareComponentUnbindable(
                new android.content.ComponentName("com.nvidia.blakepairing", "com.nvidia.blakepairing.AccessoryService"));
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            android.preference.PreferenceManager.getDefaultSharedPreferences(activity.get()).edit()
                    .putBoolean("checkbox_usb_dualsense", true).commit();
            UsbPads.Connection.reset();
            // Android's own driver keeps the HID interface: the USB service cannot claim it.
            UsbPads.Connection.failClaims();
            usbManager(activity.get()).addOrUpdateUsbDevice(UsbPads.dualSense("/dev/bus/usb/002/004"), true);
            ControllerHandler handler = handler(activity.get(), new Connection(73));
            ServiceController<UsbDriverService> service = Robolectric.buildService(UsbDriverService.class).create();
            ExecutorService worker = Executors.newSingleThreadExecutor();
            try {
                UsbDriverService.UsbDriverBinder binder = (UsbDriverService.UsbDriverBinder) service.get().onBind(new Intent());
                binder.setListener(handler);
                binder.start();
                Future<String> launch = worker.submit(handler::prepareControllersForLaunch);
                long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2);
                while (!launch.isDone() && System.nanoTime() < deadline) {
                    // The service reports the failed start on the main thread.
                    ShadowLooper.idleMainLooper();
                    Thread.sleep(10);
                }
                assertTrue("Launch kept waiting for a DualSense whose driver failed", launch.isDone());
                assertEquals("", launch.get());
                assertEquals(1, UsbPads.Connection.closes.get());
            } finally {
                worker.shutdownNow();
                service.destroy();
                handler.destroy();
            }
        }
    }

    private static ShadowUsbManager usbManager(Activity activity) {
        return org.robolectric.Shadows.shadowOf((UsbManager) activity.getSystemService(Context.USB_SERVICE));
    }

    @Test public void inputWaitsWhileLaunchPreparationAnnouncesAPad() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            CountDownLatch announcing = new CountDownLatch(1), release = new CountDownLatch(1);
            AtomicInteger inProgress = new AtomicInteger();
            AtomicBoolean overlapped = new AtomicBoolean();
            List<Byte> slots = Collections.synchronizedList(new ArrayList<>());
            Connection connection = new Connection(73) {
                @Override public int sendControllerArrivalEvent(byte slot, short mask, byte type, int buttons, short caps) {
                    if (inProgress.incrementAndGet() > 1) overlapped.set(true);
                    slots.add(slot);
                    if ((caps & MoonBridge.LI_CCAP_HAPTICS_PCM) != 0) {
                        // The connection worker is announcing the waveform pad before launch.
                        announcing.countDown();
                        try { release.await(2, TimeUnit.SECONDS); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
                    }
                    inProgress.decrementAndGet();
                    return super.sendControllerArrivalEvent(slot, mask, type, buttons, caps);
                }
            };
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.usbDualSenseHaptics = true;
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            handler.deviceAdded(new Pad());
            handler.deviceAdded(new Pad(43, MoonBridge.LI_CCAP_RUMBLE));
            ExecutorService threads = Executors.newFixedThreadPool(2);
            try {
                Future<String> launch = threads.submit(handler::prepareControllersForLaunch);
                assertTrue(announcing.await(2, TimeUnit.SECONDS));
                // The second pad's first input arrives meanwhile and needs a slot of its own.
                Future<?> input = threads.submit(() -> handler.reportControllerState(43, ControllerPacket.A_FLAG,
                        0, 0, 0, 0, 0, 0));
                try { input.get(300, TimeUnit.MILLISECONDS); } catch (TimeoutException expected) { }
                assertFalse("Slot assignment must not run on two threads at once", overlapped.get());
                release.countDown();
                assertFalse(launch.get(2, TimeUnit.SECONDS).isEmpty());
                input.get(2, TimeUnit.SECONDS);
                assertEquals(Arrays.asList((byte) 0, (byte) 1), slots);
            } finally {
                release.countDown();
                threads.shutdownNow();
                handler.destroy();
            }
        }
    }

    @Test public void inputDuringAnotherPadsSlotAssignmentSeesTheFinishedMask() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            CountDownLatch announcing = new CountDownLatch(1), release = new CountDownLatch(1);
            AtomicBoolean assigning = new AtomicBoolean();
            List<String> inputsDuringAssignment = Collections.synchronizedList(new ArrayList<>());
            Connection connection = new Connection(73) {
                @Override public int sendControllerArrivalEvent(byte slot, short mask, byte type, int buttons, short caps) {
                    if ((caps & MoonBridge.LI_CCAP_HAPTICS_PCM) != 0) {
                        // Launch preparation is still inside the waveform pad's slot assignment.
                        assigning.set(true);
                        announcing.countDown();
                        try { release.await(2, TimeUnit.SECONDS); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
                        assigning.set(false);
                    }
                    return super.sendControllerArrivalEvent(slot, mask, type, buttons, caps);
                }
                @Override public void sendControllerInput(short slot, short mask, int buttons, byte leftTrigger,
                                                         byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
                    if (assigning.get()) inputsDuringAssignment.add("slot " + slot + ", mask " + mask);
                    super.sendControllerInput(slot, mask, buttons, leftTrigger, rightTrigger, leftX, leftY, rightX, rightY);
                }
            };
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.usbDualSenseHaptics = true;
            prefs.multiController = true;
            ControllerHandler handler = new ControllerHandler(activity.get(), connection, null, prefs);
            handler.deviceAdded(new Pad(43, MoonBridge.LI_CCAP_RUMBLE));
            handler.reportControllerState(43, ControllerPacket.A_FLAG, 0, 0, 0, 0, 0, 0);
            assertEquals(1, connection.inputs);
            handler.deviceAdded(new Pad());
            ExecutorService threads = Executors.newFixedThreadPool(2);
            try {
                Future<String> launch = threads.submit(handler::prepareControllersForLaunch);
                assertTrue(announcing.await(2, TimeUnit.SECONDS));
                // The pad that already has slot 0 keeps sending input on the main thread meanwhile.
                Future<?> input = threads.submit(() -> handler.reportControllerState(43, 0, 0, 0, 0, 0, 0, 0));
                try { input.get(300, TimeUnit.MILLISECONDS); } catch (TimeoutException expected) { }
                assertEquals("The active-pad mask must not be read mid-assignment", List.of(), inputsDuringAssignment);
                release.countDown();
                assertFalse(launch.get(2, TimeUnit.SECONDS).isEmpty());
                input.get(2, TimeUnit.SECONDS);
                assertEquals(2, connection.inputs);
            } finally {
                release.countDown();
                threads.shutdownNow();
                handler.destroy();
            }
        }
    }

    @Test public void monitorAndRevokedInputCannotPrepareBindOrRouteControllerAudio() {
        for (int appId : new int[] {2147483505, 73}) {
            checkDeniedControllerAccess(appId, appId == 73 ? 0 : -1);
        }
    }

    @Test public void keyboardOnlyAccessCannotPrepareAnnounceBindOrSendControllerInput() {
        checkDeniedControllerAccess(73, 1 << 12);
    }

    private void checkDeniedControllerAccess(int appId, int permission) {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection(appId);
            ComputerDetails details = new ComputerDetails(); details.permission = permission;
            ReflectionHelpers.setField(connection, "hostDetails", details);
            assertFalse(connection.canSendControllerInput());
            if (permission == (1 << 12)) assertTrue(connection.canSendInput());
            ControllerHandler handler = handler(activity.get(), connection);
            Pad pad = new Pad();
            try {
                handler.deviceAdded(pad);
                assertEquals("", handler.prepareControllersForLaunch());
                handler.resendControllerArrivals();
                handler.reportControllerState(42, 0, 0, 0, 0, 0, 0, 0);
                ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
                context.device = pad; context.assignedControllerNumber = true;
                context.sendControllerArrival();
                handler.handleControllerAudio(0x5535, new byte[] {1, 3, 0, 0, 3, 0, 0, 0});
                assertEquals(0, connection.arrivals);
                assertEquals(0, connection.inputs);
                assertEquals(0, pad.audioBindings);
                assertEquals(0, pad.audioPackets);
                context.destroy();
            } finally { handler.destroy(); }
        }
    }

    @Test public void keyboardOnlyAccessCannotUseAnAlreadyBoundControllerAudioRoute() {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            Connection connection = new Connection(73);
            ControllerHandler handler = handler(activity.get(), connection);
            Pad pad = new Pad();
            ControllerHandler.UsbDeviceContext context = handler.new UsbDeviceContext();
            context.device = pad; context.assignedControllerNumber = true;
            try {
                assertFalse("No DualSense audio yet: the renderer skips its volume lookups",
                        handler.headphoneAudioSink().active());
                context.sendControllerArrival();
                assertTrue(handler.headphoneAudioSink().active());
                byte[] control = {1, 3, 0, 0, 3, 0, 0, 0};
                handler.handleControllerAudio(0x5535, control);
                handler.handleControllerHaptics((short) 0, 0, new byte[4]);
                assertTrue(handler.routeHeadphoneAudio(new short[2], 2, 1));
                assertEquals(1, pad.audioPackets);
                assertEquals(1, pad.headphonePackets);
                assertEquals(1, pad.waveforms);

                ComputerDetails keyboardOnly = new ComputerDetails(); keyboardOnly.permission = 1 << 12;
                ReflectionHelpers.setField(connection, "hostDetails", keyboardOnly);
                context.sendControllerArrival();
                handler.handleControllerAudio(0x5535, control);
                handler.handleControllerHaptics((short) 0, 1, new byte[4]);
                assertFalse(handler.routeHeadphoneAudio(new short[2], 2, 1));
                assertFalse(handler.headphoneAudioSink().active());
                assertEquals(1, connection.arrivals);
                assertEquals(1, pad.audioBindings);
                assertEquals(1, pad.audioPackets);
                assertEquals(1, pad.headphonePackets);
                assertEquals(1, pad.waveforms);
            } finally { context.destroy(); handler.destroy(); }
        }
    }

    private static ControllerHandler handler(Activity activity, Connection connection) {
        PreferenceConfiguration prefs = new PreferenceConfiguration(); prefs.usbDualSenseHaptics = true;
        return new ControllerHandler(activity, connection, null, prefs);
    }
    private static class Connection extends NvConnection {
        int arrivals, inputs;
        short activeMask;
        byte arrivalType;
        Connection(int appId) {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().setApp(new NvApp("Test", appId, false)).build(), null, null);
        }
        @Override public int sendControllerArrivalEvent(byte slot, short mask, byte type, int buttons, short caps) {
            arrivals++; arrivalType = type; return 0;
        }
        @Override public void sendControllerInput(short slot, short mask, int buttons, byte leftTrigger,
                                                 byte rightTrigger, short leftX, short leftY, short rightX, short rightY) {
            inputs++; activeMask = mask;
        }
    }
    private static class Pad extends AbstractController {
        int audioBindings, audioPackets, headphonePackets, waveforms;
        Pad() { this(MoonBridge.LI_CCAP_HAPTICS_PCM); }
        Pad(short caps) { this(42, caps); }
        Pad(int id, short caps) { super(id, null, 0x054c, 0x0ce6); capabilities = caps; }
        @Override public void bindAudio(NvConnection connection, int number) { audioBindings++; }
        @Override public void controllerAudio(int kind, byte[] payload) { audioPackets++; }
        @Override public void controllerHaptics(int sequence, byte[] pcm) { waveforms++; }
        @Override public boolean headphoneAudio(short[] samples, int length, float volume) { headphonePackets++; return true; }
        @Override public boolean start() { return true; }
        @Override public void stop() { }
        @Override public void rumble(short low, short high) { }
        @Override public void rumbleTriggers(short left, short right) { }
    }
}
