package com.limelight.binding.input.driver;

import android.hardware.usb.UsbConstants;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbEndpoint;
import android.hardware.usb.UsbInterface;
import android.hardware.usb.UsbManager;
import android.os.SystemClock;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.input.ControllerPacket;

public class DualSenseController extends AbstractController {
    private final UsbDevice device;
    private final UsbDeviceConnection connection;
    private final boolean edge;
    private final UsbManager waveformUsbManager;
    private volatile DualSenseHaptics haptics;
    private final DualSenseReport.Calibration calibration = new DualSenseReport.Calibration();
    private DualSenseReport.Input input = new DualSenseReport.Input();
    private DualSenseReport.Input previous = new DualSenseReport.Input();
    private DualSenseReport.Output output;
    // Guards output and the pending output below. USB writes run on the output thread, outside it.
    private final Object outputLock = new Object();
    private boolean rumblePending, ledPending, playerPending, waveformPending, audioPending;
    private short rumbleLow, rumbleHigh;
    private byte ledRed, ledGreen, ledBlue;
    private int playerNumber;
    private boolean waveformEnabled, audioHeadphones, audioMuted;
    private int triggerSides;
    private byte triggerTypeLeft, triggerTypeRight;
    private final byte[] triggerLeft = new byte[DualSenseReport.EFFECT_SIZE];
    private final byte[] triggerRight = new byte[DualSenseReport.EFFECT_SIZE];
    private UsbInterface hidInterface;
    private UsbEndpoint inEndpoint, outEndpoint;
    private volatile Thread inputThread, outputThread;
    private volatile boolean stopped;
    private boolean batteryReported;
    private boolean waveformReported, triggersReported;
    private volatile com.limelight.nvstream.NvConnection audioConnection;
    private volatile boolean audioNegotiated, microphoneAccepted, headphonesConnected, hostMicMuted;
    private volatile Object microphoneOwner;
    private final java.util.ArrayDeque<byte[]> audioControls = new java.util.ArrayDeque<>();
    private int audioNumber, micSequence, lastAudioFlags = -1;
    private long audioStatusAt;
    private long accelIntervalNs, gyroIntervalNs, lastAccelNs, lastGyroNs;

    public DualSenseController(UsbDevice device, UsbDeviceConnection connection, int deviceId, UsbDriverListener listener, UsbManager waveformUsbManager) {
        super(deviceId, listener, device.getVendorId(), device.getProductId(), device.getDeviceName());
        this.device = device;
        this.waveformUsbManager = waveformUsbManager;
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

    // Runs on the device's USB worker: the feature reads below can each take a second on a failing
    // pad. A stop() meanwhile ends it early; the release queued behind it cleans up.
    @Override
    public synchronized boolean start() {
        if (stopped) {
            return false;
        }
        UsbInterface iface = findHidInterface(device);
        if (iface == null || !connection.claimInterface(iface, true)) {
            LimeLog.warning("DualSense USB HID interface could not be claimed");
            return false;
        }
        // Audio remains with Android unless the separate waveform option is enabled.
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

        if (stopped) {
            return false;
        }
        byte[] feature = new byte[41];
        calibration.parse(feature, readFeature(0x05, feature));
        feature = new byte[64];
        int size = readFeature(0x20, feature);
        int firmware = size >= 46 && feature[0] == 0x20 ?
                (feature[44] & 0xFF) | ((feature[45] & 0xFF) << 8) : 0;
        output = new DualSenseReport.Output(edge || firmware >= 0x0224);
        if (stopped) {
            return false;
        }
        if (!writeOutput(output.reset())) {
            connection.releaseInterface(hidInterface);
            hidInterface = null;
            return false;
        }
        LimeLog.info("DualSense USB HID ready: firmware=0x" + Integer.toHexString(firmware) +
                ", waveform requested=" + (waveformUsbManager != null));

        if (waveformUsbManager != null) {
            try {
                DualSenseUsbAudio audio = DualSenseUsbAudio.open(waveformUsbManager, device);
                if (audio != null) {
                    haptics = new DualSenseHaptics(audio, this::setWaveformMode);
                    capabilities |= MoonBridge.LI_CCAP_HAPTICS_PCM;
                    LimeLog.info("DualSense USB waveform backend ready");
                }
                else LimeLog.warning("DualSense USB waveform output unavailable; ordinary feedback remains enabled");
            }
            catch (SecurityException e) {
                LimeLog.warning("DualSense USB waveform access denied; ordinary feedback remains enabled");
            }
        }

        if (stopped) {
            return false;
        }
        startOutput();
        inputThread = new Thread(this::readInput, "DualSense USB input");
        inputThread.start();
        return true;
    }

    void startOutput() {
        outputThread = new Thread(this::runOutput, "DualSense USB output");
        outputThread.start();
    }

    // Rumble, lights, trigger effects and audio state go out here, so a slow write never holds the
    // input thread or the caller, which for rumble, LED and triggers is the shared controller lock.
    // Only the newest of each is sent.
    private void runOutput() {
        try {
            while (true) {
                byte[] report;
                synchronized (outputLock) {
                    while (true) {
                        while (!stopped && !hasPendingOutput()) {
                            outputLock.wait();
                        }
                        if (stopped) {
                            return;
                        }
                        report = nextOutputReport();
                        if (report != null) {
                            break;
                        }
                    }
                }
                writeOutput(report);
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }

    private boolean hasPendingOutput() {
        if (waveformPending || audioPending || triggerSides != 0 || ledPending || playerPending || rumblePending) {
            return true;
        }
        synchronized (audioControls) {
            return !audioControls.isEmpty();
        }
    }

    // Builds the next report from what is pending, or null when that report would send nothing.
    private byte[] nextOutputReport() {
        if (waveformPending) {
            waveformPending = false;
            return output.waveform(waveformEnabled);
        }
        if (audioPending) {
            audioPending = false;
            return output.audio(audioHeadphones, audioMuted);
        }
        byte[] control;
        synchronized (audioControls) {
            control = audioControls.pollFirst();
        }
        if (control != null) {
            return output.audioControl(control);
        }
        if (triggerSides != 0) {
            int sides = triggerSides;
            triggerSides = 0;
            return output.adaptiveTriggers((byte) sides, triggerTypeLeft, triggerTypeRight, triggerLeft, triggerRight);
        }
        if (ledPending) {
            ledPending = false;
            return output.led(ledRed, ledGreen, ledBlue);
        }
        if (playerPending) {
            playerPending = false;
            return output.player(playerNumber);
        }
        rumblePending = false;
        return output.rumble(rumbleLow, rumbleHigh);
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
                    headphonesConnected = input.headphones;
                    com.limelight.nvstream.NvConnection audioConn = audioConnection;
                    if (audioConn != null && (input.buttons & ControllerPacket.MISC_FLAG) != 0 && (previous.buttons & ControllerPacket.MISC_FLAG) == 0) {
                        audioConn.setMicrophoneMuted(!audioConn.isMicrophoneMuted());
                    }
                    updateAudioState();
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
    public short getCapabilities() {
        DualSenseHaptics playback = haptics;
        return playback != null && playback.isReady() ? capabilities :
                (short)(capabilities & ~MoonBridge.LI_CCAP_HAPTICS_PCM);
    }

    private void setWaveformMode(boolean enabled) {
        synchronized (outputLock) {
            if (!stopped) {
                waveformEnabled = enabled;
                waveformPending = true;
                // Rumble is not sent while waveform playback owns the motors.
                if (enabled) rumblePending = false;
                outputLock.notifyAll();
            }
        }
    }

    @Override
    public void controllerHaptics(int sequence, byte[] pcm) {
        DualSenseHaptics playback = haptics;
        if (!stopped && playback != null && playback.isReady()) {
            if (!waveformReported) {
                waveformReported = true;
                LimeLog.info("DualSense USB first waveform packet received");
            }
            playback.offer(sequence, pcm);
        }
    }

    @Override
    public void bindAudio(com.limelight.nvstream.NvConnection conn, int number) {
        DualSenseHaptics playback = haptics;
        if (stopped || !conn.canSendControllerInput() || playback == null || !playback.isReady() ||
                !MoonBridge.hostSupportsControlMessage(DualSenseAudioPacket.CONTROL)) return;
        if (audioConnection == conn && audioNumber == number) {
            lastAudioFlags = -1;
            updateAudioState();
            return;
        }
        com.limelight.nvstream.NvConnection previousConnection = audioConnection;
        Object previousOwner = microphoneOwner;
        Object owner = new Object();
        microphoneOwner = owner;
        if (previousConnection != null) previousConnection.releaseControllerMicrophone(previousOwner);
        audioNumber = number; audioConnection = conn;
        audioNegotiated = microphoneAccepted = hostMicMuted = false;
        playback.microphone(createMicrophoneSink(conn, owner));
        lastAudioFlags = -1;
        updateAudioState();
    }
    private DualSenseHaptics.Microphone createMicrophoneSink(com.limelight.nvstream.NvConnection conn, Object owner) {
        return new DualSenseHaptics.Microphone() {
            public boolean start() { return conn.beginControllerMicrophone(); }
            public void finish() { conn.endControllerMicrophone(); }
            public boolean enabled() { return !stopped && microphoneOwner == owner && microphoneAccepted && !hostMicMuted && conn.controllerMicrophoneEnabled(); }
            public void ready(boolean ready) {
                if (microphoneOwner != owner) return;
                conn.controllerMicrophoneState(owner, microphoneAccepted, ready, hostMicMuted);
            }
            public void closed() {
                if (microphoneOwner == owner) {
                    // An idle worker can be interrupted before ready(false) observes
                    // the last host mute. Publish it before releasing the route.
                    boolean muted = hostMicMuted;
                    // Revoked controller permission may discard route state, but
                    // must not discard the mute choice carried into a reconnect.
                    if (muted) conn.setMicrophoneMuted(true);
                    conn.controllerMicrophoneState(owner, microphoneAccepted, false, muted);
                    microphoneOwner = null;
                }
                conn.releaseControllerMicrophone(owner);
            }
            public void samples(byte[] pcm, int length) {
                if (enabled()) MoonBridge.sendControllerAudio(DualSenseAudioPacket.MICROPHONE,
                        DualSenseAudioPacket.pcm(audioNumber, micSequence++, pcm, length));
            }
        };
    }
    private void updateAudioState() {
        com.limelight.nvstream.NvConnection conn = audioConnection;
        if (conn == null || stopped || !conn.canSendControllerInput()) return;
        int flags = 1 | (conn.controllerMicrophonePermitted() ? 2 : 0) | (headphonesConnected ? 4 : 0) |
                (conn.isMicrophoneMuted() || hostMicMuted ? 8 : 0);
        long now = SystemClock.uptimeMillis();
        if (flags != lastAudioFlags || now - audioStatusAt >= 1000) {
            if (MoonBridge.sendControllerAudio(DualSenseAudioPacket.CONTROL, DualSenseAudioPacket.status(audioNumber, flags))) {
                lastAudioFlags = flags; audioStatusAt = now;
            }
            synchronized (outputLock) {
                if (!stopped) {
                    audioHeadphones = headphonesConnected;
                    audioMuted = !conn.controllerMicrophoneEnabled() || hostMicMuted;
                    audioPending = true;
                    outputLock.notifyAll();
                }
            }
        }
    }
    @Override
    public void controllerAudio(int kind, byte[] payload) {
        if (stopped || audioConnection == null || !audioConnection.canSendControllerInput() || DualSenseAudioPacket.id(payload) != audioNumber) return;
        DualSenseHaptics playback = haptics;
        if (kind == DualSenseAudioPacket.SPEAKER && DualSenseAudioPacket.validPcm(payload) && playback != null) {
            playback.speaker(DualSenseAudioPacket.sequence(payload), java.util.Arrays.copyOfRange(payload, 12, payload.length));
        } else if (kind == DualSenseAudioPacket.CONTROL && DualSenseAudioPacket.validControl(payload)) {
            if (payload[1] == 3) {
                audioNegotiated = (payload[4] & 1) != 0;
                microphoneAccepted = (payload[4] & 2) != 0;
            } else {
                if ((payload[5] & 2) != 0) hostMicMuted = (payload[11] & 0x10) != 0;
                synchronized (audioControls) {
                    if (audioControls.size() == 32) audioControls.removeFirst();
                    audioControls.addLast(payload.clone());
                }
                synchronized (outputLock) {
                    outputLock.notifyAll();
                }
            }
        }
    }
    @Override
    public boolean headphoneAudio(short[] samples, int length, float volume) {
        DualSenseHaptics playback = haptics;
        com.limelight.nvstream.NvConnection conn = audioConnection;
        if (stopped || conn == null || !conn.canSendControllerInput() || !audioNegotiated ||
                !headphonesConnected || playback == null || !playback.isReady()) return false;
        playback.headphones(samples, length, volume);
        return true;
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
        // The microphone worker publishes the host's mute only when it closes, which can be after
        // the stream has read it for a reconnect. Keep it now.
        com.limelight.nvstream.NvConnection conn = audioConnection;
        if (conn != null && microphoneOwner != null && hostMicMuted) {
            conn.setMicrophoneMuted(true);
        }
        // Releasing the pad waits for a USB read in progress, so never on the caller's thread.
        runOnUsbWorker(this::release);
    }

    private void release() {
        // The worker threads may be using the pad, so join them without holding outputLock.
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
        DualSenseHaptics playback = haptics;
        if (playback != null) playback.close();
        synchronized (outputLock) {
            if (hidInterface != null) {
                if (output != null) {
                    writeOutput(output.reset());
                }
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
                rumbleLow = lowFreqMotor;
                rumbleHigh = highFreqMotor;
                rumblePending = true;
                outputLock.notifyAll();
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
                if (!triggersReported) {
                    triggersReported = true;
                    LimeLog.info("DualSense USB first adaptive-trigger packet received");
                }
                int sides = eventFlags & (DualSenseReport.LEFT_TRIGGER | DualSenseReport.RIGHT_TRIGGER);
                boolean leftValid = left != null && left.length == DualSenseReport.EFFECT_SIZE;
                boolean rightValid = right != null && right.length == DualSenseReport.EFFECT_SIZE;
                if (sides == 0 || ((sides & DualSenseReport.LEFT_TRIGGER) != 0 && !leftValid) ||
                        ((sides & DualSenseReport.RIGHT_TRIGGER) != 0 && !rightValid)) {
                    return;
                }
                // Effects for one trigger replace an earlier unsent one; the other trigger's stay.
                if ((sides & DualSenseReport.LEFT_TRIGGER) != 0) {
                    triggerTypeLeft = typeLeft;
                    System.arraycopy(left, 0, triggerLeft, 0, DualSenseReport.EFFECT_SIZE);
                }
                if ((sides & DualSenseReport.RIGHT_TRIGGER) != 0) {
                    triggerTypeRight = typeRight;
                    System.arraycopy(right, 0, triggerRight, 0, DualSenseReport.EFFECT_SIZE);
                }
                triggerSides |= sides;
                outputLock.notifyAll();
            }
        }
    }

    @Override
    public void setLed(byte r, byte g, byte b) {
        synchronized (outputLock) {
            if (!stopped) {
                ledRed = r;
                ledGreen = g;
                ledBlue = b;
                ledPending = true;
                outputLock.notifyAll();
            }
        }
    }

    @Override
    public void setPlayerNumber(int playerNumber) {
        synchronized (outputLock) {
            if (!stopped) {
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
