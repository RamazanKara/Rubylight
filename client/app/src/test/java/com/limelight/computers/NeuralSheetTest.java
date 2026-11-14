package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.DialogInterface;
import android.os.Looper;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.ui.ToolbarIconTintTest;

import com.google.android.material.button.MaterialButton;

import org.json.JSONObject;
import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** Frame generation's sheet against scripted native_api.rs envelopes of GET /neural and POST /neural/settings. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NeuralSheetTest {
    private static final String OFF = "{\"ok\":{\"app\":\"uuid-1\",\"accepted\":false,\"switches\":[{\"id\":"
            + "\"frame_generation\",\"state\":\"configurable\",\"on\":false}],\"replaces_earlier_upscaling\":true}}";
    private static final String COPYING = "{\"ok\":{\"app\":\"uuid-1\",\"accepted\":true,\"switches\":[{\"id\":"
            + "\"frame_generation\",\"state\":\"temporarily_unavailable\",\"on\":true,\"reason\":\"Busy.\"}],"
            + "\"job\":{\"action\":\"install\",\"phase\":\"place\"}}}";
    private static final String ON = "{\"ok\":{\"app\":\"uuid-1\",\"accepted\":true,\"switches\":[{\"id\":"
            + "\"frame_generation\",\"state\":\"configurable\",\"on\":true}],\"job\":{\"action\":\"install\",\"phase\":\"done\"}}}";

    private ActivityController<ToolbarIconTintTest.Screen> controller;
    private ToolbarIconTintTest.Screen screen;
    private final Map<String, ArrayDeque<String>> replies = new HashMap<>();
    private final List<String> calls = new ArrayList<>();
    private final List<Runnable> later = new ArrayList<>();

    @Before
    public void setUp() {
        controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible();
        screen = controller.get();
    }

    @After
    public void tearDown() {
        controller.pause().stop().destroy();
    }

    private void script(String op, String... envelopes) {
        replies.computeIfAbsent(op, k -> new ArrayDeque<>()).addAll(Arrays.asList(envelopes));
    }

    private NeuralSheet sheet() {
        NativeApiClient api = new NativeApiClient((host, op, argument) -> {
            calls.add(op + " " + argument);
            ArrayDeque<String> queue = replies.get(op);
            if (queue == null || queue.isEmpty()) {
                throw new AssertionError("Unexpected " + op);
            }
            return queue.poll();
        }, new HostEndpoint("pc", "10.0.0.2", 47989, 47984, "pem", "unique", new byte[1], new byte[1]));
        NeuralSheet sheet = new NeuralSheet(screen, "uuid-1", "Cyberpunk 2077", () -> api, Runnable::run,
                Runnable::run, (task, delay) -> later.add(task));
        sheet.show();
        return sheet;
    }

    private static void texts(View view, StringBuilder into) {
        if (view instanceof TextView && ((TextView) view).getText().length() > 0) {
            into.append(((TextView) view).getText()).append('|');
        }
        if (view instanceof ViewGroup) {
            for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
                texts(((ViewGroup) view).getChildAt(i), into);
            }
        }
    }

    private static String texts(NeuralSheet sheet) {
        StringBuilder text = new StringBuilder();
        texts(sheet.sheet().findViewById(android.R.id.content), text);
        return text.toString();
    }

    private static MaterialButton row(View view, String label) {
        if (view instanceof MaterialButton && ((MaterialButton) view).getText().toString().startsWith(label)) {
            return (MaterialButton) view;
        }
        if (view instanceof ViewGroup) {
            for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
                MaterialButton found = row(((ViewGroup) view).getChildAt(i), label);
                if (found != null) return found;
            }
        }
        return null;
    }

    private MaterialButton fg(NeuralSheet sheet) {
        return row(sheet.sheet().findViewById(android.R.id.content), screen.getString(R.string.neural_frame_generation));
    }

    @Test
    public void theFirstInstallAsksFirstAndCancelSendsNothing() {
        script("neural", OFF);
        NeuralSheet sheet = sheet();
        String text = texts(sheet);
        assertTrue(text, text.contains(screen.getString(R.string.neural_replaces)));
        MaterialButton fg = fg(sheet);
        assertNotNull(text, fg);
        fg.performClick();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        AlertDialog notice = (AlertDialog) ShadowDialog.getLatestDialog();
        assertTrue(notice.isShowing());
        notice.getButton(DialogInterface.BUTTON_NEGATIVE).performClick();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        assertEquals(Arrays.asList("neural uuid-1"), calls);
    }

    @Test
    public void acceptingTurnsItOnWithTheNoticeAndReadsUntilThePcIsDone() throws Exception {
        script("neural", OFF, ON);
        script("neural.write", COPYING);
        NeuralSheet sheet = sheet();
        fg(sheet).performClick();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        AlertDialog notice = (AlertDialog) ShadowDialog.getLatestDialog();
        notice.getButton(DialogInterface.BUTTON_POSITIVE).performClick();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        assertEquals(2, calls.size());
        JSONObject write = new JSONObject(calls.get(1).substring("neural.write ".length()));
        assertTrue(write.getBoolean("on") && write.getBoolean("accept_notice"));
        assertFalse(write.has("exe"));
        assertTrue(texts(sheet).contains(screen.getString(R.string.neural_copying)));
        assertTrue(sheet.shown().working());
        // The PC still works: the sheet reads again a moment later.
        assertEquals(1, later.size());
        later.remove(0).run();
        assertEquals("neural uuid-1", calls.get(2));
        assertTrue(sheet.shown().frameGeneration.on);
        assertFalse(sheet.shown().working());
        assertTrue(later.isEmpty());
    }
}
