package com.limelight.ui;

import android.app.Application;
import android.content.res.Configuration;
import android.view.Display;
import android.view.View;
import android.view.WindowManager;
import android.widget.FrameLayout;

import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;

import com.limelight.Game;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDisplayManager;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class)
public class StreamWindowLayoutTest {
    public static class WindowedGame extends Game {
        boolean windowed;
        @Override public boolean isInMultiWindowMode() { return windowed; }
    }

    @Test
    public void nativeVideoKeepsFullPanelAcrossBothLandscapeRotationsAndTransientBars() {
        Game game = game(true);
        View root = root(game);
        for (int[] size : new int[][] {{1080, 2400}, {2400, 1080}, {2400, 1080}}) {
            root.layout(0, 0, size[0], size[1]);
            Insets cutout = size[0] < size[1] ? Insets.of(0, 80, 0, 0) :
                    Insets.of(80, 0, 0, 0);
            applyInsets(root, cutout);
            assertPadding(root, 0, 0, 0, 0);
            applyInsets(root, Insets.of(0, 0, 80, 0));
            assertPadding(root, 0, 0, 0, 0);
        }
    }

    @Test
    public void fixedPresetRetainsAspectRatioAndCutoutSafetyWithoutTransientNavigationGap() {
        Game game = game(false);
        View root = root(game);
        root.layout(0, 0, 2400, 1080);
        applyInsets(root, Insets.of(80, 0, 0, 0));
        assertPadding(root, 80, 0, 0, 0);
        StreamView video = new StreamView(game);
        video.setDesiredAspectRatio(16.0 / 9.0);
        video.measure(View.MeasureSpec.makeMeasureSpec(2320, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
        assertEquals(1920, video.getMeasuredWidth());
        assertEquals(1080, video.getMeasuredHeight());
    }

    @Test
    public void windowedVideoAvoidsSystemBarsAndReturningFullscreenRestoresThePanel() {
        WindowedGame game = Robolectric.buildActivity(WindowedGame.class).get();
        PreferenceConfiguration preferences = new PreferenceConfiguration();
        preferences.resolutionFollowsScreen = true;
        ReflectionHelpers.setField(game, "prefConfig", preferences);
        game.windowed = true;
        View root = root(game);
        root.layout(0, 0, 1000, 800);
        applyInsets(root, Insets.of(80, 0, 0, 0));
        assertPadding(root, 80, 24, 0, 48);
        game.windowed = false;
        root.layout(0, 0, 2400, 1080);
        assertPadding(root, 0, 0, 0, 0);
    }

    @Test
    public void exactPanelResolutionUsesFullDisplayEvenWhenItMatchesAStandardPreset() {
        Game game = game(false);
        Display display = game.getWindowManager().getDefaultDisplay();
        Display.Mode[] original = display.getSupportedModes();
        try {
            Display.Mode mode = ReflectionHelpers.callConstructor(Display.Mode.class,
                    ReflectionHelpers.ClassParameter.from(int.class, display.getMode().getModeId()),
                    ReflectionHelpers.ClassParameter.from(int.class, 1080),
                    ReflectionHelpers.ClassParameter.from(int.class, 1920),
                    ReflectionHelpers.ClassParameter.from(float.class, 60f));
            ShadowDisplayManager.setSupportedModes(display.getDisplayId(), mode);
            assertTrue(game.usesFullDisplayForVideo());
        } finally {
            ShadowDisplayManager.setSupportedModes(display.getDisplayId(), original);
        }
    }

    @Test
    public void rotationReappliesExplicitCutoutLayoutBeforeStreamingIsReady() {
        Game game = game(true);
        root(game);
        game.getWindow().getAttributes().layoutInDisplayCutoutMode =
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_DEFAULT;
        game.onConfigurationChanged(new Configuration());
        assertEquals(WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS,
                game.getWindow().getAttributes().layoutInDisplayCutoutMode);
    }

    @Test
    public void insetExcludedNativeResolutionKeepsTheUsersCutoutSafeChoice() {
        assertCustomResolutionInsets(2320, 1080, false, false);
    }

    @Test
    public void customPcProfileResolutionDoesNotImplyNotchSpanningVideo() {
        assertCustomResolutionInsets(2048, 1152, true, false);
    }

    @Test
    public void explicitStretchStillUsesTheFullPanelForACustomPcProfile() {
        assertCustomResolutionInsets(2048, 1152, true, true);
    }

    private static void assertCustomResolutionInsets(int width, int height, boolean pcProfile, boolean stretch) {
        Game game = game(false);
        PreferenceConfiguration preferences = ReflectionHelpers.getField(game, "prefConfig");
        preferences.width = width;
        preferences.height = height;
        preferences.fromPcProfile = pcProfile;
        preferences.stretchVideo = stretch;
        // This flag is also true for the safe Native entry and any custom nonstandard size.
        ReflectionHelpers.setField(game, "nativeResolutionStream", true);
        Display display = game.getWindowManager().getDefaultDisplay();
        Display.Mode[] original = display.getSupportedModes();
        try {
            Display.Mode panel = ReflectionHelpers.callConstructor(Display.Mode.class,
                    ReflectionHelpers.ClassParameter.from(int.class, display.getMode().getModeId()),
                    ReflectionHelpers.ClassParameter.from(int.class, 1080),
                    ReflectionHelpers.ClassParameter.from(int.class, 2400),
                    ReflectionHelpers.ClassParameter.from(float.class, 60f));
            ShadowDisplayManager.setSupportedModes(display.getDisplayId(), panel);
            View root = root(game);
            root.layout(0, 0, 2400, 1080);
            applyInsets(root, Insets.of(80, 0, 0, 0));
            assertEquals(stretch, game.usesFullDisplayForVideo());
            assertPadding(root, stretch ? 0 : 80, 0, 0, 0);
        } finally {
            ShadowDisplayManager.setSupportedModes(display.getDisplayId(), original);
        }
    }

    private static Game game(boolean nativeResolution) {
        Game game = Robolectric.buildActivity(Game.class).get();
        PreferenceConfiguration preferences = new PreferenceConfiguration();
        preferences.width = 1920;
        preferences.height = 1080;
        preferences.resolutionFollowsScreen = nativeResolution;
        ReflectionHelpers.setField(game, "prefConfig", preferences);
        return game;
    }

    private static View root(Game game) {
        game.setContentView(new FrameLayout(game));
        View root = game.findViewById(android.R.id.content);
        AdaptiveLayout.attach(game, root);
        return root;
    }

    private static void applyInsets(View root, Insets cutout) {
        ViewCompat.dispatchApplyWindowInsets(root, new WindowInsetsCompat.Builder()
                .setInsets(WindowInsetsCompat.Type.systemBars(), Insets.of(0, 24, 0, 48))
                .setInsets(WindowInsetsCompat.Type.displayCutout(), cutout)
                .setInsets(WindowInsetsCompat.Type.ime(), Insets.of(0, 0, 0, 400)).build());
    }

    private static void assertPadding(View root, int left, int top, int right, int bottom) {
        assertEquals(left, root.getPaddingLeft());
        assertEquals(top, root.getPaddingTop());
        assertEquals(right, root.getPaddingRight());
        assertEquals(bottom, root.getPaddingBottom());
    }
}
