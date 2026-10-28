package com.limelight;

import com.limelight.nvstream.ClientHintBitrateController;
import com.limelight.nvstream.RateChangeResult;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

import static com.limelight.nvstream.RateChangeResult.*;
import static org.junit.Assert.*;

public class StreamRateChangesTest {
    // What went out: {serial, kbps, mHz}.
    private final List<int[]> sent = new ArrayList<>();
    private boolean sendWorks = true;
    private final StreamRateChanges changes = new StreamRateChanges((serial, kbps, mhz) -> {
        if (!sendWorks) return false;
        sent.add(new int[] {serial, kbps, mhz});
        return true;
    });

    private static RateChangeResult bitrate(int serial, int kbps, int status, int reason) {
        return new RateChangeResult(serial, kbps, 0, status, reason, STATUS_UNCHANGED, REASON_NONE);
    }

    // Clean samples until automatic bitrate asks for more; its request.
    private int[] automaticRequest(long fromMs) {
        for (long now = fromMs; now < fromMs + 20_000; now += 1000) {
            int before = sent.size();
            assertTrue(changes.sampleAutomatic(now, true, 0, 0));
            if (sent.size() > before) return sent.get(sent.size() - 1);
        }
        throw new AssertionError("automatic bitrate never asked");
    }

    @Test
    public void aRequestFromTheMenuGoesOutWithItsOwnSerialAndComesBackAsManual() {
        assertTrue(changes.sendManual(30_000, 0));
        assertTrue(changes.sendManual(0, 120_000));
        int first = sent.get(0)[0];
        assertArrayEquals(new int[] {first, 30_000, 0}, sent.get(0));
        assertArrayEquals(new int[] {first + 1, 0, 120_000}, sent.get(1));
        assertEquals(StreamRateChanges.Answer.MANUAL, changes.onResult(bitrate(first, 30_000, STATUS_APPLIED, REASON_NONE), 0));
        assertEquals("a serial answers once", StreamRateChanges.Answer.UNKNOWN,
                changes.onResult(bitrate(first, 30_000, STATUS_APPLIED, REASON_NONE), 0));
        assertEquals(StreamRateChanges.Answer.UNKNOWN,
                changes.onResult(bitrate(first + 9, 30_000, STATUS_APPLIED, REASON_NONE), 0));
    }

    @Test
    public void aRequestThatCouldNotBeSentIsNotWaitedFor() {
        sendWorks = false;
        assertFalse(changes.sendManual(30_000, 0));
        sendWorks = true;
        changes.startAutomatic(20_000, 0);
        // Nothing outstanding, so automatic bitrate samples and asks.
        int[] request = automaticRequest(ClientHintBitrateController.START_DELAY_MS);
        assertEquals(1, sent.size());
        assertEquals(StreamRateChanges.Answer.AUTOMATIC,
                changes.onResult(bitrate(request[0], request[1], STATUS_APPLIED, REASON_NONE), 10_000));
    }

    @Test
    public void automaticBitrateFollowsTheHostsAnswer() {
        changes.startAutomatic(20_000, 0);
        int[] request = automaticRequest(ClientHintBitrateController.START_DELAY_MS);
        assertEquals(21_000, request[1]);
        assertEquals("only bitrate", 0, request[2]);
        assertTrue(changes.automatic().pending());
        assertEquals(StreamRateChanges.Answer.AUTOMATIC,
                changes.onResult(bitrate(request[0], 21_000, STATUS_APPLIED, REASON_NONE), 10_000));
        assertEquals(21_000, changes.automatic().currentKbps());

        request = automaticRequest(20_000);
        assertEquals(StreamRateChanges.Answer.AUTOMATIC,
                changes.onResult(bitrate(request[0], 21_500, STATUS_CLAMPED, REASON_HOST_LIMIT), 30_000));
        assertEquals("the host's cap is the ceiling", 21_500, changes.automatic().maxKbps());
    }

    @Test
    public void aRefusalStopsAutomaticBitrateForTheStream() {
        changes.startAutomatic(20_000, 0);
        int[] request = automaticRequest(ClientHintBitrateController.START_DELAY_MS);
        assertEquals(StreamRateChanges.Answer.AUTOMATIC_STOPPED,
                changes.onResult(bitrate(request[0], 20_000, STATUS_REFUSED, REASON_CODEC), 10_000));
        assertFalse(changes.automaticOn());
        int before = sent.size();
        assertTrue(changes.sampleAutomatic(60_000, true, 0, 0));
        assertEquals(before, sent.size());
    }

    @Test
    public void automaticBitrateWaitsForTheMenuAndStartsAgainFromItsBitrate() {
        changes.startAutomatic(20_000, 0);
        assertTrue(changes.sendManual(50_000, 0));
        for (long now = 0; now < 20_000; now += 1000) assertTrue(changes.sampleAutomatic(now, true, 0, 0));
        assertEquals("nothing while the menu's request is out", 1, sent.size());
        assertEquals(StreamRateChanges.Answer.MANUAL,
                changes.onResult(bitrate(sent.get(0)[0], 50_000, STATUS_APPLIED, REASON_NONE), 20_000));
        assertEquals(50_000, changes.automatic().currentKbps());
        assertEquals(100_000, changes.automatic().maxKbps());
    }

    @Test
    public void anAutomaticResultFromBeforeAManualBitrateIsStale() {
        changes.startAutomatic(20_000, 0);
        int[] request = automaticRequest(ClientHintBitrateController.START_DELAY_MS);
        assertTrue(changes.sendManual(50_000, 0));
        assertEquals(StreamRateChanges.Answer.MANUAL,
                changes.onResult(bitrate(request[0] + 1, 50_000, STATUS_APPLIED, REASON_NONE), 10_000));
        assertEquals(StreamRateChanges.Answer.STALE,
                changes.onResult(bitrate(request[0], 21_000, STATUS_REFUSED, REASON_SUPERSEDED), 10_000));
        assertTrue("a superseded request does not stop it", changes.automaticOn());
        assertEquals(50_000, changes.automatic().currentKbps());
    }

    @Test
    public void aSendFailureStopsAutomaticBitrate() {
        changes.startAutomatic(20_000, 0);
        sendWorks = false;
        boolean failed = false;
        for (long now = ClientHintBitrateController.START_DELAY_MS; now < 20_000 && !failed; now += 1000) {
            failed = !changes.sampleAutomatic(now, true, 0, 0);
        }
        assertTrue(failed);
        assertFalse(changes.automaticOn());
    }

    @Test
    public void aLateResultOfAnEarlierStreamMatchesNoRequestOfTheNext() {
        assertTrue(changes.sendManual(30_000, 0));
        int earlier = sent.get(0)[0];
        // The stream restarts before the host's answer is handled: a new stream, new requests.
        StreamRateChanges next = new StreamRateChanges((serial, kbps, mhz) -> {
            sent.add(new int[] {serial, kbps, mhz});
            return true;
        });
        assertTrue(next.sendManual(40_000, 0));
        next.startAutomatic(20_000, 0);
        assertNotEquals(earlier, sent.get(1)[0]);
        assertEquals(StreamRateChanges.Answer.UNKNOWN,
                next.onResult(bitrate(earlier, 30_000, STATUS_REFUSED, REASON_HOST_LIMIT), 1000));
        assertTrue("the late answer neither stops automatic bitrate", next.automaticOn());
        assertEquals("nor stands in for the menu's request", StreamRateChanges.Answer.MANUAL,
                next.onResult(bitrate(sent.get(1)[0], 40_000, STATUS_APPLIED, REASON_NONE), 1000));
    }
}
