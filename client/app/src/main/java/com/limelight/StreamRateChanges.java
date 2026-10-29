package com.limelight;

import com.limelight.nvstream.ClientHintBitrateController;
import com.limelight.nvstream.RateChangeResult;
import com.limelight.preferences.StreamLayers;

import java.util.EnumSet;
import java.util.HashMap;
import java.util.Iterator;
import java.util.Map;
import java.util.concurrent.atomic.AtomicInteger;

// The stream's 0x553C rate changes: the ones asked for in the stream menu, and the client-hint
// automatic bitrate's. Each request gets a serial the host echoes, so its result reaches whoever
// asked. Automatic bitrate waits while a request from the menu is outstanding, and a bitrate set
// by hand becomes its new start. A request the host leaves unanswered is given up on after
// ANSWER_MS, as on desktop (nightjar-desktop's stream::rate_change).
final class StreamRateChanges {
    // A request the host has not answered in this long is given up on, loudly.
    static final long ANSWER_MS = 5000;

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
        // Automatic bitrate's request from before a bitrate set by hand, or from before it
        // started again.
        STALE,
        // No request of this stream has the serial.
        UNKNOWN,
        // A request from the menu got no answer in ANSWER_MS: say so.
        MANUAL_NO_ANSWER,
        // Automatic bitrate's request got no answer in ANSWER_MS, so it stopped for this stream.
        AUTOMATIC_NO_ANSWER,
    }

    // What became of one result.
    static final class Outcome {
        final Answer answer;
        // The result's bitrate is the stream's: no answer to a later request came first. The host
        // answers a request with a rate once its switch ran, with the bitrate it set then, so a
        // bitrate-only request sent after it is answered first and has the newer bitrate.
        final boolean takesBitrate;

        Outcome(Answer answer, boolean takesBitrate) {
            this.answer = answer;
            this.takesBitrate = takesBitrate;
        }
    }

    private static final class Request {
        final boolean manual;
        final long sentMs;
        // This stream's requests in the order they went out.
        final long order;

        Request(boolean manual, long sentMs, long order) {
            this.manual = manual;
            this.sentMs = sentMs;
            this.order = order;
        }
    }

    // Serials count across streams, so a late result of an earlier stream (one queued before a
    // restart) matches no request of this one. 0 is skipped, as automaticSerial 0 means none.
    private static final AtomicInteger lastSerial = new AtomicInteger();

    private final Sender sender;
    private final Map<Integer, Request> outstanding = new HashMap<>();
    private long lastOrder;
    // The order of the newest request whose answer set the bitrate; 0 for none yet.
    private long bitrateOrder;
    private int manualOutstanding;
    private int automaticSerial;
    private ClientHintBitrateController automatic;

    StreamRateChanges(Sender sender) {
        this.sender = sender;
    }

    private static int nextSerial() {
        int serial = lastSerial.incrementAndGet();
        return serial != 0 ? serial : lastSerial.incrementAndGet();
    }

    // A request from the menu; 0 keeps that value. False when it could not be sent.
    boolean sendManual(int bitrateKbps, int fpsMillihz, long nowMs) {
        int serial = nextSerial();
        if (!sender.send(serial, bitrateKbps, fpsMillihz)) return false;
        outstanding.put(serial, new Request(true, nowMs, ++lastOrder));
        manualOutstanding++;
        return true;
    }

    void startAutomatic(int initialKbps, long nowMs) {
        startAutomatic(initialKbps, 0, nowMs);
    }

    // limitKbps is this client's cap on the range (see clientLimitKbps), 0 for none.
    void startAutomatic(int initialKbps, int limitKbps, long nowMs) {
        automatic = new ClientHintBitrateController(initialKbps, limitKbps, nowMs);
        automaticSerial = 0;
    }

    // Automatic bitrate's own cap: the mobile data limit on a mobile network, none elsewhere, as
    // on desktop (Profiles::bitrate_limit). Its range otherwise reaches twice the start.
    static int clientLimitKbps(StreamLayers.Network network, int mobileLimitKbps) {
        return network == StreamLayers.Network.MOBILE && mobileLimitKbps > 0 ? mobileLimitKbps : 0;
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
        int serial = nextSerial();
        if (!sender.send(serial, kbps, 0)) {
            stopAutomatic();
            return false;
        }
        outstanding.put(serial, new Request(false, nowMs, ++lastOrder));
        automaticSerial = serial;
        return true;
    }

    Outcome onResult(RateChangeResult result, long nowMs) {
        Request request = outstanding.remove(result.serial);
        if (request == null) return new Outcome(Answer.UNKNOWN, false);
        boolean takesBitrate = result.bitrateKbps > 0 && request.order > bitrateOrder;
        if (takesBitrate) bitrateOrder = request.order;
        if (request.manual) {
            manualOutstanding--;
            if (automatic != null && takesBitrate && result.bitrateChanged()) {
                automatic.overridden(result.bitrateKbps, nowMs);
            }
            return new Outcome(Answer.MANUAL, takesBitrate);
        }
        if (automatic == null || result.serial != automaticSerial || !automatic.pending()) {
            return new Outcome(Answer.STALE, takesBitrate);
        }
        automaticSerial = 0;
        if (result.bitrateChanged()) {
            automatic.applied(result.bitrateKbps, nowMs);
            return new Outcome(Answer.AUTOMATIC, takesBitrate);
        }
        automatic.refused();
        automatic = null;
        return new Outcome(Answer.AUTOMATIC_STOPPED, takesBitrate);
    }

    // When the oldest outstanding request is given up on, in uptime; 0 when none is.
    long nextExpiryMs() {
        long next = 0;
        for (Request request : outstanding.values()) {
            long at = request.sentMs + ANSWER_MS;
            if (next == 0 || at < next) next = at;
        }
        return next;
    }

    // Gives up on requests the host has not answered within ANSWER_MS; an answer that still comes
    // is UNKNOWN. Automatic bitrate stops, as for a refusal, when its current request is one.
    // MANUAL_NO_ANSWER and AUTOMATIC_NO_ANSWER say what to tell the user.
    EnumSet<Answer> expire(long nowMs) {
        EnumSet<Answer> expired = EnumSet.noneOf(Answer.class);
        for (Iterator<Map.Entry<Integer, Request>> it = outstanding.entrySet().iterator(); it.hasNext(); ) {
            Map.Entry<Integer, Request> entry = it.next();
            Request request = entry.getValue();
            if (nowMs < request.sentMs + ANSWER_MS) continue;
            it.remove();
            int serial = entry.getKey();
            LimeLog.warning("Rate change: serial " + Integer.toUnsignedString(serial) + " got no answer in " +
                    ANSWER_MS + " ms");
            if (request.manual) {
                manualOutstanding--;
                expired.add(Answer.MANUAL_NO_ANSWER);
            } else if (automatic != null && serial == automaticSerial) {
                automaticSerial = 0;
                automatic.refused();
                automatic = null;
                expired.add(Answer.AUTOMATIC_NO_ANSWER);
            }
        }
        return expired;
    }
}
