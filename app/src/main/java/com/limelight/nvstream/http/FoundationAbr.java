package com.limelight.nvstream.http;

import org.json.JSONException;
import org.json.JSONObject;

import java.io.IOException;

/**
 * Automatic bitrate on a Foundation Sunshine PC, which runs the controller itself
 * (rubylight protocol/client/src/http/foundation.rs and desktop stream/host_abr.rs): the stream
 * turns it on, reports its network every three seconds and shows the bitrate the PC chose, as
 * Moonlight V+ does. Request and response shapes follow Foundation's src/nvhttp/abr_api.cpp.
 * Rubylight PCs keep this client's own controller.
 */
public final class FoundationAbr {
    public static final long FEEDBACK_INTERVAL_MS = 3000;

    /** The bitrate in Kbps the PC last set. */
    private int currentKbps;
    private double frames;
    private double drops;
    private double decoded;
    private long windowStartMs;
    private boolean inFlight;

    /** The PC's controller is on from {@code nowMs}, at its initial bitrate (else the stream's). */
    public FoundationAbr(int initialKbps, int streamKbps, long nowMs) {
        currentKbps = initialKbps > 0 ? initialKbps : streamKbps;
        windowStartMs = nowMs;
    }

    public int currentKbps() {
        return currentKbps;
    }

    /** One second of video: its frame loss in percent and the frames decoded. */
    public void add(double lossPercent, double fps) {
        if (fps <= 0) return;
        double loss = Math.max(0, Math.min(100, lossPercent));
        // The frames that arrived are what is left of all that were sent.
        double sent = loss >= 100 ? fps : fps * 100 / (100 - loss);
        frames += sent;
        drops += sent - fps;
        decoded += fps;
    }

    /**
     * The report body for the window that ends at {@code nowMs}, once three seconds have passed
     * and the last report has been answered; null otherwise. Starts the next window.
     */
    public JSONObject due(long nowMs, double rttMs) {
        long elapsed = nowMs - windowStartMs;
        if (inFlight || elapsed < FEEDBACK_INTERVAL_MS) return null;
        double loss = frames == 0 ? 0 : Math.min(100.0, drops * 100.0 / frames);
        JSONObject feedback = new JSONObject();
        try {
            feedback.put("packetLoss", loss);
            feedback.put("rttMs", Math.max(0, rttMs));
            feedback.put("decodeFps", decoded * 1000.0 / elapsed);
            feedback.put("droppedFrames", (int) Math.min(Integer.MAX_VALUE, Math.round(drops)));
            feedback.put("currentBitrate", currentKbps);
        } catch (JSONException e) {
            throw new IllegalStateException(e);
        }
        frames = drops = decoded = 0;
        windowStartMs = nowMs;
        inFlight = true;
        return feedback;
    }

    /** The PC's answer to the last report (null when it failed): the bitrate it moved to, else 0. */
    public int answered(JSONObject action) {
        inFlight = false;
        int kbps = action == null ? 0 : action.optInt("newBitrate", 0);
        if (kbps <= 0 || kbps == currentKbps) return 0;
        currentKbps = kbps;
        return kbps;
    }

    /** {@code POST /api/abr} to turn it on as V+ does: balanced, from 0 up to {@code maxKbps}. */
    public static JSONObject enableRequest(int maxKbps) {
        try {
            return new JSONObject().put("enabled", true).put("mode", "balanced")
                    .put("minBitrate", 0).put("maxBitrate", maxKbps);
        } catch (JSONException e) {
            throw new IllegalStateException(e);
        }
    }

    public static JSONObject disableRequest() {
        try {
            return new JSONObject().put("enabled", false);
        } catch (JSONException e) {
            throw new IllegalStateException(e);
        }
    }

    /** Whether {@code GET /api/abr/capabilities} says the PC runs the controller. */
    public static boolean supported(String capabilities) throws IOException {
        try {
            return new JSONObject(capabilities).optBoolean("supported", false);
        } catch (JSONException e) {
            throw new IOException("Invalid ABR capabilities", e);
        }
    }

    /** The initial bitrate of a {@code POST /api/abr} answer; refused answers throw. */
    public static int initialBitrate(String configured) throws IOException {
        try {
            JSONObject json = new JSONObject(configured);
            if (!json.optBoolean("success", false)) {
                throw new IOException(json.optString("error", "ABR refused"));
            }
            return json.optInt("initialBitrate", 0);
        } catch (JSONException e) {
            throw new IOException("Invalid ABR answer", e);
        }
    }
}
