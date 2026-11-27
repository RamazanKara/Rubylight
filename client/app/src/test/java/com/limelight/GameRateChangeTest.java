package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.EditText;

import androidx.appcompat.app.AlertDialog;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.nvstream.ClientHintBitrateController;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.RateChangeResult;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.shadows.ShadowToast;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static com.limelight.nvstream.RateChangeResult.*;
import static org.junit.Assert.*;

/**
 * Tests the stream menu's 0x553C rate-change dialog, host response handling, legacy HTTP bitrate
 * updates, link-lowered notices, automatic bitrate on this device, and the shared rate-change test
 * harness.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameRateChangeTest {
    /** Shared streaming {@link Game} rig with a recording 0x553C sender for rate-change tests. */
    static final class Harness {
        // What went out: {serial, kbps, mHz}.
        final List<int[]> sent = new ArrayList<>();
        // The queries for the stream's bitrate (bitrate and rate 0), apart from what sets one; the order of both.
        final List<int[]> queries = new ArrayList<>();
        final List<String> order = new ArrayList<>();
        // Queries the sender cannot send yet, as a control stream that cannot carry them.
        int unsentQueries;
        Game game;
        PreferenceConfiguration prefs;

        void streamingGame() {
            game = Robolectric.buildActivity(Game.class).get();
            game.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "rate-change-pc"));
            game.setTheme(R.style.StreamTheme);
            game.setContentView(R.layout.activity_game);
            prefs = new PreferenceConfiguration();
            prefs.bitrate = 50_000;
            ReflectionHelpers.setField(game, "prefConfig", prefs);
            NvApp app = new NvApp("Game", 73, false);
            ReflectionHelpers.setField(game, "app", app);
            ReflectionHelpers.setField(game, "foreground", true);
            ReflectionHelpers.setField(game, "connected", true);
            ReflectionHelpers.setField(game, "currentBitrate", 50_000);
            ReflectionHelpers.setField(game, "streamFps", 60);
            NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984,
                    "test", new StreamConfiguration.Builder().setApp(app).build(), null, null) {
                @Override public void stop() {}
            };
            ReflectionHelpers.setField(connection, "hostDetails", TestHosts.details("rubylight-serverinfo.xml"));
            ReflectionHelpers.setField(game, "conn", connection);
            useRenderer();
            ShadowDialog.reset();
            ShadowToast.reset();
        }

        StreamRateChanges recording() {
            return new StreamRateChanges((serial, kbps, mhz) -> {
                if (kbps == 0 && mhz == 0) {
                    if (unsentQueries > 0) {
                        unsentQueries--;
                        return false;
                    }
                    queries.add(new int[] {serial, kbps, mhz});
                    order.add("query");
                    return true;
                }
                sent.add(new int[] {serial, kbps, mhz});
                order.add("change");
                return true;
            });
        }

        // A stream start on a PC that answers the query at once with the stream's own bitrate, as one running Rubylight
        // from after 6.0.1 does for the launch's.
        void startStream(StreamRateChanges changes) {
            int asked = queries.size();
            startStreamAsking(changes);
            if (changes != null && queries.size() > asked) {
                answerQuery((int) ReflectionHelpers.getField(game, "streamStartBitrateKbps"));
            }
        }

        // A stream start whose query the test answers.
        void startStreamAsking(StreamRateChanges changes) {
            ReflectionHelpers.callInstanceMethod(game.adaptation, "startStreamBitrate",
                    ReflectionHelpers.ClassParameter.from(StreamRateChanges.class, changes));
        }

        // The PC's answer to the last query: the stream's bitrate as it counts it, nothing changed.
        void answerQuery(int kbps) {
            assertFalse("a query went out", queries.isEmpty());
            game.rateChange(new RateChangeResult(queries.get(queries.size() - 1)[0], kbps, 60_000, STATUS_UNCHANGED, REASON_NONE,
                    STATUS_REFUSED, REASON_INVALID));
            ShadowLooper.idleMainLooper();
        }

        AlertDialog openDialog() {
            ReflectionHelpers.callInstanceMethod(game.adaptation, "showRateChangeDialog");
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            assertNotNull(dialog);
            assertTrue(dialog.isShowing());
            return dialog;
        }

        // The dialog's fields in order: bitrate, then frame rate when it shows.
        static List<EditText> fields(AlertDialog dialog) {
            List<EditText> found = new ArrayList<>();
            collect(dialog.getWindow().getDecorView(), found);
            return found;
        }

        private static void collect(View view, List<EditText> found) {
            if (view instanceof EditText) found.add((EditText) view);
            if (view instanceof ViewGroup) {
                ViewGroup group = (ViewGroup) view;
                for (int i = 0; i < group.getChildCount(); i++) collect(group.getChildAt(i), found);
            }
        }

        static void send(AlertDialog dialog) {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            ShadowLooper.idleMainLooper();
        }

        void useRenderer() {
            ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
            MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(game, prefs, null, 0,
                    false, false, "", null) {
                @Override public boolean hasRecentVideoFrames(long nowMs) { return true; }
            };
            ReflectionHelpers.setField(game, "decoderRenderer", renderer);
        }

        // Seconds of stream time, one automatic bitrate sample each.
        static void runFor(int seconds) {
            for (int i = 0; i < seconds; i++) ShadowLooper.idleMainLooper(1, TimeUnit.SECONDS);
        }

        // The start delay, then clean samples until automatic bitrate asks for more.
        void runUntilAutomaticAsks() {
            runFor((int) (ClientHintBitrateController.START_DELAY_MS / 1000) +
                    ClientHintBitrateController.STABLE_SAMPLES + 2);
        }

        // The legacy bitrate goes out on its own thread; its answer comes back to the UI thread.
        void awaitHostAction() throws InterruptedException {
            for (int i = 0; i < 500; i++) {
                ShadowLooper.idleMainLooper();
                if (!(boolean) ReflectionHelpers.getField(game, "hostActionInProgress")) return;
                Thread.sleep(10);
            }
            throw new AssertionError("the bitrate request never finished");
        }

        // The bitrate dialog's Keep my bitrate.
        static com.google.android.material.materialswitch.MaterialSwitch keepSwitch(AlertDialog dialog) {
            List<View> found = new ArrayList<>();
            collectSwitches(dialog.getWindow().getDecorView(), found);
            assertEquals(1, found.size());
            return (com.google.android.material.materialswitch.MaterialSwitch) found.get(0);
        }

        private static void collectSwitches(View view, List<View> found) {
            if (view instanceof com.google.android.material.materialswitch.MaterialSwitch) found.add(view);
            if (view instanceof ViewGroup) {
                ViewGroup group = (ViewGroup) view;
                for (int i = 0; i < group.getChildCount(); i++) collectSwitches(group.getChildAt(i), found);
            }
        }

        android.content.SharedPreferences stored() {
            return android.preference.PreferenceManager.getDefaultSharedPreferences(game);
        }
    }

    private final Harness h = new Harness();

    @Before
    public void streamingGame() {
        h.streamingGame();
    }

    @Test
    public void theDialogSendsBitrateAndFrameRateInOneRequest() {
        h.startStream(h.recording());
        AlertDialog dialog = h.openDialog();
        List<EditText> fields = Harness.fields(dialog);
        assertEquals(2, fields.size());
        assertEquals("50", fields.get(0).getText().toString());
        assertEquals("60", fields.get(1).getText().toString());
        fields.get(0).setText("30,5");
        fields.get(1).setText("90");
        Harness.send(dialog);
        assertEquals(1, h.sent.size());
        assertEquals(30_500, h.sent.get(0)[1]);
        assertEquals(90_000, h.sent.get(0)[2]);
        assertFalse(dialog.isShowing());
    }

    @Test
    public void aFieldLeftAsItIsKeepsItsValueAndNothingChangedSendsNothing() {
        h.startStream(h.recording());
        AlertDialog dialog = h.openDialog();
        Harness.send(dialog);
        assertTrue("nothing to change", h.sent.isEmpty());
        assertTrue(dialog.isShowing());

        Harness.fields(dialog).get(1).setText("120");
        Harness.send(dialog);
        assertEquals(1, h.sent.size());
        assertEquals("the bitrate stays", 0, h.sent.get(0)[1]);
        assertEquals(120_000, h.sent.get(0)[2]);
    }

    @Test
    public void sendAfterTheStreamEndedClosesTheDialogAndSaysSo() {
        h.startStream(h.recording());
        AlertDialog dialog = h.openDialog();
        Harness.fields(dialog).get(0).setText("30");
        ReflectionHelpers.setField(h.game, "connected", false);
        Harness.send(dialog);
        assertTrue(h.sent.isEmpty());
        assertFalse(dialog.isShowing());
        assertEquals(h.game.getString(R.string.stream_rate_change_stream_ended), ShadowToast.getTextOfLatestToast());
    }

    @Test
    public void sendAfterTheTerminationArrivedClosesTheDialogAndSaysTheStreamEnded() {
        // The Retroid's R8: the host died with the dialog open. connectionTerminated marks the
        // stream terminated at once; connected stays true until stopConnection closes the dialog,
        // so this text never showed and "not sent" or "no answer" did instead.
        h.startStream(h.recording());
        AlertDialog dialog = h.openDialog();
        Harness.fields(dialog).get(0).setText("30");
        ReflectionHelpers.setField(h.game, "streamTerminated", true);
        assertTrue((boolean) ReflectionHelpers.getField(h.game, "connected"));
        Harness.send(dialog);
        assertTrue(h.sent.isEmpty());
        assertFalse(dialog.isShowing());
        assertEquals(h.game.getString(R.string.stream_rate_change_stream_ended), ShadowToast.getTextOfLatestToast());
    }

    @Test
    public void sendAfterTheStreamRestartedClosesTheDialogInsteadOfAskingTheNewStream() {
        h.startStream(h.recording());
        AlertDialog dialog = h.openDialog();
        Harness.fields(dialog).get(0).setText("30");
        // The stream restarted while the dialog was open: a new stream with its own rate changes.
        h.startStream(h.recording());
        Harness.send(dialog);
        assertTrue(h.sent.isEmpty());
        assertFalse(dialog.isShowing());
        assertEquals(h.game.getString(R.string.stream_rate_change_stream_ended), ShadowToast.getTextOfLatestToast());
    }

    @Test
    public void theHostsAnswerShowsWhatBecameOfEachValueRefusalsWithTheirReason() {
        h.startStream(h.recording());
        AlertDialog dialog = h.openDialog();
        List<EditText> fields = Harness.fields(dialog);
        fields.get(0).setText("30");
        fields.get(1).setText("120");
        Harness.send(dialog);
        int serial = h.sent.get(0)[0];

        h.game.rateChange(new RateChangeResult(serial, 30_000, 60_000, STATUS_APPLIED, REASON_NONE,
                STATUS_REFUSED, REASON_DISABLED));
        ShadowLooper.idleMainLooper();
        assertEquals("Bitrate: 30.0 Mbps\nFrame rate not changed, still 60 fps. The PC has these changes turned off.",
                ShadowToast.getTextOfLatestToast());
        assertEquals(30_000, (int) ReflectionHelpers.getField(h.game, "currentBitrate"));
        assertEquals("a refused rate keeps the stream's", 60, (int) ReflectionHelpers.getField(h.game, "streamFps"));

        // A result for no request of this stream (one from before a restart, say) says nothing.
        int toasts = ShadowToast.shownToastCount();
        h.game.rateChange(new RateChangeResult(serial, 10_000, 0, STATUS_APPLIED, REASON_NONE,
                STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertEquals(toasts, ShadowToast.shownToastCount());
        assertEquals(30_000, (int) ReflectionHelpers.getField(h.game, "currentBitrate"));
    }

    @Test
    public void anAnswerThatComesAfterALaterRequestsKeepsThatOnesBitrate() {
        h.startStream(h.recording());
        AlertDialog dialog = h.openDialog();
        List<EditText> fields = Harness.fields(dialog);
        fields.get(0).setText("30");
        fields.get(1).setText("120");
        Harness.send(dialog);
        dialog = h.openDialog();
        Harness.fields(dialog).get(0).setText("40");
        Harness.send(dialog);
        assertEquals(2, h.sent.size());

        // The host answers the bitrate-only request at once and the first once its switch ended,
        // here back at 60 fps, with the bitrate it set when that request came.
        h.game.rateChange(new RateChangeResult(h.sent.get(1)[0], 40_000, 60_000, STATUS_APPLIED, REASON_NONE,
                STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertEquals(40_000, (int) ReflectionHelpers.getField(h.game, "currentBitrate"));
        h.game.rateChange(new RateChangeResult(h.sent.get(0)[0], 30_000, 60_000, STATUS_APPLIED, REASON_NONE,
                STATUS_REFUSED, REASON_ENCODER_FAILED));
        ShadowLooper.idleMainLooper();
        assertEquals("Bitrate: 30.0 Mbps\nFrame rate not changed, still 60 fps. " +
                        h.game.getString(R.string.stream_rate_change_reason_encoder_failed),
                ShadowToast.getTextOfLatestToast());
        assertEquals("the later request's bitrate stays", 40_000,
                (int) ReflectionHelpers.getField(h.game, "currentBitrate"));
    }

    @Test
    public void aChangeTheHostNeverAnswersIsGivenUpOnLoudly() {
        h.startStream(h.recording());
        AlertDialog dialog = h.openDialog();
        Harness.fields(dialog).get(0).setText("30");
        Harness.send(dialog);
        Harness.runFor(4);
        assertNotEquals(h.game.getString(R.string.stream_rate_change_no_answer), ShadowToast.getTextOfLatestToast());
        Harness.runFor(1);
        assertEquals(h.game.getString(R.string.stream_rate_change_no_answer), ShadowToast.getTextOfLatestToast());

        // An answer after that is for no request any more.
        int toasts = ShadowToast.shownToastCount();
        h.game.rateChange(new RateChangeResult(h.sent.get(0)[0], 30_000, 60_000, STATUS_APPLIED, REASON_NONE,
                STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertEquals(toasts, ShadowToast.shownToastCount());
        assertEquals(50_000, (int) ReflectionHelpers.getField(h.game, "currentBitrate"));
    }

    @Test
    public void automaticBitrateStopsWhenTheHostNeverAnswersItsRequest() {
        h.prefs.rateChangeBitrate = true;
        h.startStream(h.recording());
        h.runUntilAutomaticAsks();
        assertEquals(1, h.sent.size());
        Harness.runFor((int) (StreamRateChanges.ANSWER_MS / 1000));
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(h.game.adaptation, "isRateChangeBitrateOn"));
        assertEquals(h.game.getString(R.string.stream_rate_change_auto_no_answer), ShadowToast.getTextOfLatestToast());
        Harness.runFor(20);
        assertEquals("nothing more goes out", 1, h.sent.size());
    }

    @Test
    public void anAppliedBitrateFromTheOpenMenusSliderShowsNoToastOverTheMenu() {
        // A real stream on the Retroid: "Bitrate: 32.0 Mbps" lay over the menu's Disconnect while the slider showed 32 already.
        h.startStream(h.recording());
        h.game.showStreamMenu();
        assertNotNull("the menu is open", ReflectionHelpers.getField(h.game.menuActions, "streamMenuSheet"));
        int toasts = ShadowToast.shownToastCount();
        assertEquals(32_000, h.game.adaptation.applyBitrateFromMenu(32_000));
        h.game.rateChange(new RateChangeResult(h.sent.get(0)[0], 32_000, 0, STATUS_APPLIED, REASON_NONE,
                STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertEquals("the slider shows it", toasts, ShadowToast.shownToastCount());
        assertEquals(32_000, (int) ReflectionHelpers.getField(h.game, "currentBitrate"));

        // A clamp is news the slider cannot show.
        h.game.adaptation.applyBitrateFromMenu(90_000);
        h.game.rateChange(new RateChangeResult(h.sent.get(1)[0], 80_000, 0, STATUS_CLAMPED, REASON_ENCODER_LIMIT,
                STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertEquals(toasts + 1, ShadowToast.shownToastCount());
    }

    /** A host without the stream's rate changes takes the menu's bitrate over HTTP: the same rule for its toast. */
    @Test
    public void anAppliedLegacyBitrateFromTheOpenMenuShowsNoToastOverTheMenu() throws Exception {
        int[] answer = {32_000};
        NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984,
                "test", new StreamConfiguration.Builder().setApp(new NvApp("Game", 73, false)).build(), null, null) {
            @Override public void stop() {}
            @Override public int setBitrate(int kbps) { return answer[0]; }
        };
        ReflectionHelpers.setField(connection, "hostDetails", TestHosts.details("rubylight-serverinfo.xml"));
        ReflectionHelpers.setField(h.game, "conn", connection);
        h.game.showStreamMenu();
        assertNotNull("the menu is open", ReflectionHelpers.getField(h.game.menuActions, "streamMenuSheet"));
        int toasts = ShadowToast.shownToastCount();
        assertEquals(32_000, h.game.adaptation.applyBitrateFromMenu(32_000));
        h.awaitHostAction();
        assertEquals("the slider shows it", toasts, ShadowToast.shownToastCount());
        assertEquals(32_000, (int) ReflectionHelpers.getField(h.game, "currentBitrate"));

        // A rate the host lowered is news the slider cannot show.
        answer[0] = 20_000;
        h.game.adaptation.applyBitrateFromMenu(90_000);
        h.awaitHostAction();
        assertEquals(toasts + 1, ShadowToast.shownToastCount());
    }

    @Test
    public void thePcLoweringTheBitrateIsToldOncePerStreamNotOverTheOverlayAndNeverWithNever() {
        assertTrue(GameAdaptation.tellLinkLowered(false, false, false));
        assertFalse("once per stream", GameAdaptation.tellLinkLowered(true, false, false));
        assertFalse("the overlay has its own line", GameAdaptation.tellLinkLowered(false, true, false));
        assertFalse("not with Never", GameAdaptation.tellLinkLowered(false, false, true));

        h.prefs.enablePerfOverlay = false;
        com.limelight.binding.video.LinkNoticeStatus status = com.limelight.binding.video.LinkNoticeStatus.parse(1, 50_000, 18_400);
        // A PC that took no rate changes: what happened, and no remedy that would not work.
        h.game.adaptation.noteLinkLowered(status);
        assertTrue(h.game.linkLoweredTold);
        String told = ShadowToast.getTextOfLatestToast();
        assertTrue(told, told.contains("18 Mbps") && !told.contains("Keep my bitrate") && !told.contains("Settings"));
        ShadowToast.reset();
        h.game.adaptation.noteLinkLowered(status);
        assertNull("not again", ShadowToast.getLatestToast());

        // A PC that took them: the stream menu's switch.
        h.startStream(h.recording());
        h.game.linkLoweredTold = false;
        h.game.adaptation.noteLinkLowered(status);
        told = ShadowToast.getTextOfLatestToast();
        assertTrue(told, told.contains("18 Mbps") && told.contains("Control Center") && told.contains("Keep my bitrate"));
        // Not while the PC keeps this stream's bitrate.
        ShadowToast.reset();
        h.game.linkLoweredTold = false;
        h.game.bitrateKept = true;
        h.game.adaptation.noteLinkLowered(status);
        assertNull(ShadowToast.getLatestToast());
    }

    @Test
    public void theNoticeStaysQuietWhileTheOverlayShows() {
        ReflectionHelpers.setField(h.game, "performanceOverlayScroll", h.game.findViewById(R.id.performanceOverlayScroll));
        h.game.performanceOverlayScroll.setVisibility(View.VISIBLE);
        h.game.adaptation.noteLinkLowered(com.limelight.binding.video.LinkNoticeStatus.parse(1, 50_000, 18_400));
        assertNull(ShadowToast.getLatestToast());
        assertFalse(h.game.linkLoweredTold);
        // The wiring from the PC's report: Game.linkNotice reaches it.
        h.game.performanceOverlayScroll.setVisibility(View.GONE);
        h.game.linkNotice(1, 50_000, 18_400);
        ShadowLooper.idleMainLooper();
        assertTrue(h.game.linkLoweredTold);
        assertNotNull(ShadowToast.getLatestToast());
    }

    @Test
    public void clampedValuesSayWhatLimitsThem() {
        RateChangeResult result = new RateChangeResult(1, 80_000, 90_000, STATUS_CLAMPED, REASON_ENCODER_LIMIT,
                STATUS_CLAMPED, REASON_HOST_LIMIT);
        assertEquals("Bitrate: 80.0 Mbps, lower than asked. That is the most the PC’s encoder manages at this resolution.\n" +
                        "Frame rate: 90 fps, lower than asked. That is the most the PC or this device allows.",
                Game.rateChangeText(h.game, result));
        assertEquals("Frame rate: 59.94 fps", Game.rateChangeText(h.game, new RateChangeResult(1, 30_000, 59_940,
                STATUS_UNCHANGED, REASON_NONE, STATUS_APPLIED, REASON_NONE)));
    }

    @Test
    public void withTheSettingOnAutomaticBitrateRunsOnThisDeviceAndSendsItsRequests() {
        h.prefs.rateChangeBitrate = true;
        h.startStream(h.recording());
        assertTrue((boolean) ReflectionHelpers.callInstanceMethod(h.game.adaptation, "isRateChangeBitrateOn"));
        assertNull("not the host's automatic bitrate as well", ReflectionHelpers.getField(h.game, "adaptiveBitrate"));
        h.runUntilAutomaticAsks();
        assertFalse("clean samples ask for more", h.sent.isEmpty());
        int[] request = h.sent.get(0);
        assertTrue(request[1] > 50_000);
        assertEquals("only the bitrate", 0, request[2]);

        // The host refuses it: automatic bitrate stops for this stream and says why.
        h.game.rateChange(new RateChangeResult(request[0], 50_000, 0, STATUS_REFUSED, REASON_CODEC,
                STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(h.game.adaptation, "isRateChangeBitrateOn"));
        assertEquals(h.game.getString(R.string.stream_rate_change_auto_stopped,
                h.game.getString(R.string.stream_rate_change_reason_codec)), ShadowToast.getTextOfLatestToast());
        int before = h.sent.size();
        Harness.runFor(10);
        assertEquals(before, h.sent.size());
    }

    @Test
    public void withTheSettingOffNothingIsSentOnItsOwn() {
        h.startStream(h.recording());
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(h.game.adaptation, "isRateChangeBitrateOn"));
        Harness.runFor(15);
        assertTrue(h.sent.isEmpty());
    }

    @Test
    public void turningItOnFromTheMenuExplainsItAsTheSettingDoes() {
        h.prefs.rateChangeBitrate = true;
        h.startStream(h.recording());
        ReflectionHelpers.callInstanceMethod(h.game.adaptation, "toggleAdaptiveBitrateFromMenu");
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(h.game.adaptation, "isRateChangeBitrateOn"));
        ReflectionHelpers.callInstanceMethod(h.game.adaptation, "toggleAdaptiveBitrateFromMenu");
        assertTrue((boolean) ReflectionHelpers.callInstanceMethod(h.game.adaptation, "isRateChangeBitrateOn"));
        assertEquals(h.game.getString(R.string.summary_rate_change_bitrate), ShadowToast.getTextOfLatestToast());
        h.runUntilAutomaticAsks();
        assertFalse(h.sent.isEmpty());
    }

    @Test
    public void theDoctorOffersNoBitrateTrialWhileAutomaticBitrateOnThisDeviceRuns() {
        assertTrue(Game.healthTrialAllowed(false, false, false));
        assertFalse("automatic bitrate on this device owns the bitrate", Game.healthTrialAllowed(false, true, false));
        assertFalse(Game.healthTrialAllowed(true, false, false));
        assertFalse(Game.healthTrialAllowed(false, false, true));
    }
}
