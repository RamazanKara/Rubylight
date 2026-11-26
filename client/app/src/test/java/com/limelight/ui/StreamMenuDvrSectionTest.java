package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.view.View;
import android.view.ViewGroup;

import androidx.test.core.app.ApplicationProvider;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

/**
 * The stream menu's Instant Replay group: offered only with the host's Stream DVR feature and 0x554A on the stream's own
 * device, rows that follow the host's state, the requests they send, and the words of its results and reasons.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w960dp-h540dp-land-xhdpi")
public class StreamMenuDvrSectionTest {
    private final Context context = ApplicationProvider.getApplicationContext();
    private final List<int[]> sent = new ArrayList<>();
    private final List<String> toasts = new ArrayList<>();
    private long now = 1_000;
    private boolean sends = true;

    private StreamMenuDvrSection section() {
        return new StreamMenuDvrSection(context, (id, op, secs) -> { sent.add(new int[] {id, op, secs}); return sends; },
                text -> toasts.add(text.toString()), () -> now);
    }

    @Test
    public void onlyTheStreamsOwnDeviceOnAHostWithTheFeatureAndTheMessageGetsIt() {
        assertTrue(StreamMenuDvrSection.offered(true, true, NvApp.Role.STREAM));
        assertFalse("not announced", StreamMenuDvrSection.offered(true, false, NvApp.Role.STREAM));
        assertFalse("no feature", StreamMenuDvrSection.offered(false, true, NvApp.Role.STREAM));
        assertFalse(StreamMenuDvrSection.offered(true, true, NvApp.Role.REMOTE_MONITOR));
        assertFalse(StreamMenuDvrSection.offered(true, true, NvApp.Role.INPUT_ONLY));
    }

    @Test
    public void theRowsFollowTheHostState() {
        StreamMenuDvrSection dvr = section();
        dvr.start();
        assertArrayEquals(new int[] {1, StreamMenuDvrSection.OP_QUERY, 0}, sent.get(0));
        StreamMenuDvrSection.Row[] rows = dvr.rows();
        assertTrue("both work before the first state", rows[0].enabled && rows[1].enabled);
        assertNull(rows[0].detail);

        dvr.onState(1, StreamMenuDvrSection.ARMED, StreamMenuDvrSection.NONE, 60, 0);
        rows = dvr.rows();
        assertEquals("Save clip", rows[0].label);
        assertEquals("Last 60 s ready", rows[0].detail);
        assertEquals(StreamMenuDvrSection.OP_START, rows[1].op);
        assertEquals("Start recording", rows[1].label);

        dvr.onState(0, StreamMenuDvrSection.RECORDING, StreamMenuDvrSection.NONE, 60, 134);
        rows = dvr.rows();
        assertEquals(StreamMenuDvrSection.OP_STOP, rows[1].op);
        assertEquals("Stop recording", rows[1].label);
        assertEquals("Recording 02:14", rows[1].detail);

        dvr.onState(0, StreamMenuDvrSection.UNSUPPORTED, StreamMenuDvrSection.PYROWAVE, 0, 0);
        rows = dvr.rows();
        assertFalse(rows[0].enabled || rows[1].enabled);
        assertTrue(rows[0].problem);
        assertEquals(context.getString(R.string.stream_dvr_reason_pyrowave), rows[0].detail);
        assertTrue("no toast for a state nobody asked for", toasts.isEmpty());

        // A state a newer host added greys the rows out; an unknown reason reads as the plain one.
        dvr.onState(0, 42, 77, 60, 0);
        rows = dvr.rows();
        assertFalse(rows[0].enabled || rows[1].enabled);
        assertEquals(context.getString(R.string.stream_dvr_reason_unknown), rows[0].detail);
    }

    @Test
    public void aRefusedStartAndAStopWithNothingRecordingComeAsClipResults() {
        StreamMenuDvrSection dvr = section();
        dvr.onState(0, StreamMenuDvrSection.ARMED, StreamMenuDvrSection.NONE, 60, 0);
        dvr.request(StreamMenuDvrSection.OP_START);
        dvr.onClip(sent.get(0)[0], 1, StreamMenuDvrSection.PERMISSION_DENIED, 0, 0);
        assertEquals(context.getString(R.string.stream_dvr_reason_permission), toasts.get(0));
        assertTrue("the row works again", dvr.rows()[1].enabled);
        dvr.request(StreamMenuDvrSection.OP_STOP);
        dvr.onClip(sent.get(1)[0], 1, StreamMenuDvrSection.NO_KEYFRAME_YET, 0, 0);
        assertEquals(context.getString(R.string.stream_dvr_reason_no_keyframe), toasts.get(1));
    }

    @Test
    public void aSaveWaitsForItsResultAndToastsIt() {
        StreamMenuDvrSection dvr = section();
        dvr.onState(0, StreamMenuDvrSection.ARMED, StreamMenuDvrSection.NONE, 60, 0);
        dvr.request(StreamMenuDvrSection.OP_SAVE);
        int id = sent.get(0)[0];
        assertEquals(StreamMenuDvrSection.OP_SAVE, sent.get(0)[1]);
        assertFalse(dvr.rows()[0].enabled);
        assertEquals("Saving clip…", dvr.rows()[0].detail);
        dvr.onClip(id, StreamMenuDvrSection.SAVED, StreamMenuDvrSection.NONE, 60, 84_000_000L);
        assertEquals("Clip saved (60 s, 84 MB)", toasts.get(0));
        assertTrue(dvr.rows()[0].enabled);

        // Unanswered, the row frees itself after a while.
        dvr.request(StreamMenuDvrSection.OP_SAVE);
        assertFalse(dvr.rows()[0].enabled);
        now += StreamMenuDvrSection.ANSWER_MS;
        assertTrue(dvr.rows()[0].enabled);

        sends = false;
        dvr.request(StreamMenuDvrSection.OP_SAVE);
        assertEquals(context.getString(R.string.stream_dvr_reason_unknown), toasts.get(1));
        assertTrue(dvr.rows()[0].enabled);
    }

    @Test
    public void aRecordingStartsWithAStateAndStopsWithAClip() {
        StreamMenuDvrSection dvr = section();
        dvr.request(StreamMenuDvrSection.OP_START);
        assertFalse(dvr.rows()[1].enabled);
        dvr.onState(sent.get(0)[0], StreamMenuDvrSection.RECORDING, StreamMenuDvrSection.NONE, 60, 0);
        assertTrue(toasts.isEmpty());
        dvr.request(StreamMenuDvrSection.OP_STOP);
        dvr.onClip(sent.get(1)[0], StreamMenuDvrSection.SAVED, StreamMenuDvrSection.NONE, 134, 1_250_000_000L);
        assertEquals("Recording saved (02:14, 1.2 GB)", toasts.get(0));

        dvr.request(StreamMenuDvrSection.OP_START);
        dvr.onState(sent.get(2)[0], StreamMenuDvrSection.ERROR, StreamMenuDvrSection.DISK_FULL, 0, 0);
        assertEquals(context.getString(R.string.stream_dvr_reason_disk), toasts.get(1));
    }

    @Test
    public void everyReasonReadsPlainly() {
        assertEquals(context.getString(R.string.stream_dvr_reason_disk),
                StreamMenuDvrSection.reasonText(context, StreamMenuDvrSection.QUOTA_EXCEEDED));
        assertEquals("Instant Replay is turned off on the host PC. Turn on Stream DVR in the host PC’s settings to save clips.",
                StreamMenuDvrSection.reasonText(context, StreamMenuDvrSection.DISABLED));
        assertEquals("Clips require Opus audio. Switch audio codec from passthrough to Opus to record clips.",
                StreamMenuDvrSection.reasonText(context, StreamMenuDvrSection.AUDIO_CODEC_UNSUPPORTED));
        assertEquals(context.getString(R.string.stream_dvr_reason_permission),
                StreamMenuDvrSection.clipText(context, StreamMenuDvrSection.OP_SAVE, 1, StreamMenuDvrSection.PERMISSION_DENIED, 0, 0));
        assertEquals(context.getString(R.string.stream_dvr_reason_unknown), StreamMenuDvrSection.reasonText(context, 99));
        assertEquals("00:59", StreamMenuDvrSection.clock(59));
        assertEquals("1:02:14", StreamMenuDvrSection.clock(3734));
        assertEquals("1 MB", StreamMenuDvrSection.size(1));
    }

    private static List<String> buttonTexts(View root) {
        List<String> texts = new ArrayList<>();
        ArrayDeque<View> left = new ArrayDeque<>(List.of(root));
        while (!left.isEmpty()) {
            View view = left.pop();
            if (view instanceof MaterialButton) texts.add(((MaterialButton) view).getText().toString());
            if (view instanceof ViewGroup) {
                for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) left.add(((ViewGroup) view).getChildAt(i));
            }
        }
        return texts;
    }

    @Test
    public void theMenuShowsTheGroupOnlyWhenTheStreamOffersIt() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            for (boolean offered : new boolean[] {false, true}) {
                StreamMenu.Options state = new StreamMenu.Options(NvApp.Role.STREAM, TestHosts.details("rubylight-serverinfo.xml"));
                if (offered) state.dvr = section();
                StreamMenu menu = new StreamMenu(activity, () -> state, item -> { }, null, null);
                menu.show();
                ShadowLooper.idleMainLooper(100, TimeUnit.MILLISECONDS);
                List<String> texts = buttonTexts(menu.sheet().getWindow().getDecorView());
                assertEquals(texts.toString(), offered, texts.contains("Save clip"));
                assertEquals(texts.toString(), offered, texts.contains("Start recording"));
                if (offered) {
                    // A state while the menu is open rewrites the rows in place.
                    state.dvr.onState(0, StreamMenuDvrSection.RECORDING, StreamMenuDvrSection.NONE, 60, 5);
                    assertTrue(buttonTexts(menu.sheet().getWindow().getDecorView()).contains("Stop recording"));
                }
                menu.sheet().dismiss();
            }
        }
    }
}
