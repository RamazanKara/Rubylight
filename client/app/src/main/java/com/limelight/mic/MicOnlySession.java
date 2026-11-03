package com.limelight.mic;

import android.media.AudioManager;

/**
 * Where a Microphone only session is: connecting to the PC, sending the microphone, paused while this device is in a
 * call, unavailable (another app took the microphone), or ended. A call that starts while connecting pauses the
 * microphone as soon as it would start. Once ended, nothing brings it back: a new session starts over. The service
 * ({@link MicOnlyService}) feeds it and shows it.
 */
public final class MicOnlySession {
    public enum Phase { CONNECTING, LIVE, PAUSED_FOR_CALL, UNAVAILABLE, ENDED }

    private boolean connected;
    private boolean inCall;
    private boolean unavailable;
    private boolean ended;

    public synchronized Phase phase() {
        if (ended) return Phase.ENDED;
        if (!connected) return Phase.CONNECTING;
        if (inCall) return Phase.PAUSED_FOR_CALL;
        return unavailable ? Phase.UNAVAILABLE : Phase.LIVE;
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
        // The call held the microphone; once it ends, the microphone is tried again.
        if (!active) unavailable = false;
        return true;
    }

    /** The microphone could not be opened or stopped working. Returns whether the phase changed. */
    public synchronized boolean unavailable() {
        if (ended || unavailable) return false;
        unavailable = true;
        return !inCall;
    }

    /** The microphone opened again after it was unavailable. Returns whether the phase changed. */
    public synchronized boolean available() {
        if (ended || !unavailable) return false;
        unavailable = false;
        return !inCall;
    }

    /** Whether to try opening the microphone again: it is unavailable, outside a call. */
    public synchronized boolean retries() {
        return !ended && connected && unavailable && !inCall;
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
