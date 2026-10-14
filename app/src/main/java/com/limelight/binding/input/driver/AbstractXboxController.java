package com.limelight.binding.input.driver;

import android.hardware.usb.UsbConstants;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbEndpoint;
import android.hardware.usb.UsbInterface;
import android.os.SystemClock;

import com.limelight.LimeLog;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;

public abstract class AbstractXboxController extends AbstractController {
    protected final UsbDevice device;
    protected final UsbDeviceConnection connection;

    private volatile Thread inputThread;
    private volatile boolean stopped;
    private final Object stopLock = new Object();
    private int claimedInterfaceCount;

    protected UsbEndpoint inEndpt, outEndpt;

    public AbstractXboxController(UsbDevice device, UsbDeviceConnection connection, int deviceId, UsbDriverListener listener) {
        super(deviceId, listener, device.getVendorId(), device.getProductId(), device.getDeviceName());
        this.device = device;
        this.connection = connection;
        this.type = MoonBridge.LI_CTYPE_XBOX;
        this.capabilities = MoonBridge.LI_CCAP_ANALOG_TRIGGERS | MoonBridge.LI_CCAP_RUMBLE;
        this.supportedButtonFlags =
                ControllerPacket.A_FLAG | ControllerPacket.B_FLAG | ControllerPacket.X_FLAG | ControllerPacket.Y_FLAG |
                        ControllerPacket.UP_FLAG | ControllerPacket.DOWN_FLAG | ControllerPacket.LEFT_FLAG | ControllerPacket.RIGHT_FLAG |
                        ControllerPacket.LB_FLAG | ControllerPacket.RB_FLAG |
                        ControllerPacket.LS_CLK_FLAG | ControllerPacket.RS_CLK_FLAG |
                        ControllerPacket.BACK_FLAG | ControllerPacket.PLAY_FLAG | ControllerPacket.SPECIAL_BUTTON_FLAG;
    }

    private Thread createInputThread() {
        return new Thread() {
            public void run() {
                try {
                    // Delay for a moment before reporting the new gamepad and
                    // accepting new input. This allows time for the old InputDevice
                    // to go away before we reclaim its spot. If the old device is still
                    // around when we call notifyDeviceAdded(), we won't be able to claim
                    // the controller number used by the original InputDevice.
                    Thread.sleep(1000);
                } catch (InterruptedException e) {
                    return;
                }

                // Report that we're added _before_ reporting input
                notifyDeviceAdded();

                while (!isInterrupted() && !stopped) {
                    byte[] buffer = new byte[64];

                    int res;

                    //
                    // There's no way that I can tell to determine if a device has failed
                    // or if the timeout has simply expired. We'll check how long the transfer
                    // took to fail and assume the device failed if it happened before the timeout
                    // expired.
                    //

                    do {
                        // Read the next input state packet
                        long lastMillis = SystemClock.uptimeMillis();
                        res = connection.bulkTransfer(inEndpt, buffer, buffer.length, 3000);

                        // If we get a zero length response, treat it as an error
                        if (res == 0) {
                            res = -1;
                        }

                        if (res == -1 && SystemClock.uptimeMillis() - lastMillis < 1000) {
                            LimeLog.warning("Detected device I/O error");
                            AbstractXboxController.this.stop();
                            break;
                        }
                    } while (res == -1 && !isInterrupted() && !stopped);

                    if (res == -1 || stopped) {
                        break;
                    }

                    if (handleRead(ByteBuffer.wrap(buffer, 0, res).order(ByteOrder.LITTLE_ENDIAN))) {
                        // Report input if handleRead() returns true
                        reportInput();
                    }
                }
            }
        };
    }

    // Runs on the device's USB worker: the init transfers can each take 3 s on a failing pad.
    public synchronized boolean start() {
        if (stopped) {
            return false;
        }
        // Force claim all interfaces
        for (int i = 0; i < device.getInterfaceCount(); i++) {
            UsbInterface iface = device.getInterface(i);

            if (!connection.claimInterface(iface, true)) {
                LimeLog.warning("Failed to claim interfaces");
                return false;
            }
            claimedInterfaceCount++;
        }

        // Find the endpoints
        UsbInterface iface = device.getInterface(0);
        for (int i = 0; i < iface.getEndpointCount(); i++) {
            UsbEndpoint endpt = iface.getEndpoint(i);
            if (endpt.getDirection() == UsbConstants.USB_DIR_IN) {
                if (inEndpt != null) {
                    LimeLog.warning("Found duplicate IN endpoint");
                    return false;
                }
                inEndpt = endpt;
            }
            else if (endpt.getDirection() == UsbConstants.USB_DIR_OUT) {
                if (outEndpt != null) {
                    LimeLog.warning("Found duplicate OUT endpoint");
                    return false;
                }
                outEndpt = endpt;
            }
        }

        // Make sure the required endpoints were present
        if (inEndpt == null || outEndpt == null) {
            LimeLog.warning("Missing required endpoint");
            return false;
        }

        // Run the init function
        if (!doInit() || stopped) {
            return false;
        }

        // Start listening for controller input
        inputThread = createInputThread();
        inputThread.start();

        return true;
    }

    public void stop() {
        // Not synchronized: a start() still running on the USB worker must not hold up the caller.
        synchronized (stopLock) {
            if (stopped) {
                return;
            }
            stopped = true;
        }

        Thread worker = inputThread;
        if (worker != null) {
            worker.interrupt();
        }

        // Releasing the pad waits for a USB read in progress, so never on the caller's thread.
        runOnUsbWorker(this::release);
    }

    private void release() {
        // Cancel any rumble effects
        if (outEndpt != null) {
            rumble((short)0, (short)0);
            rumbleTriggers((short)0, (short)0);
        }

        // Stop the input thread. An interrupt cannot end a USB read, which returns within its
        // 3 s timeout, and closing the connection under a read in progress is not safe.
        Thread worker = inputThread;
        if (worker != null) {
            worker.interrupt();
            try {
                worker.join(3500);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
            inputThread = null;
        }

        // Let Android reclaim controllers that were detached by claimInterface().
        for (int i = 0; i < claimedInterfaceCount; i++) {
            if (!connection.releaseInterface(device.getInterface(i))) {
                LimeLog.warning("Failed to release controller interface");
            }
        }

        // Close the USB connection
        connection.close();

        // Report the device removed
        notifyDeviceRemoved();
    }

    protected abstract boolean handleRead(ByteBuffer buffer);
    protected abstract boolean doInit();
}
