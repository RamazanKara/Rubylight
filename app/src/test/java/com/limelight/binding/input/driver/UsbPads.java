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

    static UsbDevice gameSir(String name, int vendorId, boolean xboxOne) {
        return device(name, vendorId, 1, usbInterface(0, UsbConstants.USB_CLASS_VENDOR_SPEC,
                xboxOne ? 71 : 93, xboxOne ? 208 : 1, endpoint(0x81, 64), endpoint(0x01, 64)));
    }

    static UsbDevice gameSirAndroid(String name) {
        return device(name, 0x3537, 1, usbInterface(0, UsbConstants.USB_CLASS_HID, 0, 0,
                endpoint(0x81, 64)));
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
            if (System.nanoTime() > deadline) {
                throw new AssertionError("Timed out waiting: closes " + Connection.closes.get()
                        + ", claims " + Connection.claims.get() + ", releases " + Connection.releases.get()
                        + ", reads " + Connection.readsStarted.get() + ", read held " + Connection.readHeld.get());
            }
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            Thread.sleep(5);
        }
        org.robolectric.shadows.ShadowLooper.idleMainLooper();
    }

    // Makes input reads slow and waits until one that started afterwards is in progress, so the test
    // can act while the pad is blocked in a USB read.
    static void awaitSlowRead(long readMs) throws InterruptedException {
        Connection.inputReadMs = readMs;
        int started = Connection.readsStarted.get();
        await(() -> Connection.readsStarted.get() > started && Connection.readInProgress.get());
    }

    // Holds input reads until releaseReads() and waits until one is held. The read in flight when
    // the gate closes is held too, so this waits for the hold itself, not for a later read. While
    // a read is held, the pad cannot finish it, so a stop() that returns proves it did not wait for
    // the read, however slowly the test machine runs.
    static void holdReads() throws InterruptedException {
        Connection.readGate = new CountDownLatch(1);
        await(Connection.readHeld::get);
    }

    static void releaseReads() {
        CountDownLatch gate = Connection.readGate;
        Connection.readGate = null;
        if (gate != null) gate.countDown();
    }

    // Transfers like a real pad's: they block the calling thread, ignore interrupts and end at their
    // own time. Settings apply to every connection opened while a test runs; reset() between tests.
    // A connection opened before the last reset() is stale: a pad an earlier test left releasing on
    // its USB worker fails its transfers and touches none of the counters of the test now running.
    @Implements(UsbDeviceConnection.class)
    public static class Connection {
        private static final AtomicInteger generation = new AtomicInteger();
        private final int openedIn = generation.get();
        private volatile boolean closed;
        // Control transfers (feature reads) wait for this gate, up to their timeout, then fail.
        static volatile CountDownLatch controlGate;
        static volatile boolean claimSucceeds;
        static volatile long inputReadMs;
        // Input reads wait for this gate too, ignoring interrupts, up to HELD_READ_MS.
        static volatile CountDownLatch readGate;
        static final long HELD_READ_MS = 10_000;
        static final AtomicBoolean readHeld = new AtomicBoolean();
        // Output transfers take this long; each is recorded with the thread that sent it.
        static volatile long outputWriteMs;
        static final java.util.List<byte[]> outputs = new java.util.concurrent.CopyOnWriteArrayList<>();
        static final java.util.List<String> outputThreads = new java.util.concurrent.CopyOnWriteArrayList<>();
        static volatile byte[] inputReport;
        public static final AtomicInteger claims = new AtomicInteger(), releases = new AtomicInteger(), closes = new AtomicInteger();
        static final AtomicBoolean readInProgress = new AtomicBoolean();
        static final AtomicInteger readsStarted = new AtomicInteger();
        static volatile boolean closedDuringRead;
        static final Set<String> controlThreads = ConcurrentHashMap.newKeySet();

        public static void reset() {
            generation.incrementAndGet();
            controlGate = null;
            CountDownLatch held = readGate;
            readGate = null;
            if (held != null) held.countDown();
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
            readHeld.set(false);
            readsStarted.set(0);
            closedDuringRead = false;
            controlThreads.clear();
        }

        public static void failClaims() {
            claimSucceeds = false;
        }

        private boolean stale() {
            return openedIn != generation.get();
        }

        @Implementation
        protected boolean claimInterface(UsbInterface usbInterface, boolean force) {
            if (stale()) return false;
            controlThreads.add(Thread.currentThread().getName());
            claims.incrementAndGet();
            return claimSucceeds;
        }

        @Implementation
        protected boolean releaseInterface(UsbInterface usbInterface) {
            if (stale()) return true;
            releases.incrementAndGet();
            return true;
        }

        @Implementation
        protected int controlTransfer(int requestType, int request, int value, int index, byte[] buffer, int length, int timeout) {
            if (stale()) return -1;
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
            if (closed || stale()) return -1;
            boolean out = endpoint.getDirection() == UsbConstants.USB_DIR_OUT;
            if (out) {
                outputs.add(java.util.Arrays.copyOfRange(buffer, offset, offset + length));
                outputThreads.add(Thread.currentThread().getName());
                if (outputWriteMs == 0) return length;
            } else {
                readsStarted.incrementAndGet();
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
                CountDownLatch gate = out ? null : readGate;
                long heldUntil = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(HELD_READ_MS);
                while (gate != null && gate.getCount() > 0 && System.nanoTime() < heldUntil) {
                    readHeld.set(true);
                    try {
                        gate.await(heldUntil - System.nanoTime(), TimeUnit.NANOSECONDS);
                    } catch (InterruptedException e) {
                        interrupted = true;
                    }
                }
            } finally {
                if (!out && !stale()) {
                    readHeld.set(false);
                    readInProgress.set(false);
                }
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
            closed = true;
            if (stale()) return;
            if (readInProgress.get()) closedDuringRead = true;
            closes.incrementAndGet();
        }
    }
}
