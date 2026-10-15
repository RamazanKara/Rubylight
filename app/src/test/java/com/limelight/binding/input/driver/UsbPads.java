package com.limelight.binding.input.driver;

import android.hardware.usb.UsbConfiguration;
import android.hardware.usb.UsbConstants;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbEndpoint;
import android.hardware.usb.UsbInterface;
import android.os.Parcelable;

import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.util.ReflectionHelpers;

import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

import static org.robolectric.util.ReflectionHelpers.ClassParameter.from;

// USB pads as Android enumerates them, and a connection whose transfers behave like a real pad's.
public final class UsbPads {
    private UsbPads() {}

    public static UsbDevice dualSense(String name) {
        return device(name, 0x054C, 0x0CE6, usbInterface(0, UsbConstants.USB_CLASS_HID, 0, 0,
                endpoint(0x84, 64), endpoint(0x03, 64)));
    }

    public static UsbDevice switchPro(String name) {
        return device(name, 0x057E, 0x2009, usbInterface(0, UsbConstants.USB_CLASS_HID, 0, 0,
                endpoint(0x81, 64), endpoint(0x01, 64)));
    }

    public static UsbDevice xboxOne(String name) {
        return device(name, 0x045E, 0x02EA, usbInterface(0, UsbConstants.USB_CLASS_VENDOR_SPEC, 71, 208,
                endpoint(0x81, 64), endpoint(0x01, 64)));
    }

    // An input report from a DualSense at rest.
    static byte[] dualSenseReport() {
        byte[] report = new byte[DualSenseReport.INPUT_SIZE];
        report[0] = 0x01;
        report[1] = report[2] = report[3] = report[4] = (byte) 0x80;
        return report;
    }

    private static UsbEndpoint endpoint(int address, int maxPacketSize) {
        return ReflectionHelpers.callConstructor(UsbEndpoint.class, from(int.class, address),
                from(int.class, UsbConstants.USB_ENDPOINT_XFER_INT), from(int.class, maxPacketSize), from(int.class, 1));
    }

    private static UsbInterface usbInterface(int id, int usbClass, int subclass, int protocol, UsbEndpoint... endpoints) {
        UsbInterface usbInterface = ReflectionHelpers.callConstructor(UsbInterface.class, from(int.class, id),
                from(int.class, 0), from(String.class, "pad"), from(int.class, usbClass),
                from(int.class, subclass), from(int.class, protocol));
        ReflectionHelpers.callInstanceMethod(usbInterface, "setEndpoints", from(Parcelable[].class, endpoints));
        return usbInterface;
    }

    private static UsbDevice device(String name, int vendorId, int productId, UsbInterface... interfaces) {
        UsbDevice device = Shadow.newInstanceOf(UsbDevice.class);
        ReflectionHelpers.setField(device, "mName", name);
        ReflectionHelpers.setField(device, "mVendorId", vendorId);
        ReflectionHelpers.setField(device, "mProductId", productId);
        ReflectionHelpers.setField(device, "mConfigurations", new UsbConfiguration[0]);
        ReflectionHelpers.setField(device, "mInterfaces", interfaces);
        return device;
    }

    // What a USB driver tells its listener, in order.
    static final class Recorder implements UsbDriverListener {
        final java.util.List<String> events = java.util.Collections.synchronizedList(new java.util.ArrayList<>());

        boolean has(String event) {
            return events.contains(event);
        }

        int count(String event) {
            synchronized (events) {
                int count = 0;
                for (String e : events) if (e.equals(event)) count++;
                return count;
            }
        }

        @Override public void reportControllerState(int controllerId, int buttonFlags, float leftStickX, float leftStickY,
                                                    float rightStickX, float rightStickY, float leftTrigger, float rightTrigger) {
            events.add("state");
        }
        @Override public void reportControllerTouch(int controllerId, byte eventType, int pointerId, float x, float y, float pressure) {
            events.add("touch");
        }
        @Override public void reportControllerMotion(int controllerId, byte motionType, float x, float y, float z) {
            events.add("motion");
        }
        @Override public void reportControllerBattery(int controllerId, byte state, byte percentage) {
            events.add("battery");
        }
        @Override public void deviceRemoved(AbstractController controller) {
            events.add("removed");
        }
        @Override public void deviceAdded(AbstractController controller) {
            events.add("added");
        }
        public void deviceStartFailed(int usbDeviceId) {
            events.add("start failed");
        }
    }

    // Waits for something another thread does, letting the main looper deliver what it posts.
    static void await(java.util.function.BooleanSupplier condition) throws InterruptedException {
        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
        while (!condition.getAsBoolean()) {
            if (System.nanoTime() > deadline) throw new AssertionError("Timed out waiting");
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            Thread.sleep(5);
        }
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
    }

    // Transfers like a real pad's: they block the calling thread, ignore interrupts and end at their
    // own time. Settings apply to every connection opened while a test runs; reset() between tests.
    @Implements(UsbDeviceConnection.class)
    public static class Connection {
        // Control transfers (feature reads) wait for this gate, up to their timeout, then fail.
        static volatile CountDownLatch controlGate;
        static volatile boolean claimSucceeds;
        static volatile long inputReadMs;
        // Output transfers take this long; each is recorded with the thread that sent it.
        static volatile long outputWriteMs;
        static final java.util.List<byte[]> outputs = new java.util.concurrent.CopyOnWriteArrayList<>();
        static final java.util.List<String> outputThreads = new java.util.concurrent.CopyOnWriteArrayList<>();
        static volatile byte[] inputReport;
        public static final AtomicInteger claims = new AtomicInteger(), releases = new AtomicInteger(), closes = new AtomicInteger();
        static final AtomicBoolean readInProgress = new AtomicBoolean();
        static volatile boolean closedDuringRead;
        static final Set<String> controlThreads = ConcurrentHashMap.newKeySet();

        public static void reset() {
            controlGate = null;
            claimSucceeds = true;
            inputReadMs = 4;
            outputWriteMs = 0;
            outputs.clear();
            outputThreads.clear();
            inputReport = null;
            claims.set(0);
            releases.set(0);
            closes.set(0);
            readInProgress.set(false);
            closedDuringRead = false;
            controlThreads.clear();
        }

        public static void failClaims() {
            claimSucceeds = false;
        }

        @Implementation
        protected boolean claimInterface(UsbInterface usbInterface, boolean force) {
            controlThreads.add(Thread.currentThread().getName());
            claims.incrementAndGet();
            return claimSucceeds;
        }

        @Implementation
        protected boolean releaseInterface(UsbInterface usbInterface) {
            releases.incrementAndGet();
            return true;
        }

        @Implementation
        protected int controlTransfer(int requestType, int request, int value, int index, byte[] buffer, int length, int timeout) {
            controlThreads.add(Thread.currentThread().getName());
            CountDownLatch gate = controlGate;
            if (gate != null) {
                try {
                    if (!gate.await(timeout, TimeUnit.MILLISECONDS)) return -1;
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return -1;
                }
            }
            return length;
        }

        @Implementation
        protected int bulkTransfer(UsbEndpoint endpoint, byte[] buffer, int offset, int length, int timeout) {
            if (closes.get() > 0) return -1;
            boolean out = endpoint.getDirection() == UsbConstants.USB_DIR_OUT;
            if (out) {
                outputs.add(java.util.Arrays.copyOfRange(buffer, offset, offset + length));
                outputThreads.add(Thread.currentThread().getName());
                if (outputWriteMs == 0) return length;
            } else {
                readInProgress.set(true);
            }
            boolean interrupted = false;
            try {
                long end = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(out ? outputWriteMs : inputReadMs);
                for (long left; (left = end - System.nanoTime()) > 0; ) {
                    try {
                        TimeUnit.NANOSECONDS.sleep(left);
                    } catch (InterruptedException e) {
                        // USB reads end at their own timeout, not on an interrupt.
                        interrupted = true;
                    }
                }
            } finally {
                if (!out) readInProgress.set(false);
                if (interrupted) Thread.currentThread().interrupt();
            }
            if (out) return length;
            byte[] report = inputReport;
            if (report == null) return -1;
            int size = Math.min(report.length, length);
            System.arraycopy(report, 0, buffer, offset, size);
            return size;
        }

        @Implementation
        protected int bulkTransfer(UsbEndpoint endpoint, byte[] buffer, int length, int timeout) {
            return bulkTransfer(endpoint, buffer, 0, length, timeout);
        }

        @Implementation
        protected void close() {
            if (readInProgress.get()) closedDuringRead = true;
            closes.incrementAndGet();
        }
    }
}
