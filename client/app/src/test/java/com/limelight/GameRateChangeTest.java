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

// The stream menu's 0x553C dialog, what Game says about the host's answers, and automatic
// bitrate on this device, with the bridge replaced by a recording sender.
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameRateChangeTest {
    // What went out: {serial, kbps, mHz}.
    private final List<int[]> sent = new ArrayList<>();
    private Game game;
    private PreferenceConfiguration prefs;

    @Before
    public void streamingGame() {
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

    private StreamRateChanges recording() {
        return new StreamRateChanges((serial, kbps, mhz) -> {
            sent.add(new int[] {serial, kbps, mhz});
            return true;
        });
    }

    private void startStream(StreamRateChanges changes) {
        ReflectionHelpers.callInstanceMethod(game, "startStreamBitrate",
                ReflectionHelpers.ClassParameter.from(StreamRateChanges.class, changes));
    }

    private AlertDialog openDialog() {
        ReflectionHelpers.callInstanceMethod(game, "showRateChangeDialog");
        AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
        assertNotNull(dialog);
        assertTrue(dialog.isShowing());
        return dialog;
    }

    // The dialog's fields in order: bitrate, then frame rate when it shows.
    private static List<EditText> fields(AlertDialog dialog) {
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

    private static void send(AlertDialog dialog) {
        dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
        ShadowLooper.idleMainLooper();
    }

    private void useRenderer() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(game, prefs, null, 0,
                false, false, "", null) {
            @Override public boolean hasRecentVideoFrames(long nowMs) { return true; }
        };
        ReflectionHelpers.setField(game, "decoderRenderer", renderer);
    }

    // Seconds of stream time, one automatic bitrate sample each.
    private static void runFor(int seconds) {
        for (int i = 0; i < seconds; i++) ShadowLooper.idleMainLooper(1, TimeUnit.SECONDS);
    }

    // The start delay, then clean samples until automatic bitrate asks for more.
    private void runUntilAutomaticAsks() {
        runFor((int) (ClientHintBitrateController.START_DELAY_MS / 1000) +
                ClientHintBitrateController.STABLE_SAMPLES + 2);
    }

    @Test
    public void theDialogSendsBitrateAndFrameRateInOneRequest() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        List<EditText> fields = fields(dialog);
        assertEquals(2, fields.size());
        assertEquals("50", fields.get(0).getText().toString());
        assertEquals("60", fields.get(1).getText().toString());
        fields.get(0).setText("30,5");
        fields.get(1).setText("90");
        send(dialog);
        assertEquals(1, sent.size());
        assertEquals(30_500, sent.get(0)[1]);
        assertEquals(90_000, sent.get(0)[2]);
        assertFalse(dialog.isShowing());
    }

    @Test
    public void aFieldLeftAsItIsKeepsItsValueAndNothingChangedSendsNothing() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        send(dialog);
        assertTrue("nothing to change", sent.isEmpty());
        assertTrue(dialog.isShowing());

        fields(dialog).get(1).setText("120");
        send(dialog);
        assertEquals(1, sent.size());
        assertEquals("the bitrate stays", 0, sent.get(0)[1]);
        assertEquals(120_000, sent.get(0)[2]);
    }

    @Test
    public void sendAfterTheStreamEndedClosesTheDialogAndSaysSo() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        fields(dialog).get(0).setText("30");
        ReflectionHelpers.setField(game, "connected", false);
        send(dialog);
        assertTrue(sent.isEmpty());
        assertFalse(dialog.isShowing());
        assertEquals(game.getString(R.string.stream_rate_change_stream_ended), ShadowToast.getTextOfLatestToast());
    }

    @Test
    public void sendAfterTheStreamRestartedClosesTheDialogInsteadOfAskingTheNewStream() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        fields(dialog).get(0).setText("30");
        // The stream restarted while the dialog was open: a new stream with its own rate changes.
        startStream(recording());
        send(dialog);
        assertTrue(sent.isEmpty());
        assertFalse(dialog.isShowing());
        assertEquals(game.getString(R.string.stream_rate_change_stream_ended), ShadowToast.getTextOfLatestToast());
    }

    @Test
    public void theHostsAnswerShowsWhatBecameOfEachValueRefusalsWithTheirReason() {
        startStream(recording());
        AlertDialog dialog = openDialog();
        List<EditText> fields = fields(dialog);
        fields.get(0).setText("30");
        fields.get(1).setText("120");
        send(dialog);
        int serial = sent.get(0)[0];

        game.rateChange(new RateChangeResult(serial, 30_000, 60_000, STATUS_APPLIED, REASON_NONE,
                STATUS_REFUSED, REASON_DISABLED));
        ShadowLooper.idleMainLooper();
        assertEquals("Bitrate: 30.0 Mbps\nFrame rate not changed, still 60 fps. The host has these changes turned off.",
                ShadowToast.getTextOfLatestToast());
        assertEquals(30_000, (int) ReflectionHelpers.getField(game, "currentBitrate"));
        assertEquals("a refused rate keeps the stream's", 60, (int) ReflectionHelpers.getField(game, "streamFps"));

        // A result for no request of this stream (one from before a restart, say) says nothing.
        int toasts = ShadowToast.shownToastCount();
        game.rateChange(new RateChangeResult(serial, 10_000, 0, STATUS_APPLIED, REASON_NONE,
                STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertEquals(toasts, ShadowToast.shownToastCount());
        assertEquals(30_000, (int) ReflectionHelpers.getField(game, "currentBitrate"));
    }

    @Test
    public void clampedValuesSayWhatLimitsThem() {
        RateChangeResult result = new RateChangeResult(1, 80_000, 90_000, STATUS_CLAMPED, REASON_ENCODER_LIMIT,
                STATUS_CLAMPED, REASON_HOST_LIMIT);
        assertEquals("Bitrate: 80.0 Mbps, lower than asked. That is the most the host's encoder manages at this resolution.\n" +
                        "Frame rate: 90 fps, lower than asked. That is the most the host or this device allows.",
                Game.rateChangeText(game, result));
        assertEquals("Frame rate: 59.94 fps", Game.rateChangeText(game, new RateChangeResult(1, 30_000, 59_940,
                STATUS_UNCHANGED, REASON_NONE, STATUS_APPLIED, REASON_NONE)));
    }

    @Test
    public void withTheSettingOnAutomaticBitrateRunsOnThisDeviceAndSendsItsRequests() {
        prefs.rateChangeBitrate = true;
        startStream(recording());
        assertTrue((boolean) ReflectionHelpers.callInstanceMethod(game, "isRateChangeBitrateOn"));
        assertNull("not the host's automatic bitrate as well", ReflectionHelpers.getField(game, "adaptiveBitrate"));
        runUntilAutomaticAsks();
        assertFalse("clean samples ask for more", sent.isEmpty());
        int[] request = sent.get(0);
        assertTrue(request[1] > 50_000);
        assertEquals("only the bitrate", 0, request[2]);

        // The host refuses it: automatic bitrate stops for this stream and says why.
        game.rateChange(new RateChangeResult(request[0], 50_000, 0, STATUS_REFUSED, REASON_CODEC,
                STATUS_UNCHANGED, REASON_NONE));
        ShadowLooper.idleMainLooper();
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(game, "isRateChangeBitrateOn"));
        assertEquals(game.getString(R.string.stream_rate_change_auto_stopped,
                game.getString(R.string.stream_rate_change_reason_codec)), ShadowToast.getTextOfLatestToast());
        int before = sent.size();
        runFor(10);
        assertEquals(before, sent.size());
    }

    @Test
    public void withTheSettingOffNothingIsSentOnItsOwn() {
        startStream(recording());
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(game, "isRateChangeBitrateOn"));
        runFor(15);
        assertTrue(sent.isEmpty());
    }

    @Test
    public void turningItOnFromTheMenuExplainsItAsTheSettingDoes() {
        prefs.rateChangeBitrate = true;
        startStream(recording());
        ReflectionHelpers.callInstanceMethod(game, "toggleAdaptiveBitrateFromMenu");
        assertFalse((boolean) ReflectionHelpers.callInstanceMethod(game, "isRateChangeBitrateOn"));
        ReflectionHelpers.callInstanceMethod(game, "toggleAdaptiveBitrateFromMenu");
        assertTrue((boolean) ReflectionHelpers.callInstanceMethod(game, "isRateChangeBitrateOn"));
        assertEquals(game.getString(R.string.summary_rate_change_bitrate), ShadowToast.getTextOfLatestToast());
        runUntilAutomaticAsks();
        assertFalse(sent.isEmpty());
    }
}
