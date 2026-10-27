package com.limelight.binding.usb;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.hardware.usb.UsbConfiguration;
import android.hardware.usb.UsbConstants;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbManager;
import android.os.Looper;

import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.json.JSONException;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.shadows.ShadowUsbManager;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.AbstractExecutorService;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 33, application = Application.class)
public class UsbForwardingTest {
    private Context context;
    private UsbManager usbManager;
    private final PreferenceConfiguration prefs = new PreferenceConfiguration();
    private final FakeBridge bridge = new FakeBridge();
    private final Worker worker = new Worker();
    private final List<FakeConnection> connections = new ArrayList<>();
    private final List<String> messages = new ArrayList<>();
    private UsbForwarding forwarding;

    // The engine as far as Java sees it: usbAttach's result and the list it then reports.
    private static final class FakeBridge implements UsbForwarding.Bridge {
        int result = 7;
        String list = "[]";
        Runnable listener;
        final List<Integer> detached = new ArrayList<>();
        final List<String> attached = new ArrayList<>();

        @Override public int attach(int fd, String path, String name) {
            attached.add(fd + " " + path + " " + name);
            return result;
        }
        @Override public boolean detach(int requestId) {
            detached.add(requestId);
            return true;
        }
        @Override public String list() {
            return list;
        }
        @Override public void setListener(Runnable listener) {
            this.listener = listener;
        }

        void changed(String list) {
            this.list = list;
            listener.run();
            shadowOf(Looper.getMainLooper()).idle();
        }
    }

    private static final class FakeConnection implements UsbForwarding.Connection {
        final int fd;
        int closes;

        FakeConnection(int fd) {
            this.fd = fd;
        }
        @Override public int fd() {
            if (closes > 0) throw new AssertionError("fd of a closed connection");
            return fd;
        }
        @Override public void close() {
            closes++;
        }
    }

    // Runs attaches when the test says so, as the attach thread would.
    private static final class Worker extends AbstractExecutorService {
        final List<Runnable> queued = new ArrayList<>();
        boolean shutdown;

        void runAll() {
            for (Runnable task : new ArrayList<>(queued)) task.run();
            queued.clear();
            shadowOf(Looper.getMainLooper()).idle();
        }
        @Override public void execute(Runnable task) {
            if (shutdown) throw new java.util.concurrent.RejectedExecutionException();
            queued.add(task);
        }
        @Override public void shutdown() {
            shutdown = true;
        }
        @Override public List<Runnable> shutdownNow() {
            shutdown = true;
            return Collections.emptyList();
        }
        @Override public boolean isShutdown() {
            return shutdown;
        }
        @Override public boolean isTerminated() {
            return shutdown;
        }
        @Override public boolean awaitTermination(long timeout, TimeUnit unit) {
            return true;
        }
    }

    @Before
    public void setUp() {
        context = org.robolectric.RuntimeEnvironment.getApplication();
        usbManager = (UsbManager) context.getSystemService(Context.USB_SERVICE);
        forwarding = new UsbForwarding(context, usbManager, prefs, bridge, device -> {
            FakeConnection connection = new FakeConnection(40 + connections.size());
            connections.add(connection);
            return connection;
        }, worker, text -> messages.add(text.toString()));
    }

    private ShadowUsbManager shadowUsb() {
        return shadowOf(usbManager);
    }

    private static UsbDevice device(String path, String product, int deviceClass, int vendorId, int productId,
                                    android.hardware.usb.UsbInterface... interfaces) {
        UsbDevice device = Shadow.newInstanceOf(UsbDevice.class);
        ReflectionHelpers.setField(device, "mName", path);
        ReflectionHelpers.setField(device, "mProductName", product);
        ReflectionHelpers.setField(device, "mClass", deviceClass);
        ReflectionHelpers.setField(device, "mVendorId", vendorId);
        ReflectionHelpers.setField(device, "mProductId", productId);
        ReflectionHelpers.setField(device, "mConfigurations", new UsbConfiguration[0]);
        ReflectionHelpers.setField(device, "mInterfaces", interfaces);
        return device;
    }

    private static UsbDevice tablet() {
        return device("/dev/bus/usb/001/004", "Drawing Tablet", 0, 0x056A, 0x0374);
    }

    private static String entry(int requestId, String phase, String ended) {
        return "{\"requestId\":" + requestId + ",\"busid\":\"1-4\",\"name\":\"Drawing Tablet\",\"vid\":1366,\"pid\":884,"
                + "\"phase\":\"" + phase + "\"" + (ended == null ? "" : "," + ended) + "}";
    }

    private String text(int id, Object... args) {
        return context.getString(id, args);
    }

    // Picked with permission, attached, and the engine's list says it is on the PC.
    private UsbDevice attachTablet() {
        UsbDevice tablet = tablet();
        shadowUsb().addOrUpdateUsbDevice(tablet, true);
        forwarding.choose(tablet);
        worker.runAll();
        bridge.changed("[" + entry(7, "attached", null) + "]");
        return tablet;
    }

    @Test
    public void theListIsReadAsTheEngineWritesIt() throws Exception {
        List<UsbForwarding.Attachment> list = UsbForwarding.Attachment.parseList("[" + entry(3, "opening", null) + ","
                + entry(4, "ended", "\"state\":\"failed\",\"reason\":\"device_error\",\"error\":\"tunnel closed\"") + "]");
        assertEquals(2, list.size());
        assertEquals(3, list.get(0).requestId);
        assertFalse(list.get(0).ended());
        assertNull(list.get(0).state);
        assertTrue(list.get(1).ended());
        assertEquals("failed", list.get(1).state);
        assertEquals("device_error", list.get(1).reason);
        assertEquals("tunnel closed", list.get(1).error);
        try {
            UsbForwarding.Attachment.parseList("[{\"phase\":\"attached\"}]");
            fail("an entry without its request id");
        } catch (JSONException expected) {
        }
    }

    @Test
    public void thePickerLeavesOutHubsAndWhatTheInputDriversTake() {
        prefs.usbDualSense = true;
        shadowUsb().addOrUpdateUsbDevice(device("/dev/bus/usb/001/002", "Hub", UsbConstants.USB_CLASS_HUB, 0x05E3, 0x0610), true);
        shadowUsb().addOrUpdateUsbDevice(com.limelight.binding.input.driver.UsbPads.dualSense("/dev/bus/usb/001/003"), true);
        shadowUsb().addOrUpdateUsbDevice(tablet(), true);
        List<UsbDevice> candidates = forwarding.candidates();
        assertEquals(1, candidates.size());
        assertEquals("Drawing Tablet", UsbForwarding.name(candidates.get(0)));
        // Without its own driver the DualSense may go to the PC like anything else.
        prefs.usbDualSense = false;
        assertEquals(2, forwarding.candidates().size());
    }

    @Test
    public void aDeviceWithoutAProductNameIsCalledByItsIds() {
        assertEquals("USB 056a:0374", UsbForwarding.name(device("/dev/bus/usb/001/004", null, 0, 0x056A, 0x0374)));
    }

    @Test
    public void attachingSaysEachStepAndTheHostsRefusal() {
        UsbDevice tablet = tablet();
        shadowUsb().addOrUpdateUsbDevice(tablet, true);
        forwarding.choose(tablet);
        assertEquals(Collections.singletonList(text(R.string.usb_forwarding_asking, "Drawing Tablet")), messages);
        assertTrue("attaching waits for the attach thread", bridge.attached.isEmpty());
        worker.runAll();
        assertEquals(Collections.singletonList("40 /dev/bus/usb/001/004 Drawing Tablet"), bridge.attached);
        bridge.changed("[" + entry(7, "opening", null) + "]");
        assertEquals(1, messages.size());
        bridge.changed("[" + entry(7, "attached", null) + "]");
        assertEquals(text(R.string.usb_forwarding_attached, "Drawing Tablet"), messages.get(1));
        assertTrue(forwarding.isForwarded(tablet));
        assertEquals(text(R.string.usb_forwarding_detach, "Drawing Tablet"), forwarding.label(tablet).toString());
        // The same list again says nothing new.
        bridge.changed(bridge.list);
        assertEquals(2, messages.size());
        bridge.changed("[" + entry(7, "ended", "\"state\":\"refused\",\"reason\":\"limit\"") + "]");
        assertEquals(text(R.string.usb_forwarding_refused, "Drawing Tablet", text(R.string.usb_forwarding_reason_limit)),
                messages.get(2));
        assertEquals(1, connections.get(0).closes);
        assertFalse(forwarding.isForwarded(tablet));
    }

    @Test
    public void aFailureHereSaysWhatFailed() {
        attachTablet();
        bridge.changed("[" + entry(7, "ended", "\"state\":\"failed\",\"reason\":\"none\",\"error\":\"the tunnel closed\"") + "]");
        assertEquals(text(R.string.usb_forwarding_failed, "Drawing Tablet", "the tunnel closed"), messages.get(messages.size() - 1));
        assertEquals(1, connections.get(0).closes);
    }

    @Test
    public void anAttachTheEngineRefusesSaysWhyAndCloses() {
        bridge.result = MoonBridge.USB_UNSUPPORTED;
        UsbDevice tablet = tablet();
        shadowUsb().addOrUpdateUsbDevice(tablet, true);
        forwarding.choose(tablet);
        worker.runAll();
        assertEquals(text(R.string.usb_forwarding_failed, "Drawing Tablet", text(R.string.usb_forwarding_code_unsupported)),
                messages.get(1));
        assertEquals(1, connections.get(0).closes);
        assertFalse(forwarding.isForwarded(tablet));
    }

    @Test
    public void everyAttachCodeAndHostReasonHasItsOwnText() {
        int[] codes = {MoonBridge.USB_NO_STREAM, MoonBridge.USB_BUSY, MoonBridge.USB_UNSUPPORTED,
                MoonBridge.USB_DEVICE_FAILED, MoonBridge.USB_INVALID, MoonBridge.USB_HOST_FAILED};
        java.util.Set<Integer> texts = new java.util.HashSet<>();
        for (int code : codes) texts.add(UsbForwarding.codeReason(code));
        assertEquals(codes.length, texts.size());
        assertFalse(texts.contains(R.string.usb_forwarding_reason_unknown));
        String[] reasons = {"permission", "disabled", "limit", "driver", "tunnel_timeout", "device_error", "session_end"};
        texts.clear();
        for (String reason : reasons) texts.add(UsbForwarding.reasonString(reason));
        assertEquals(reasons.length, texts.size());
        assertFalse(texts.contains(R.string.usb_forwarding_reason_unknown));
        assertEquals(0, UsbForwarding.reasonString("none"));
        assertEquals(R.string.usb_forwarding_reason_unknown, UsbForwarding.reasonString("later"));
    }

    @Test
    public void choosingAnAttachedDeviceDetachesIt() {
        UsbDevice tablet = attachTablet();
        forwarding.choose(tablet);
        assertEquals(Collections.singletonList(7), bridge.detached);
        assertEquals("closed once the engine says it is detached", 0, connections.get(0).closes);
        bridge.changed("[" + entry(7, "ended", "\"state\":\"detached\",\"reason\":\"none\"") + "]");
        assertEquals(text(R.string.usb_forwarding_detached, "Drawing Tablet"), messages.get(messages.size() - 1));
        assertEquals(1, connections.get(0).closes);
    }

    @Test
    public void unpluggingDetachesItAndSaysSo() {
        UsbDevice tablet = attachTablet();
        context.sendBroadcast(new Intent(UsbManager.ACTION_USB_DEVICE_DETACHED).setPackage(context.getPackageName())
                .putExtra(UsbManager.EXTRA_DEVICE, tablet));
        shadowOf(Looper.getMainLooper()).idle();
        assertEquals(Collections.singletonList(7), bridge.detached);
        assertEquals(1, connections.get(0).closes);
        assertEquals(text(R.string.usb_forwarding_unplugged, "Drawing Tablet"), messages.get(messages.size() - 1));
        // The engine's own detached entry afterwards is not said twice.
        int said = messages.size();
        bridge.changed("[" + entry(7, "ended", "\"state\":\"detached\",\"reason\":\"none\"") + "]");
        assertEquals(said, messages.size());
    }

    @Test
    public void stoppingDetachesEverythingAndStopsListening() {
        attachTablet();
        forwarding.stop();
        assertEquals(Collections.singletonList(7), bridge.detached);
        assertEquals(1, connections.get(0).closes);
        assertNull(bridge.listener);
        assertTrue(worker.shutdown);
    }

    @Test
    public void aDeviceStoppedWhileAttachingIsClosedOnlyOnceTheEngineHasIt() {
        UsbDevice tablet = tablet();
        shadowUsb().addOrUpdateUsbDevice(tablet, true);
        forwarding.choose(tablet);
        forwarding.stop();
        // The engine copies the descriptor in usbAttach: closing it before would hand it a dead or reused one.
        assertEquals(0, connections.get(0).closes);
        worker.runAll();
        assertEquals(Collections.singletonList("40 /dev/bus/usb/001/004 Drawing Tablet"), bridge.attached);
        assertEquals(Collections.singletonList(7), bridge.detached);
        assertEquals(1, connections.get(0).closes);
    }

    @Test
    public void aDeniedPermissionSaysSo() {
        UsbDevice tablet = tablet();
        shadowUsb().addOrUpdateUsbDevice(tablet, false);
        forwarding.choose(tablet);
        context.sendBroadcast(new Intent(UsbForwarding.ACTION_PERMISSION).setPackage(context.getPackageName())
                .putExtra(UsbManager.EXTRA_DEVICE, tablet).putExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false));
        shadowOf(Looper.getMainLooper()).idle();
        assertEquals(Collections.singletonList(text(R.string.usb_forwarding_permission_denied, "Drawing Tablet")), messages);
        assertTrue(connections.isEmpty());
    }
}
