package com.limelight;

import com.limelight.nvstream.ClientHintBitrateController;
import com.limelight.nvstream.RateChangeResult;
import com.limelight.preferences.StreamLayers;

import org.junit.Test;

import java.util.ArrayList;
import java.util.EnumSet;
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
        assertTrue(changes.sendManual(30_000, 0, 0));
        assertTrue(changes.sendManual(0, 120_000, 0));
        int first = sent.get(0)[0];
        assertArrayEquals(new int[] {first, 30_000, 0}, sent.get(0));
        assertArrayEquals(new int[] {first + 1, 0, 120_000}, sent.get(1));
        assertEquals(StreamRateChanges.Answer.MANUAL,
                changes.onResult(bitrate(first, 30_000, STATUS_APPLIED, REASON_NONE), 0).answer);
        assertEquals("a serial answers once", StreamRateChanges.Answer.UNKNOWN,
                changes.onResult(bitrate(first, 30_000, STATUS_APPLIED, REASON_NONE), 0).answer);
        assertEquals(StreamRateChanges.Answer.UNKNOWN,
                changes.onResult(bitrate(first + 9, 30_000, STATUS_APPLIED, REASON_NONE), 0).answer);
    }

    @Test
    public void aRequestThatCouldNotBeSentIsNotWaitedFor() {
        sendWorks = false;
        assertFalse(changes.sendManual(30_000, 0, 0));
        sendWorks = true;
        changes.startAutomatic(20_000, 0);
        // Nothing outstanding, so automatic bitrate samples and asks.
        int[] request = automaticRequest(ClientHintBitrateController.START_DELAY_MS);
        assertEquals(1, sent.size());
        assertEquals(StreamRateChanges.Answer.AUTOMATIC,
                changes.onResult(bitrate(request[0], request[1], STATUS_APPLIED, REASON_NONE), 10_000).answer);
    }

    @Test
    public void automaticBitrateFollowsTheHostsAnswer() {
        changes.startAutomatic(20_000, 0);
        int[] request = automaticRequest(ClientHintBitrateController.START_DELAY_MS);
        assertEquals(21_000, request[1]);
        assertEquals("only bitrate", 0, request[2]);
        assertTrue(changes.automatic().pending());
        assertEquals(StreamRateChanges.Answer.AUTOMATIC,
                changes.onResult(bitrate(request[0], 21_000, STATUS_APPLIED, REASON_NONE), 10_000).answer);
        assertEquals(21_000, changes.automatic().currentKbps());

        request = automaticRequest(20_000);
        assertEquals(StreamRateChanges.Answer.AUTOMATIC,
                changes.onResult(bitrate(request[0], 21_500, STATUS_CLAMPED, REASON_HOST_LIMIT), 30_000).answer);
        assertEquals("the host's cap is the ceiling", 21_500, changes.automatic().maxKbps());
    }

    @Test
    public void aRefusalStopsAutomaticBitrateForTheStream() {
        changes.startAutomatic(20_000, 0);
        int[] request = automaticRequest(ClientHintBitrateController.START_DELAY_MS);
        assertEquals(StreamRateChanges.Answer.AUTOMATIC_STOPPED,
                changes.onResult(bitrate(request[0], 20_000, STATUS_REFUSED, REASON_CODEC), 10_000).answer);
        assertFalse(changes.automaticOn());
        int before = sent.size();
        assertTrue(changes.sampleAutomatic(60_000, true, 0, 0));
        assertEquals(before, sent.size());
    }

    @Test
    public void automaticBitrateWaitsForTheMenuAndStartsAgainFromItsBitrate() {
        changes.startAutomatic(20_000, 0);
        assertTrue(changes.sendManual(50_000, 0, 0));
        for (long now = 0; now < 20_000; now += 1000) assertTrue(changes.sampleAutomatic(now, true, 0, 0));
        assertEquals("nothing while the menu's request is out", 1, sent.size());
        assertEquals(StreamRateChanges.Answer.MANUAL,
                changes.onResult(bitrate(sent.get(0)[0], 50_000, STATUS_APPLIED, REASON_NONE), 20_000).answer);
        assertEquals(50_000, changes.automatic().currentKbps());
        assertEquals(100_000, changes.automatic().maxKbps());
    }

    @Test
    public void aManualBitrateAfterAnAutomaticRequestIsAnsweredAfterIt() {
        // The host answers bitrate-only requests as they come, so automatic bitrate's comes first
        // and the bitrate set by hand is the one that stays.
        changes.startAutomatic(20_000, 0);
        int[] request = automaticRequest(ClientHintBitrateController.START_DELAY_MS);
        assertTrue(changes.sendManual(50_000, 0, 10_000));
        int manual = sent.get(sent.size() - 1)[0];
        StreamRateChanges.Outcome automatic =
                changes.onResult(bitrate(request[0], 21_000, STATUS_APPLIED, REASON_NONE), 10_000);
        assertEquals(StreamRateChanges.Answer.AUTOMATIC, automatic.answer);
        assertTrue(automatic.takesBitrate);
        StreamRateChanges.Outcome byHand = changes.onResult(bitrate(manual, 50_000, STATUS_APPLIED, REASON_NONE), 10_100);
        assertEquals(StreamRateChanges.Answer.MANUAL, byHand.answer);
        assertTrue(byHand.takesBitrate);
        assertTrue(changes.automaticOn());
        assertEquals("automatic bitrate starts again from it", 50_000, changes.automatic().currentKbps());
    }

    @Test
    public void anAutomaticResultFromBeforeItStartedAgainIsStale() {
        changes.startAutomatic(20_000, 0);
        int[] request = automaticRequest(ClientHintBitrateController.START_DELAY_MS);
        // Turned off and on again from the stream menu while its request was out.
        changes.stopAutomatic();
        changes.startAutomatic(30_000, 9_000);
        StreamRateChanges.Outcome late = changes.onResult(bitrate(request[0], 21_000, STATUS_APPLIED, REASON_NONE), 10_000);
        assertEquals(StreamRateChanges.Answer.STALE, late.answer);
        assertTrue("the host set it, and nothing later did", late.takesBitrate);
        assertTrue(changes.automaticOn());
        assertEquals("the new run keeps its own start", 30_000, changes.automatic().currentKbps());
        assertFalse(changes.automatic().pending());
    }

    // R8: from 28000 kbps the range reaches twice the start by design (V+'s balanced range, as on
    // desktop); on a mobile network the mobile data limit caps it, as desktop's bitrate_limit does.
    @Test
    public void theMobileDataLimitCapsAutomaticBitrateOnlyOnAMobileNetwork() {
        assertEquals(25_000, StreamRateChanges.clientLimitKbps(
                StreamLayers.Network.MOBILE, 25_000));
        assertEquals("no limit set", 0, StreamRateChanges.clientLimitKbps(
                StreamLayers.Network.MOBILE, 0));
        for (StreamLayers.Network other : new StreamLayers.Network[] {
                StreamLayers.Network.HOME_WIFI, StreamLayers.Network.WIFI,
                StreamLayers.Network.OTHER, null}) {
            assertEquals(String.valueOf(other), 0, StreamRateChanges.clientLimitKbps(other, 25_000));
        }

        changes.startAutomatic(28_000, 0);
        assertEquals("twice the start without a cap", 56_000, changes.automatic().maxKbps());
        changes.startAutomatic(20_000, StreamRateChanges.clientLimitKbps(
                StreamLayers.Network.MOBILE, 25_000), 0);
        assertEquals(25_000, changes.automatic().maxKbps());
        // Clean seconds probe up to the cap and never past it.
        long now = ClientHintBitrateController.START_DELAY_MS;
        for (int i = 0; i < 20; i++, now += 10_000) {
            int[] request = automaticRequest(now);
            assertTrue(request[1] <= 25_000);
            changes.onResult(bitrate(request[0], request[1], STATUS_APPLIED, REASON_NONE), now);
            if (request[1] == 25_000) break;
        }
        assertEquals(25_000, changes.automatic().currentKbps());
        int before = sent.size();
        for (long t = now + 10_000; t < now + 40_000; t += 1000) assertTrue(changes.sampleAutomatic(t, true, 0, 0));
        assertEquals("no probe past the cap", before, sent.size());
    }

    @Test
    public void anAnswerThatComesAfterALaterRequestsKeepsThatOnesBitrate() {
        // The host holds the answer to a request with a rate until its switch ran, with the bitrate
        // it set then; a bitrate-only request sent after it is answered first.
        changes.startAutomatic(20_000, 0);
        assertTrue(changes.sendManual(30_000, 120_000, 0));
        assertTrue(changes.sendManual(40_000, 0, 100));
        int first = sent.get(0)[0];
        int second = sent.get(1)[0];
        StreamRateChanges.Outcome newer = changes.onResult(bitrate(second, 40_000, STATUS_APPLIED, REASON_NONE), 200);
        assertEquals(StreamRateChanges.Answer.MANUAL, newer.answer);
        assertTrue(newer.takesBitrate);
        StreamRateChanges.Outcome older = changes.onResult(new RateChangeResult(first, 30_000, 120_000,
                STATUS_APPLIED, REASON_NONE, STATUS_APPLIED, REASON_NONE), 900);
        assertEquals("still said, as the menu asked", StreamRateChanges.Answer.MANUAL, older.answer);
        assertFalse("its bitrate is older than the stream's", older.takesBitrate);
        assertEquals(40_000, changes.automatic().currentKbps());
        assertEquals("nothing is waited for any more", 0, changes.nextExpiryMs());
    }

    @Test
    public void aMenuRequestTheHostNeverAnswersIsGivenUpOnLoudly() {
        assertEquals(0, changes.nextExpiryMs());
        assertTrue(changes.sendManual(30_000, 0, 1000));
        int serial = sent.get(0)[0];
        assertEquals(1000 + StreamRateChanges.ANSWER_MS, changes.nextExpiryMs());
        assertTrue(changes.expire(5999).isEmpty());
        assertEquals(EnumSet.of(StreamRateChanges.Answer.MANUAL_NO_ANSWER), changes.expire(6000));
        assertEquals(0, changes.nextExpiryMs());
        assertEquals("too late", StreamRateChanges.Answer.UNKNOWN,
                changes.onResult(bitrate(serial, 30_000, STATUS_APPLIED, REASON_NONE), 7000).answer);
        // Automatic bitrate no longer waits for it.
        changes.startAutomatic(20_000, 7000);
        automaticRequest(7000 + ClientHintBitrateController.START_DELAY_MS);
    }

    @Test
    public void anAutomaticRequestTheHostNeverAnswersStopsAutomaticBitrate() {
        changes.startAutomatic(20_000, 0);
        int[] request = automaticRequest(ClientHintBitrateController.START_DELAY_MS);
        long sentMs = changes.nextExpiryMs() - StreamRateChanges.ANSWER_MS;
        assertTrue(changes.expire(sentMs + StreamRateChanges.ANSWER_MS - 1).isEmpty());
        assertTrue(changes.automaticOn());
        assertEquals(EnumSet.of(StreamRateChanges.Answer.AUTOMATIC_NO_ANSWER),
                changes.expire(sentMs + StreamRateChanges.ANSWER_MS));
        assertFalse(changes.automaticOn());
        assertEquals(StreamRateChanges.Answer.UNKNOWN,
                changes.onResult(bitrate(request[0], request[1], STATUS_APPLIED, REASON_NONE), sentMs + 6000).answer);

        // One from before automatic bitrate started again stops nothing.
        changes.startAutomatic(20_000, 0);
        automaticRequest(ClientHintBitrateController.START_DELAY_MS);
        changes.startAutomatic(20_000, 0);
        assertTrue(changes.expire(60_000).isEmpty());
        assertTrue(changes.automaticOn());
        assertEquals(0, changes.nextExpiryMs());
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
        assertTrue(changes.sendManual(30_000, 0, 0));
        int earlier = sent.get(0)[0];
        // The stream restarts before the host's answer is handled: a new stream, new requests.
        StreamRateChanges next = new StreamRateChanges((serial, kbps, mhz) -> {
            sent.add(new int[] {serial, kbps, mhz});
            return true;
        });
        assertTrue(next.sendManual(40_000, 0, 0));
        next.startAutomatic(20_000, 0);
        assertNotEquals(earlier, sent.get(1)[0]);
        assertEquals(StreamRateChanges.Answer.UNKNOWN,
                next.onResult(bitrate(earlier, 30_000, STATUS_REFUSED, REASON_HOST_LIMIT), 1000).answer);
        assertTrue("the late answer neither stops automatic bitrate", next.automaticOn());
        assertEquals("nor stands in for the menu's request", StreamRateChanges.Answer.MANUAL,
                next.onResult(bitrate(sent.get(1)[0], 40_000, STATUS_APPLIED, REASON_NONE), 1000).answer);
    }
}
