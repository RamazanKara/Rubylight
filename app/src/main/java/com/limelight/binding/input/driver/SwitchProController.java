package com.limelight.binding.input.driver;

import android.hardware.usb.UsbConstants;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbEndpoint;
import android.hardware.usb.UsbInterface;
import android.os.SystemClock;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;

// Switch Pro Controller over USB, following SDL's USB start: handshake, 3 Mbit link, handshake,
// USB-only mode, calibration from SPI flash, vibration and IMU on, full reports (0x30).
public class SwitchProController extends AbstractController {
    private static final int REPLY_WAIT_MS = 100;
    private static final int ATTEMPTS = 5;
    private static final long RUMBLE_REPEAT_MS = 50;

    private final UsbDevice device;
    private final UsbDeviceConnection connection;
    private final SwitchProReport.StickCalibration sticks = new SwitchProReport.StickCalibration();
    private final SwitchProReport.ImuCalibration imu = new SwitchProReport.ImuCalibration();
    private final SwitchProReport.Input input = new SwitchProReport.Input();
    private final SwitchProReport.Output output = new SwitchProReport.Output();
    // Guards output and the pending output below. USB writes run on the output thread, outside it.
    private final Object outputLock = new Object();
    private final byte[] outputReport = new byte[SwitchProReport.REPORT_SIZE];
    private UsbInterface hidInterface;
    private UsbEndpoint inEndpoint, outEndpoint;
    private volatile Thread inputThread, outputThread;
    private volatile boolean stopped;
    private int lastBatteryByte = -2;
    private long lastRumbleMs;
    private boolean rumblePending, playerPending, forceUsbPending;
    private short rumbleLow, rumbleHigh;
    private int playerNumber;
    private long accelIntervalNs, gyroIntervalNs, lastAccelNs, lastGyroNs;

    public SwitchProController(UsbDevice device, UsbDeviceConnection connection, int deviceId, UsbDriverListener listener) {
        super(deviceId, listener, device.getVendorId(), device.getProductId(), device.getDeviceName());
        this.device = device;
        this.connection = connection;
        type = MoonBridge.LI_CTYPE_NINTENDO;
        capabilities = MoonBridge.LI_CCAP_RUMBLE | MoonBridge.LI_CCAP_ACCEL | MoonBridge.LI_CCAP_GYRO |
                MoonBridge.LI_CCAP_BATTERY_STATE;
        supportedButtonFlags = SwitchProReport.BUTTONS;
    }

    private static UsbInterface findHidInterface(UsbDevice device) {
        for (int i = 0; i < device.getInterfaceCount(); i++) {
            UsbInterface iface = device.getInterface(i);
            if (iface.getInterfaceClass() != UsbConstants.USB_CLASS_HID) {
                continue;
            }
            boolean input = false, output = false;
            for (int j = 0; j < iface.getEndpointCount(); j++) {
                UsbEndpoint endpoint = iface.getEndpoint(j);
                if (endpoint.getType() == UsbConstants.USB_ENDPOINT_XFER_INT) {
                    input |= endpoint.getDirection() == UsbConstants.USB_DIR_IN &&
                            endpoint.getMaxPacketSize() >= SwitchProReport.REPORT_SIZE;
                    output |= endpoint.getDirection() == UsbConstants.USB_DIR_OUT;
                }
            }
            if (input && output) {
                return iface;
            }
        }
        return null;
    }

    public static boolean canClaimDevice(UsbDevice device) {
        return SwitchProReport.isProController(device.getVendorId(), device.getProductId()) &&
                findHidInterface(device) != null;
    }

    int getUsbDeviceId() {
        return device.getDeviceId();
    }

    private boolean write(byte[] report) {
        int size = connection.bulkTransfer(outEndpoint, report, report.length, 100);
        if (size != report.length) {
            LimeLog.warning("Switch Pro output transfer failed: " + size);
            return false;
        }
        return true;
    }

    // Reads until a matching reply arrives or the wait ends; input reports in between are skipped.
    private interface Reply {
        boolean matches(byte[] report, int length);
    }

    private int awaitReply(byte[] buffer, Reply reply) {
        // Wall time, not the uptime clock: the wait must end even where that clock stands still.
        long deadline = System.nanoTime() + REPLY_WAIT_MS * 1000000L;
        while (!stopped) {
            long left = (deadline - System.nanoTime()) / 1000000L;
            if (left <= 0) {
                return -1;
            }
            int size = connection.bulkTransfer(inEndpoint, buffer, buffer.length, (int)left);
            if (size > 0 && reply.matches(buffer, size)) {
                return size;
            }
        }
        return -1;
    }

    private boolean proprietary(byte[] buffer, int command, boolean waitForAck) {
        for (int attempt = 0; attempt < ATTEMPTS && !stopped; attempt++) {
            if (write(output.proprietary(command)) && (!waitForAck ||
                    awaitReply(buffer, (r, n) -> SwitchProReport.isCommandAck(r, n, command)) > 0)) {
                return true;
            }
        }
        return false;
    }

    private boolean subcommand(byte[] buffer, int subcommand, int value) {
        for (int attempt = 0; attempt < ATTEMPTS && !stopped; attempt++) {
            if (write(output.subcommand(subcommand, 1, value)) &&
                    awaitReply(buffer, (r, n) -> SwitchProReport.isSubcommandReply(r, n, subcommand)) > 0) {
                return true;
            }
        }
        return false;
    }

    // Returns the offset of the data in buffer, or -1.
    private int spiRead(byte[] buffer, int address, int size) {
        for (int attempt = 0; attempt < ATTEMPTS && !stopped; attempt++) {
            if (write(output.spiRead(address, size)) &&
                    awaitReply(buffer, (r, n) -> SwitchProReport.spiReadData(r, n, address, size) >= 0) > 0) {
                return SwitchProReport.spiReadData(buffer, buffer.length, address, size);
            }
        }
        return -1;
    }

    private void loadStickCalibration(byte[] buffer) {
        byte[] user = null;
        int offset = spiRead(buffer, SwitchProReport.SPI_STICK_USER, SwitchProReport.SPI_STICK_USER_SIZE);
        if (offset >= 0) {
            user = new byte[SwitchProReport.SPI_STICK_USER_SIZE];
            System.arraycopy(buffer, offset, user, 0, user.length);
        }
        byte[] factory = null;
        if (!SwitchProReport.StickCalibration.hasUserLeft(user) || !SwitchProReport.StickCalibration.hasUserRight(user)) {
            offset = spiRead(buffer, SwitchProReport.SPI_STICK_FACTORY, SwitchProReport.SPI_STICK_FACTORY_SIZE);
            if (offset >= 0) {
                factory = new byte[SwitchProReport.SPI_STICK_FACTORY_SIZE];
                System.arraycopy(buffer, offset, factory, 0, factory.length);
            }
        }
        if (!sticks.load(user, factory)) {
            LimeLog.warning("Switch Pro stick calibration unreadable; using defaults");
        }
    }

    private void loadImuCalibration(byte[] buffer) {
        int offset = spiRead(buffer, SwitchProReport.SPI_IMU_FACTORY, SwitchProReport.SPI_IMU_FACTORY_SIZE);
        if (offset < 0) {
            LimeLog.warning("Switch Pro motion calibration unreadable; using nominal scales");
            return;
        }
        byte[] factory = new byte[SwitchProReport.SPI_IMU_FACTORY_SIZE];
        System.arraycopy(buffer, offset, factory, 0, factory.length);
        byte[] user = null;
        offset = spiRead(buffer, SwitchProReport.SPI_IMU_USER, SwitchProReport.SPI_IMU_USER_SIZE);
        if (offset >= 0) {
            user = new byte[SwitchProReport.SPI_IMU_USER_SIZE];
            System.arraycopy(buffer, offset, user, 0, user.length);
        }
        if (!imu.parse(factory, 0, user, 0)) {
            LimeLog.warning("Switch Pro motion calibration out of range; using nominal scales");
        }
    }

    // Runs on the device's USB worker: every step waits for the pad. A stop() meanwhile ends it
    // early; the release queued behind it cleans up.
    @Override
    public synchronized boolean start() {
        if (stopped) {
            return false;
        }
        UsbInterface iface = findHidInterface(device);
        if (iface == null || !connection.claimInterface(iface, true)) {
            LimeLog.warning("Switch Pro USB HID interface could not be claimed");
            return false;
        }
        hidInterface = iface;
        for (int i = 0; i < iface.getEndpointCount(); i++) {
            UsbEndpoint endpoint = iface.getEndpoint(i);
            if (endpoint.getType() == UsbConstants.USB_ENDPOINT_XFER_INT) {
                if (endpoint.getDirection() == UsbConstants.USB_DIR_IN) {
                    inEndpoint = endpoint;
                }
                else {
                    outEndpoint = endpoint;
                }
            }
        }

        byte[] buffer = new byte[SwitchProReport.REPORT_SIZE];
        // Nothing else writes yet: the pad is announced only once its input thread runs.
        // Some compatible pads skip the speed change or the second handshake; SDL accepts that.
        if (!proprietary(buffer, SwitchProReport.PROPRIETARY_HANDSHAKE, true)) {
            LimeLog.warning("Switch Pro USB handshake failed");
            return false;
        }
        proprietary(buffer, SwitchProReport.PROPRIETARY_HIGH_SPEED, true);
        proprietary(buffer, SwitchProReport.PROPRIETARY_HANDSHAKE, true);
        if (!proprietary(buffer, SwitchProReport.PROPRIETARY_FORCE_USB, false)) {
            return false;
        }
        loadStickCalibration(buffer);
        loadImuCalibration(buffer);
        if (stopped) {
            return false;
        }
        if (!subcommand(buffer, SwitchProReport.SUBCOMMAND_VIBRATION, 1)) {
            LimeLog.warning("Switch Pro did not confirm vibration");
        }
        if (!subcommand(buffer, SwitchProReport.SUBCOMMAND_IMU, 1)) {
            LimeLog.warning("Switch Pro did not confirm motion sensors");
        }
        if (!subcommand(buffer, SwitchProReport.SUBCOMMAND_INPUT_MODE, SwitchProReport.FULL_STATE) ||
                !proprietary(buffer, SwitchProReport.PROPRIETARY_FORCE_USB, false)) {
            LimeLog.warning("Switch Pro full input reports could not be enabled");
            return false;
        }
        if (stopped) {
            return false;
        }
        LimeLog.info("Switch Pro USB HID ready");
        startOutput();
        inputThread = new Thread(this::readInput, "Switch Pro USB input");
        inputThread.start();
        return true;
    }

    void startOutput() {
        outputThread = new Thread(this::writeOutput, "Switch Pro USB output");
        outputThread.start();
    }

    // Rumble, player lights and the USB-only request go out here, so a slow write never holds an
    // input report or the thread that asked for rumble. Only the newest rumble level is sent; an
    // active one is repeated as SDL does.
    private void writeOutput() {
        try {
            while (true) {
                synchronized (outputLock) {
                    while (!stopped && !forceUsbPending && !playerPending && !rumblePending) {
                        long wait = output.rumbleActive() ? lastRumbleMs + RUMBLE_REPEAT_MS - SystemClock.uptimeMillis() : 0;
                        if (output.rumbleActive() && wait <= 0) {
                            break;
                        }
                        outputLock.wait(wait);
                    }
                    if (stopped) {
                        return;
                    }
                    byte[] report;
                    if (forceUsbPending) {
                        forceUsbPending = false;
                        report = output.proprietary(SwitchProReport.PROPRIETARY_FORCE_USB);
                    } else if (playerPending) {
                        playerPending = false;
                        report = output.player(playerNumber);
                    } else {
                        report = rumblePending ? output.rumble(rumbleLow, rumbleHigh) : output.rumbleReport();
                        rumblePending = false;
                        lastRumbleMs = SystemClock.uptimeMillis();
                    }
                    System.arraycopy(report, 0, outputReport, 0, outputReport.length);
                }
                write(outputReport);
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }

    private void readInput() {
        try {
            // Allow Android's detached InputDevice to release its controller number first.
            Thread.sleep(1000);
            synchronized (this) {
                if (stopped) {
                    return;
                }
                notifyDeviceAdded();
            }

            byte[] data = new byte[SwitchProReport.REPORT_SIZE];
            while (!stopped && !Thread.currentThread().isInterrupted()) {
                long before = SystemClock.uptimeMillis();
                int size = connection.bulkTransfer(inEndpoint, data, data.length, 1000);
                long now = SystemClock.uptimeMillis();
                if (size <= 0) {
                    // A fast failure indicates disconnect. A silent pad may have left USB-only
                    // mode (another app can do that); SDL asks for it again.
                    if (now - before < 500) {
                        break;
                    }
                    synchronized (outputLock) {
                        forceUsbPending = true;
                        outputLock.notifyAll();
                    }
                    continue;
                }
                synchronized (this) {
                    if (stopped) {
                        break;
                    }
                    if (!input.parse(data, size, sticks, imu)) {
                        continue;
                    }
                    listener.reportControllerState(getControllerId(), input.buttons,
                            input.leftStickX, input.leftStickY, input.rightStickX, input.rightStickY,
                            input.leftTrigger, input.rightTrigger);
                    if (input.batteryByte != lastBatteryByte) {
                        lastBatteryByte = input.batteryByte;
                        listener.reportControllerBattery(getControllerId(), input.batteryState(), input.batteryPercentage());
                    }
                    if (input.hasMotion) {
                        long nowNs = SystemClock.elapsedRealtimeNanos();
                        if (accelIntervalNs != 0 && nowNs - lastAccelNs >= accelIntervalNs) {
                            listener.reportControllerMotion(getControllerId(), MoonBridge.LI_MOTION_TYPE_ACCEL,
                                    input.accel[0], input.accel[1], input.accel[2]);
                            lastAccelNs = DualSenseController.advanceMotionTimestamp(lastAccelNs, nowNs, accelIntervalNs);
                        }
                        if (gyroIntervalNs != 0 && nowNs - lastGyroNs >= gyroIntervalNs) {
                            listener.reportControllerMotion(getControllerId(), MoonBridge.LI_MOTION_TYPE_GYRO,
                                    input.gyro[0], input.gyro[1], input.gyro[2]);
                            lastGyroNs = DualSenseController.advanceMotionTimestamp(lastGyroNs, nowNs, gyroIntervalNs);
                        }
                    }
                }
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        } finally {
            stop();
        }
    }

    @Override
    public void stop() {
        synchronized (outputLock) {
            if (stopped) {
                return;
            }
            stopped = true;
            outputLock.notifyAll();
        }
        Thread worker = inputThread;
        if (worker != null) {
            worker.interrupt();
        }
        // Releasing the pad waits for a USB read in progress, so never on the caller's thread.
        runOnUsbWorker(this::release);
    }

    private void release() {
        // An interrupt cannot end a USB transfer; a read returns within its 1 s timeout, a write
        // within 100 ms.
        for (Thread worker : new Thread[] {inputThread, outputThread}) {
            if (worker != null) {
                worker.interrupt();
                try {
                    worker.join(1500);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
            }
        }
        synchronized (outputLock) {
            if (hidInterface != null) {
                // Leave the pad quiet: no rumble and the motion sensors off.
                write(output.stopRumble());
                write(output.subcommand(SwitchProReport.SUBCOMMAND_IMU, 1, 0));
                if (!connection.releaseInterface(hidInterface)) {
                    LimeLog.warning("Failed to release Switch Pro HID interface");
                }
                hidInterface = null;
            }
            connection.close();
        }
        notifyDeviceRemoved();
    }

    @Override
    public void rumble(short lowFreqMotor, short highFreqMotor) {
        synchronized (outputLock) {
            if (!stopped && outEndpoint != null) {
                rumbleLow = lowFreqMotor;
                rumbleHigh = highFreqMotor;
                rumblePending = true;
                outputLock.notifyAll();
            }
        }
    }

    @Override
    public void rumbleTriggers(short leftTrigger, short rightTrigger) {
        // The Pro Controller has no trigger motors.
    }

    @Override
    public void setPlayerNumber(int playerNumber) {
        synchronized (outputLock) {
            if (!stopped && outEndpoint != null) {
                this.playerNumber = playerNumber;
                playerPending = true;
                outputLock.notifyAll();
            }
        }
    }

    @Override
    public synchronized void setMotionEventState(byte motionType, short reportRateHz) {
        long interval = reportRateHz == 0 ? 0 : 1000000000L / (reportRateHz & 0xFFFF);
        if (motionType == MoonBridge.LI_MOTION_TYPE_ACCEL) {
            accelIntervalNs = interval;
            lastAccelNs = 0;
        }
        else if (motionType == MoonBridge.LI_MOTION_TYPE_GYRO) {
            gyroIntervalNs = interval;
            lastGyroNs = 0;
        }
    }
}
