package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.util.DisplayMetrics;
import android.view.View;

import com.limelight.R;
import com.limelight.nvstream.http.NvApp;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

import java.io.File;
import java.io.FileOutputStream;
import java.util.ArrayList;

import static org.junit.Assert.*;

/**
 * The Control Center drawn at each form's size, for eyes: with NIGHTJAR_PREVIEW_OUTPUT set it writes the sheet of an
 * upright phone and the dock of a handheld and of a TV, on Quick, Picture and Power, over a stream on a PC that offers
 * all there is, with the stream's notice and the encoder's load. Without it, it still holds each form to its placement
 * and each of those modules to a page that has room in the panel.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = ControlCenterTest.HANDHELD)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class ControlCenterPreviewTest {
    @Before
    public void aDeviceThatDoesNotBlur() {
        ControlCenterTest.quietDevice();
    }

    @After
    public void forget() {
        ControlCenterTest.forgetDevice();
    }

    @Test @Config(qualifiers = ControlCenterTest.PHONE_UPRIGHT)
    public void anUprightPhoneGetsTheSheet() throws Exception {
        draw("phone", ControlCenterModel.SHEET, false, ControlCenterModel.QUICK, ControlCenterModel.PICTURE,
                ControlCenterModel.CLIPS, ControlCenterModel.POWER);
    }

    @Test
    public void aHandheldGetsTheCompactDock() throws Exception {
        draw("handheld", ControlCenterModel.DOCK, true, ControlCenterModel.QUICK, ControlCenterModel.PICTURE);
    }

    @Test @Config(qualifiers = ControlCenterTest.TELEVISION)
    public void aTvGetsTheDock() throws Exception {
        draw("tv", ControlCenterModel.DOCK, true, ControlCenterModel.QUICK, ControlCenterModel.PICTURE,
                ControlCenterModel.CLIPS, ControlCenterModel.POWER);
    }

    // Opens it over a stream that has everything, with a controller or without, and walks the modules named.
    private static void draw(String form, String placement, boolean pad, String... modules) throws Exception {
        StreamMenu.gamepadForTest = pad;
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            StreamMenu.Options state = ControlCenterTest.everything(NvApp.Role.STREAM);
            state.rateChange = true;
            state.overlayShown = true;
            state.launchNotice = activity.getString(R.string.launch_warning_virtual_display_unavailable);
            state.encoderLoad = "37.5";
            // Instant Replay on Clips and a game paused by Quick Suspend (Resume game on Power), as the host offers them.
            state.gates = java.util.Collections.singleton("stream_dvr");
            state.dvr = new StreamMenuDvrSection(activity, (id, op, secs) -> true, text -> { }, () -> 0);
            state.quickSuspendOffered = state.quickSuspendPaused = true;
            StreamMenu menu = ControlCenterTest.open(activity, state, new ArrayList<>(), null);
            ControlCenterTest.layOut(activity, menu);
            assertEquals(placement, menu.sheet().placement());
            for (String module : modules) {
                assertTrue(module + " is on the rail", menu.modules().contains(module));
                menu.select(module);
                ControlCenterTest.layOut(activity, menu);
                assertEquals(module, menu.module());
                View page = menu.sheet().page().scroll();
                assertTrue(module + ": its page has room in the panel", page.getWidth() > 0 && page.getHeight() > 0);
                capture(activity, menu, form + "-control-center-" + module + ".png");
            }
            menu.sheet().dismiss();
        }
    }

    // The Control Center's own window at the screen's size, over a grey that stands for the picture.
    private static void capture(Activity activity, StreamMenu menu, String name) throws Exception {
        String output = System.getenv("NIGHTJAR_PREVIEW_OUTPUT");
        if (output == null || output.isBlank()) return;
        File directory = new File(output, "day");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        DisplayMetrics screen = activity.getResources().getDisplayMetrics();
        Bitmap bitmap = Bitmap.createBitmap(screen.widthPixels, screen.heightPixels, Bitmap.Config.ARGB_8888);
        Canvas canvas = new Canvas(bitmap);
        canvas.drawColor(0xFF505860);
        menu.sheet().getWindow().getDecorView().draw(canvas);
        try (FileOutputStream stream = new FileOutputStream(new File(directory, name))) {
            assertTrue(bitmap.compress(Bitmap.CompressFormat.PNG, 100, stream));
        }
    }
}
