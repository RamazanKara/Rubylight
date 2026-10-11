package com.limelight.binding.input.driver;

import android.hardware.usb.UsbConstants;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbEndpoint;
import android.hardware.usb.UsbInterface;
import android.os.SystemClock;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;

public class DualSenseController extends AbstractController {
    private final UsbDevice device;
    private final UsbDeviceConnection connection;
    private final boolean edge;
    private final DualSenseReport.Calibration calibration = new DualSenseReport.Calibration();
    private DualSenseReport.Input input = new DualSenseReport.Input();
    private DualSenseReport.Input previous = new DualSenseReport.Input();
    private DualSenseReport.Output output;
    private final Object outputLock = new Object();
    private UsbInterface hidInterface;
    private UsbEndpoint inEndpoint, outEndpoint;
    private Thread inputThread;
    private volatile boolean stopped;
    private boolean batteryReported;
    private long accelIntervalNs, gyroIntervalNs, lastAccelNs, lastGyroNs;

    public DualSenseController(UsbDevice device, UsbDeviceConnection connection, int deviceId, UsbDriverListener listener) {
        super(deviceId, listener, device.getVendorId(), device.getProductId());
        this.device = device;
        this.connection = connection;
        edge = device.getProductId() == 0x0DF2;
        type = MoonBridge.LI_CTYPE_PS;
        capabilities = MoonBridge.LI_CCAP_ANALOG_TRIGGERS | MoonBridge.LI_CCAP_RUMBLE |
                MoonBridge.LI_CCAP_TOUCHPAD | MoonBridge.LI_CCAP_ACCEL | MoonBridge.LI_CCAP_GYRO |
                MoonBridge.LI_CCAP_RGB_LED | MoonBridge.LI_CCAP_BATTERY_STATE;
        supportedButtonFlags = DualSenseReport.BUTTONS | (edge ? DualSenseReport.EDGE_BUTTONS : 0);
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
                            endpoint.getMaxPacketSize() >= DualSenseReport.INPUT_SIZE;
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
        return device.getVendorId() == 0x054C &&
                (device.getProductId() == 0x0CE6 || device.getProductId() == 0x0DF2) &&
                findHidInterface(device) != null;
    }

    int getUsbDeviceId() {
        return device.getDeviceId();
    }

    private int readFeature(int reportId, byte[] data) {
        return connection.controlTransfer(0xA1, 0x01, 0x0300 | reportId,
                hidInterface.getId(), data, data.length, 1000);
    }

    @Override
    public synchronized boolean start() {
        UsbInterface iface = findHidInterface(device);
        if (iface == null || !connection.claimInterface(iface, true)) {
            return false;
        }
        // Claim only HID: the other interfaces belong to the controller's USB audio device.
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

        byte[] feature = new byte[41];
        calibration.parse(feature, readFeature(0x05, feature));
        feature = new byte[64];
        int size = readFeature(0x20, feature);
        int firmware = size >= 46 && feature[0] == 0x20 ?
                (feature[44] & 0xFF) | ((feature[45] & 0xFF) << 8) : 0;
        output = new DualSenseReport.Output(edge || firmware >= 0x0224);
        if (!writeOutput(output.reset())) {
            connection.releaseInterface(hidInterface);
            hidInterface = null;
            return false;
        }

        inputThread = new Thread(this::readInput, "DualSense USB input");
        inputThread.start();
        return true;
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

            byte[] data = new byte[DualSenseReport.INPUT_SIZE];
            while (!stopped && !Thread.currentThread().isInterrupted()) {
                long before = SystemClock.uptimeMillis();
                int size = connection.bulkTransfer(inEndpoint, data, data.length, 1000);
                if (size <= 0) {
                    // A fast failure indicates disconnect; an expired read can be retried.
                    if (SystemClock.uptimeMillis() - before < 500) {
                        break;
                    }
                    continue;
                }
                synchronized (this) {
                    if (stopped) {
                        break;
                    }
                    if (!input.parse(data, size, edge)) {
                        continue;
                    }
                    calibration.apply(input);
                    listener.reportControllerState(getControllerId(), input.buttons,
                            input.leftStickX, input.leftStickY, input.rightStickX, input.rightStickY,
                            input.leftTrigger, input.rightTrigger);
                    input.reportTouchChanges(previous, listener, getControllerId());
                    if (!batteryReported || input.batteryState != previous.batteryState ||
                            input.batteryPercentage != previous.batteryPercentage) {
                        listener.reportControllerBattery(getControllerId(), input.batteryState, input.batteryPercentage);
                        batteryReported = true;
                    }

                    long now = SystemClock.elapsedRealtimeNanos();
                    if (accelIntervalNs != 0 && now - lastAccelNs >= accelIntervalNs) {
                        listener.reportControllerMotion(getControllerId(), MoonBridge.LI_MOTION_TYPE_ACCEL,
                                input.accel[0], input.accel[1], input.accel[2]);
                        lastAccelNs = advanceMotionTimestamp(lastAccelNs, now, accelIntervalNs);
                    }
                    if (gyroIntervalNs != 0 && now - lastGyroNs >= gyroIntervalNs) {
                        listener.reportControllerMotion(getControllerId(), MoonBridge.LI_MOTION_TYPE_GYRO,
                                input.gyro[0], input.gyro[1], input.gyro[2]);
                        lastGyroNs = advanceMotionTimestamp(lastGyroNs, now, gyroIntervalNs);
                    }
                    DualSenseReport.Input temp = previous;
                    previous = input;
                    input = temp;
                }
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        } finally {
            stop();
        }
    }

    static long advanceMotionTimestamp(long previous, long now, long interval) {
        return previous == 0 ? now : now - (now - previous) % interval;
    }

    private boolean writeOutput(byte[] report) {
        if (report == null) {
            return false;
        }
        int size = connection.bulkTransfer(outEndpoint, report, report.length, 100);
        if (size != report.length) {
            LimeLog.warning("DualSense output transfer failed: " + size);
            return false;
        }
        return true;
    }

    @Override
    public void stop() {
        synchronized (outputLock) {
            if (stopped) {
                return;
            }
            stopped = true;
            if (inputThread != null) {
                inputThread.interrupt();
            }
            if (hidInterface != null) {
                writeOutput(output.reset());
                if (!connection.releaseInterface(hidInterface)) {
                    LimeLog.warning("Failed to release DualSense HID interface");
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
            if (!stopped) {
                writeOutput(output.rumble(lowFreqMotor, highFreqMotor));
            }
        }
    }

    @Override
    public void rumbleTriggers(short leftTrigger, short rightTrigger) {
        // Adaptive trigger effects arrive through setAdaptiveTriggers(), not Xbox trigger rumble.
    }

    @Override
    public void setAdaptiveTriggers(byte eventFlags, byte typeLeft, byte typeRight, byte[] left, byte[] right) {
        synchronized (outputLock) {
            if (!stopped) {
                writeOutput(output.adaptiveTriggers(eventFlags, typeLeft, typeRight, left, right));
            }
        }
    }

    @Override
    public void setLed(byte r, byte g, byte b) {
        synchronized (outputLock) {
            if (!stopped) {
                writeOutput(output.led(r, g, b));
            }
        }
    }

    @Override
    public void setPlayerNumber(int playerNumber) {
        synchronized (outputLock) {
            if (!stopped) {
                writeOutput(output.player(playerNumber));
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
