package com.limelight.mic;

import android.media.AudioManager;

/**
 * Where a Microphone only session is: connecting to the PC, sending the microphone, paused while this device is in a
 * call, or ended. A call that starts while connecting pauses the microphone as soon as it would start. Once ended,
 * nothing brings it back: a new session starts over. The service ({@link MicOnlyService}) feeds it and shows it.
 */
public final class MicOnlySession {
    public enum Phase { CONNECTING, LIVE, PAUSED_FOR_CALL, ENDED }

    private boolean connected;
    private boolean inCall;
    private boolean ended;

    public synchronized Phase phase() {
        if (ended) return Phase.ENDED;
        if (!connected) return Phase.CONNECTING;
        return inCall ? Phase.PAUSED_FOR_CALL : Phase.LIVE;
    }

    /** Whether the microphone should be muted now: during a call, so the PC never hears it. */
    public synchronized boolean muted() {
        return inCall;
    }

    /** The stream to the PC is up. Returns whether the phase changed. */
    public synchronized boolean connected() {
        if (ended || connected) return false;
        connected = true;
        return true;
    }

    /** A call started or ended on this device. Returns whether the microphone's mute changed. */
    public synchronized boolean call(boolean active) {
        if (ended || inCall == active) return false;
        inCall = active;
        return true;
    }

    /** The session ended (Stop, the PC went away or it failed). Returns false when it had already ended. */
    public synchronized boolean end() {
        if (ended) return false;
        ended = true;
        return true;
    }

    /**
     * Whether {@code mode} ({@link AudioManager#getMode()}) means a call: a phone call or a call in another app.
     * Ringing alone is not one; the microphone keeps going until the call is answered.
     */
    public static boolean isCall(int mode) {
        return mode == AudioManager.MODE_IN_CALL || mode == AudioManager.MODE_IN_COMMUNICATION
                || mode == AudioManager.MODE_CALL_SCREENING || mode == AudioManager.MODE_CALL_REDIRECT
                || mode == AudioManager.MODE_COMMUNICATION_REDIRECT;
    }
}
