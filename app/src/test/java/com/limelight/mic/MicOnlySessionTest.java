package com.limelight.mic;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.Manifest;
import android.media.AudioManager;

import org.junit.Test;

/** Microphone only's phases: connecting, live, paused for a call and back, ended for good. */
public class MicOnlySessionTest {
    @Test
    public void itGoesLiveOnceConnected() {
        MicOnlySession session = new MicOnlySession();
        assertEquals(MicOnlySession.Phase.CONNECTING, session.phase());
        assertFalse(session.muted());
        assertTrue(session.connected());
        assertFalse(session.connected());
        assertEquals(MicOnlySession.Phase.LIVE, session.phase());
    }

    @Test
    public void aCallPausesTheMicrophoneAndItsEndResumesIt() {
        MicOnlySession session = new MicOnlySession();
        session.connected();
        assertTrue(session.call(true));
        assertFalse(session.call(true));
        assertTrue(session.muted());
        assertEquals(MicOnlySession.Phase.PAUSED_FOR_CALL, session.phase());
        assertTrue(session.call(false));
        assertFalse(session.muted());
        assertEquals(MicOnlySession.Phase.LIVE, session.phase());
    }

    @Test
    public void aCallWhileConnectingKeepsTheMicrophoneMutedOnceUp() {
        MicOnlySession session = new MicOnlySession();
        assertTrue(session.call(true));
        assertEquals(MicOnlySession.Phase.CONNECTING, session.phase());
        session.connected();
        assertTrue(session.muted());
        assertEquals(MicOnlySession.Phase.PAUSED_FOR_CALL, session.phase());
    }

    @Test
    public void anUnavailableMicrophoneSaysSoAndIsTriedAgainOutsideCalls() {
        MicOnlySession session = new MicOnlySession();
        session.connected();
        assertTrue(session.unavailable());
        assertFalse(session.unavailable());
        assertEquals(MicOnlySession.Phase.UNAVAILABLE, session.phase());
        assertTrue(session.retries());
        assertTrue(session.available());
        assertEquals(MicOnlySession.Phase.LIVE, session.phase());
        assertFalse(session.retries());
    }

    @Test
    public void aCallThatTookTheMicrophoneTriesItAgainWhenItEnds() {
        MicOnlySession session = new MicOnlySession();
        session.connected();
        session.call(true);
        // Paused already: the notification does not change.
        assertFalse(session.unavailable());
        assertFalse(session.retries());
        assertEquals(MicOnlySession.Phase.PAUSED_FOR_CALL, session.phase());
        assertTrue(session.call(false));
        assertEquals(MicOnlySession.Phase.LIVE, session.phase());
        assertFalse(session.retries());
    }

    @Test
    public void anEndedSessionStaysEnded() {
        MicOnlySession session = new MicOnlySession();
        session.connected();
        assertTrue(session.end());
        assertFalse(session.end());
        assertFalse(session.call(true));
        assertFalse(session.connected());
        assertEquals(MicOnlySession.Phase.ENDED, session.phase());
    }

    @Test
    public void callsAreCallsAndRingingIsNot() {
        assertTrue(MicOnlySession.isCall(AudioManager.MODE_IN_CALL));
        assertTrue(MicOnlySession.isCall(AudioManager.MODE_IN_COMMUNICATION));
        assertTrue(MicOnlySession.isCall(AudioManager.MODE_CALL_SCREENING));
        assertFalse(MicOnlySession.isCall(AudioManager.MODE_RINGTONE));
        assertFalse(MicOnlySession.isCall(AudioManager.MODE_NORMAL));
    }

    @Test
    public void itAsksForNotificationsOnlyWhereAndroidDoes() {
        assertArrayEquals(new String[] {Manifest.permission.RECORD_AUDIO}, MicOnlyActivity.wanted(32));
        assertArrayEquals(new String[] {Manifest.permission.RECORD_AUDIO, Manifest.permission.POST_NOTIFICATIONS},
                MicOnlyActivity.wanted(33));
    }
}
