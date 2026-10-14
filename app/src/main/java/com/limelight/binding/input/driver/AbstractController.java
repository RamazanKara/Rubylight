package com.limelight.binding.input.driver;

import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;

public abstract class AbstractController {
    // USB transfers can block for seconds when a pad fails. Each USB device's start and release
    // run in order on that device's own worker, never on the caller's (often the main) thread,
    // so a new driver for the same device starts only after the previous one let it go.
    private static final Map<String, ThreadPoolExecutor> usbWorkers = new HashMap<>();
    private final ThreadPoolExecutor usbWork;

    private final int deviceId;
    private final int vendorId;
    private final int productId;

    protected final UsbDriverListener listener;

    protected int buttonFlags, supportedButtonFlags;
    protected float leftTrigger, rightTrigger;
    protected float rightStickX, rightStickY;
    protected float leftStickX, leftStickY;
    protected short capabilities;
    protected byte type;

    public int getControllerId() {
        return deviceId;
    }

    public int getVendorId() {
        return vendorId;
    }

    public int getProductId() {
        return productId;
    }

    public int getSupportedButtonFlags() {
        return supportedButtonFlags;
    }

    public short getCapabilities() {
        return capabilities;
    }

    public byte getType() {
        return type;
    }

    protected void setButtonFlag(int buttonFlag, int data) {
        if (data != 0) {
            buttonFlags |= buttonFlag;
        }
        else {
            buttonFlags &= ~buttonFlag;
        }
    }

    protected void reportInput() {
        listener.reportControllerState(deviceId, buttonFlags, leftStickX, leftStickY,
                rightStickX, rightStickY, leftTrigger, rightTrigger);
    }

    // start() may block on USB transfers; the USB service runs it on the device's worker.
    public abstract boolean start();
    // Safe on any thread: stops input at once and leaves blocking USB work to the device's worker.
    public abstract void stop();

    public AbstractController(int deviceId, UsbDriverListener listener, int vendorId, int productId) {
        this(deviceId, listener, vendorId, productId, null);
    }

    protected AbstractController(int deviceId, UsbDriverListener listener, int vendorId, int productId,
                                 String usbDeviceName) {
        this.deviceId = deviceId;
        this.listener = listener;
        this.vendorId = vendorId;
        this.productId = productId;
        this.usbWork = usbDeviceName == null ? newUsbWorker() : usbWorkerFor(usbDeviceName);
    }

    private static ThreadPoolExecutor newUsbWorker() {
        // No thread exists until there is work, and an idle one ends after a second.
        ThreadPoolExecutor worker = new ThreadPoolExecutor(1, 1, 1, TimeUnit.SECONDS,
                new LinkedBlockingQueue<>(), work -> new Thread(work, "USB controller"));
        worker.allowCoreThreadTimeOut(true);
        return worker;
    }

    private static synchronized ThreadPoolExecutor usbWorkerFor(String usbDeviceName) {
        return usbWorkers.computeIfAbsent(usbDeviceName, name -> newUsbWorker());
    }

    protected final void runOnUsbWorker(Runnable work) {
        usbWork.execute(work);
    }

    // Starts the pad off the caller's thread. A stop() requested meanwhile runs after this start.
    final void startOnUsbWorker(Runnable onFailure) {
        runOnUsbWorker(() -> {
            if (!start()) {
                stop();
                onFailure.run();
            }
        });
    }

    public abstract void rumble(short lowFreqMotor, short highFreqMotor);

    public abstract void rumbleTriggers(short leftTrigger, short rightTrigger);

    public void controllerHaptics(int sequence, byte[] pcm) {}
    public void controllerAudio(int kind, byte[] payload) {}
    public void bindAudio(com.limelight.nvstream.NvConnection connection, int number) {}
    public boolean headphoneAudio(short[] samples, int length, float volume) { return false; }

    public void setAdaptiveTriggers(byte eventFlags, byte typeLeft, byte typeRight, byte[] left, byte[] right) {}

    public void setLed(byte r, byte g, byte b) {}

    public void setPlayerNumber(int playerNumber) {}

    public void setMotionEventState(byte motionType, short reportRateHz) {}

    protected void notifyDeviceRemoved() {
        listener.deviceRemoved(this);
    }

    protected void notifyDeviceAdded() {
        listener.deviceAdded(this);
    }
}
