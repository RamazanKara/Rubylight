package com.limelight;

import com.limelight.nvstream.ClientHintBitrateController;
import com.limelight.nvstream.RateChangeResult;

import java.util.HashMap;
import java.util.Map;

// The stream's 0x553C rate changes: the ones asked for in the stream menu, and the client-hint
// automatic bitrate's. Each request gets a serial the host echoes, so its result reaches whoever
// asked. Automatic bitrate waits while a request from the menu is outstanding, and a bitrate set
// by hand becomes its new start.
final class StreamRateChanges {
    interface Sender {
        // MoonBridge.sendRateChange: false when it could not be sent.
        boolean send(int serial, int bitrateKbps, int fpsMillihz);
    }

    enum Answer {
        // The result of a request from the menu: say what became of it.
        MANUAL,
        // Automatic bitrate's request ran.
        AUTOMATIC,
        // Automatic bitrate's request was refused, so it stopped for this stream: say why.
        AUTOMATIC_STOPPED,
        // Automatic bitrate's request from before a bitrate set by hand.
        STALE,
        // No request of this stream has the serial.
        UNKNOWN,
    }

    private final Sender sender;
    private final Map<Integer, Boolean> outstanding = new HashMap<>();
    private int nextSerial = 1;
    private int manualOutstanding;
    private int automaticSerial;
    private ClientHintBitrateController automatic;

    StreamRateChanges(Sender sender) {
        this.sender = sender;
    }

    // A request from the menu; 0 keeps that value. False when it could not be sent.
    boolean sendManual(int bitrateKbps, int fpsMillihz) {
        int serial = nextSerial++;
        if (!sender.send(serial, bitrateKbps, fpsMillihz)) return false;
        outstanding.put(serial, true);
        manualOutstanding++;
        return true;
    }

    void startAutomatic(int initialKbps, long nowMs) {
        automatic = new ClientHintBitrateController(initialKbps, 0, nowMs);
        automaticSerial = 0;
    }

    void stopAutomatic() {
        automatic = null;
        automaticSerial = 0;
    }

    boolean automaticOn() {
        return automatic != null;
    }

    ClientHintBitrateController automatic() {
        return automatic;
    }

    // Automatic bitrate's sample once a second. False when its request could not be sent, which
    // stops it for this stream.
    boolean sampleAutomatic(long nowMs, boolean recentVideo, float lossPercent, float queueMs) {
        if (automatic == null || manualOutstanding > 0) return true;
        int kbps = automatic.sample(nowMs, recentVideo, lossPercent, queueMs);
        if (kbps == 0) return true;
        int serial = nextSerial++;
        if (!sender.send(serial, kbps, 0)) {
            stopAutomatic();
            return false;
        }
        outstanding.put(serial, false);
        automaticSerial = serial;
        return true;
    }

    Answer onResult(RateChangeResult result, long nowMs) {
        Boolean manual = outstanding.remove(result.serial);
        if (manual == null) return Answer.UNKNOWN;
        if (manual) {
            manualOutstanding--;
            if (automatic != null && result.bitrateChanged()) automatic.overridden(result.bitrateKbps, nowMs);
            return Answer.MANUAL;
        }
        if (automatic == null || result.serial != automaticSerial || !automatic.pending()) return Answer.STALE;
        automaticSerial = 0;
        if (result.bitrateChanged()) {
            automatic.applied(result.bitrateKbps, nowMs);
            return Answer.AUTOMATIC;
        }
        automatic.refused();
        automatic = null;
        return Answer.AUTOMATIC_STOPPED;
    }
}
