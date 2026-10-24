package com.limelight;

import android.app.Application;
import android.content.Context;
import android.hardware.display.DisplayManager;
import android.view.Display;
import android.view.Surface;

import com.limelight.binding.video.DisplayCapsReporter;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.ui.StreamPresentation;
import com.limelight.ui.StreamView;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.annotation.RealObject;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.shadows.ShadowDisplay;
import org.robolectric.shadows.ShadowDisplayManager;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = {29, 36}, application = Application.class,
        shadows = {GameDisplayCapsTest.CapsBridge.class, GameDisplayCapsTest.ModeDisplay.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameDisplayCapsTest {
    private final List<DisplayCapsReporter.Caps> reports = new ArrayList<>();

    @After public void resetModes() {
        ModeDisplay.modes.clear();
    }

    @Test public void brightnessRefreshModeIdAndRotationKeepTheCurrentPanelReport() {
        Game game = connectedGame();
        Display display = game.getWindowManager().getDefaultDisplay();
        panel(display, 1, 1968, 2184, 60, 1000);
        report(game);
        assertEquals(1, reports.size());
        assertTrue(reports.get(0).hdr);

        luminance(display, 1600);
        report(game);
        assertEquals("Brightness alone must not change source HDR metadata", 1, reports.size());

        panel(display, 2, 1968, 2184, 120, 1800);
        report(game);
        assertEquals("Refresh and Android mode IDs do not identify a new panel", 1, reports.size());

        Shadows.shadowOf(display).setRotation(Surface.ROTATION_90);
        panel(display, 3, 2184, 1968, 120, 2000);
        report(game);
        assertEquals("Rotating the same physical dimensions must retain the report", 1, reports.size());
        assertEquals(1000, reports.get(0).maxNits, 0);
    }

    @Test public void anHlgOnlyOutputIsNotReportedAsAnHdr10Output() {
        // The request needs HDR10; the report to the host must use the same test.
        Game game = connectedGame();
        Display display = game.getWindowManager().getDefaultDisplay();
        panel(display, 1, 1968, 2184, 60, 1000);
        Shadows.shadowOf(display).setDisplayHdrCapabilities(display.getDisplayId(), 1000, 500, 0.0005f,
                Display.HdrCapabilities.HDR_TYPE_HLG);
        report(game);
        assertEquals(1, reports.size());
        assertFalse(reports.get(0).hdr);
    }

    @Test public void aFoldWithTheSameLogicalDisplayIdReportsTheNewPanel() {
        Game game = connectedGame();
        Display display = game.getWindowManager().getDefaultDisplay();
        panel(display, 1, 1968, 2184, 120, 1000);
        report(game);

        // Foldable devices can retain both the logical display ID and mode ID.
        panel(display, 1, 1080, 2520, 120, 800);
        report(game);
        assertEquals(2, reports.size());
        assertEquals(800, reports.get(1).maxNits, 0);

        luminance(display, 1200);
        report(game);
        assertEquals("The new panel also ignores later brightness measurements", 2, reports.size());
    }

    @Test public void anExternalOutputWithMatchingDimensionsReportsItsOwnCapabilities() {
        Game game = connectedGame();
        Display local = game.getWindowManager().getDefaultDisplay();
        panel(local, 1, 1968, 2184, 120, 1000);
        report(game);
        int externalId = ShadowDisplayManager.addDisplay("w1968dp-h2184dp");
        try {
            Display external = ((DisplayManager) game.getSystemService(Context.DISPLAY_SERVICE))
                    .getDisplay(externalId);
            panel(external, 1, 1968, 2184, 120, 1600);
            StreamPresentation presentation = new StreamPresentation(game, external, false);
            ReflectionHelpers.setField(game, "externalPresentation", presentation);
            ReflectionHelpers.setField(game, "renderView", presentation.videoView());
            report(game);
            assertEquals(2, reports.size());
            assertEquals(1600, reports.get(1).maxNits, 0);

            luminance(external, 2000);
            report(game);
            assertEquals(2, reports.size());

            ReflectionHelpers.setField(game, "externalPresentation", null);
            ReflectionHelpers.setField(game, "renderView", new StreamView(game));
            report(game);
            assertEquals(3, reports.size());
            assertEquals(1000, reports.get(2).maxNits, 0);
        } finally {
            ShadowDisplayManager.removeDisplay(externalId);
        }
    }

    private Game connectedGame() {
        Game game = Robolectric.buildActivity(Game.class).get();
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "streamHdr", true);
        ReflectionHelpers.setField(game, "renderView", new StreamView(game));
        ReflectionHelpers.setField(game, "displayCapsReporter", new DisplayCapsReporter((hdr, peak, average, black) -> {
            reports.add(DisplayCapsReporter.Caps.of(hdr, peak, average, black));
            return true;
        }));
        return game;
    }

    private static void report(Game game) {
        ReflectionHelpers.callInstanceMethod(game, "reportDisplayCaps");
    }

    private static void panel(Display display, int modeId, int width, int height, float refresh, float peak) {
        ModeDisplay.modes.put(display.getDisplayId(), ReflectionHelpers.callConstructor(Display.Mode.class,
                ReflectionHelpers.ClassParameter.from(int.class, modeId),
                ReflectionHelpers.ClassParameter.from(int.class, width),
                ReflectionHelpers.ClassParameter.from(int.class, height),
                ReflectionHelpers.ClassParameter.from(float.class, refresh)));
        luminance(display, peak);
    }

    private static void luminance(Display display, float peak) {
        Shadows.shadowOf(display).setDisplayHdrCapabilities(display.getDisplayId(), peak, peak / 2, 0.0005f,
                Display.HdrCapabilities.HDR_TYPE_HDR10);
    }

    // Keep the platform Display/HDR shadows, while controlling the selected mode independently
    // of Robolectric's supported-mode list (which otherwise retains its original selected ID).
    @Implements(Display.class)
    public static class ModeDisplay extends ShadowDisplay {
        static final Map<Integer, Display.Mode> modes = new HashMap<>();
        @RealObject private Display display;

        @Implementation protected Display.Mode getMode() {
            Display.Mode mode = modes.get(display.getDisplayId());
            return mode != null ? mode : Shadow.directlyOn(display, Display.class, "getMode");
        }
    }

    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class CapsBridge {
        @Implementation protected static void __staticInitializer__() {}
        @Implementation public static boolean hostSupportsControlMessage(int messageType) {
            return messageType == MoonBridge.CONTROL_MESSAGE_DISPLAY_CAPS;
        }
    }
}
