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
        boolean coversDisplay;
        @Override public boolean isInMultiWindowMode() { return windowed; }
        @Override protected boolean windowCoversDisplay() { return coversDisplay; }
    }

    // Galaxy Z Fold 7 inner screen in landscape: 2184x1968, punch-hole camera on a long edge.
    private static final int FOLD_WIDTH = 2184;
    private static final int FOLD_HEIGHT = 1968;
    private static final int FOLD_CUTOUT = 54;

    private static Game foldStream(boolean followScreen, boolean stretch, int width, int height) {
        Game game = game(followScreen);
        PreferenceConfiguration preferences = ReflectionHelpers.getField(game, "prefConfig");
        preferences.width = width;
        preferences.height = height;
        preferences.setVideoScaling(stretch ? VideoScaling.STRETCH : VideoScaling.FIT);
        return game;
    }

    /** Lays out the stream like activity_game.xml and returns the video view. */
    private static StreamView layOutStream(Game game, View root, Insets cutout, boolean stretch) {
        PreferenceConfiguration preferences = ReflectionHelpers.getField(game, "prefConfig");
        FrameLayout content = (FrameLayout) root;
        content.removeAllViews();
        View background = new View(game);
        StreamView video = new StreamView(game);
        video.setVideoScaling(stretch ? VideoScaling.STRETCH : VideoScaling.FIT, VideoScaling.CENTER,
                preferences.width, preferences.height);
        content.addView(background, new FrameLayout.LayoutParams(-1, -1, android.view.Gravity.CENTER));
        content.addView(video, new FrameLayout.LayoutParams(-1, -1, android.view.Gravity.CENTER));
        ReflectionHelpers.setField(game, "streamView", video);
        ViewCompat.dispatchApplyWindowInsets(root, new WindowInsetsCompat.Builder()
                .setInsets(WindowInsetsCompat.Type.displayCutout(), cutout).build());
        root.forceLayout();
        root.measure(View.MeasureSpec.makeMeasureSpec(FOLD_WIDTH, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(FOLD_HEIGHT, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, FOLD_WIDTH, FOLD_HEIGHT);
        return video;
    }

    private static float[] touch(Game game, View view, float x, float y) {
        android.view.MotionEvent event = android.view.MotionEvent.obtain(0, 0, android.view.MotionEvent.ACTION_DOWN, x, y, 0);
        try {
            return ReflectionHelpers.callInstanceMethod(ReflectionHelpers.getField(game, "touchInput"), "getStreamViewRelativeNormalizedXY",
                    ReflectionHelpers.ClassParameter.from(View.class, view),
                    ReflectionHelpers.ClassParameter.from(android.view.MotionEvent.class, event),
                    ReflectionHelpers.ClassParameter.from(int.class, 0));
        } finally {
            event.recycle();
        }
    }

    @Test
    public void fullPanelStreamsSpanTheFoldInnerScreenWithTheCutoutOnEitherLongEdge() {
        // Follow-screen, the exact native size and a stretched stream all use the strip beside the camera.
        boolean[][] streams = {{true, false}, {false, false}, {false, true}};
        for (boolean[] stream : streams) {
            boolean follow = stream[0];
            boolean stretch = stream[1];
            Game game = foldStream(follow, stretch, FOLD_WIDTH, FOLD_HEIGHT);
            if (!follow && !stretch) useFoldPanelMode(game);
            View root = root(game);
            for (Insets cutout : new Insets[] {Insets.of(0, 0, FOLD_CUTOUT, 0), Insets.of(FOLD_CUTOUT, 0, 0, 0),
                    Insets.of(0, 0, FOLD_CUTOUT, 0)}) {
                StreamView video = layOutStream(game, root, cutout, stretch);
                String label = "follow=" + follow + " stretch=" + stretch + " cutout=" + cutout;
                assertTrue(label, game.usesFullDisplayForVideo());
                assertPadding(root, 0, 0, 0, 0);
                assertEquals(label, 0, video.getLeft());
                assertEquals(label, FOLD_WIDTH, video.getRight());
                assertEquals(label, FOLD_HEIGHT, video.getHeight());
                // A touch at the right edge of the panel, over the camera, is the right edge of the stream.
                assertEquals(label, 1f, touch(game, video, FOLD_WIDTH, 0)[0], 0.001f);
            }
        }
    }

    private static void useFoldPanelMode(Game game) {
        Display display = game.getWindowManager().getDefaultDisplay();
        Display.Mode panel = ReflectionHelpers.callConstructor(Display.Mode.class,
                ReflectionHelpers.ClassParameter.from(int.class, display.getMode().getModeId()),
                ReflectionHelpers.ClassParameter.from(int.class, FOLD_HEIGHT),
                ReflectionHelpers.ClassParameter.from(int.class, FOLD_WIDTH),
                ReflectionHelpers.ClassParameter.from(float.class, 120f));
        ShadowDisplayManager.setSupportedModes(display.getDisplayId(), panel);
    }

    @Test
    public void aStreamInThePanelsShapeAtAnotherSizeSpansTheCutoutToo() {
        // Half the Fold's panel, or a PC profile's size taken from a scaled screen: Fit fills the panel.
        for (int[] size : new int[][] {{FOLD_WIDTH / 2, FOLD_HEIGHT / 2}, {1638, 1476}}) {
            Game game = foldStream(false, false, size[0], size[1]);
            useFoldPanelMode(game);
            View root = root(game);
            StreamView video = layOutStream(game, root, Insets.of(0, 0, FOLD_CUTOUT, 0), false);
            String label = size[0] + "x" + size[1];
            assertTrue(label, game.usesFullDisplayForVideo());
            assertPadding(root, 0, 0, 0, 0);
            assertEquals(label, 0, video.getLeft());
            assertEquals(label, FOLD_WIDTH, video.getRight());
            assertEquals(label, FOLD_HEIGHT, video.getHeight());
        }
        // A 16:9 preset keeps the cutout clear.
        Game game = foldStream(false, false, 1920, 1080);
        useFoldPanelMode(game);
        assertFalse(game.usesFullDisplayForVideo());
    }

    @Test
    public void exactNativeSizeCountsAsFullPanelEvenWhenTheModeListDisagrees() {
        Game game = foldStream(false, false, 2400, 1080);
        Display display = game.getWindowManager().getDefaultDisplay();
        android.graphics.Point real = new android.graphics.Point();
        display.getRealSize(real);
        PreferenceConfiguration preferences = ReflectionHelpers.getField(game, "prefConfig");
        preferences.width = Math.max(real.x, real.y);
        preferences.height = Math.min(real.x, real.y);
        Display.Mode[] original = display.getSupportedModes();
        try {
            Display.Mode scaled = ReflectionHelpers.callConstructor(Display.Mode.class,
                    ReflectionHelpers.ClassParameter.from(int.class, display.getMode().getModeId()),
                    ReflectionHelpers.ClassParameter.from(int.class, 7),
                    ReflectionHelpers.ClassParameter.from(int.class, 7),
                    ReflectionHelpers.ClassParameter.from(float.class, 60f));
            ShadowDisplayManager.setSupportedModes(display.getDisplayId(), scaled);
            assertTrue(game.usesFullDisplayForVideo());
        } finally {
            ShadowDisplayManager.setSupportedModes(display.getDisplayId(), original);
        }
    }

    @Test
    public void cutoutSafeNativeKeepsTheStripBesideTheCameraAfterEitherRotation() {
        int safeWidth = FOLD_WIDTH - FOLD_CUTOUT;
        Game game = foldStream(false, false, safeWidth, FOLD_HEIGHT);
        View root = root(game);
        StreamView video = layOutStream(game, root, Insets.of(0, 0, FOLD_CUTOUT, 0), false);
        assertFalse(game.usesFullDisplayForVideo());
        assertPadding(root, 0, 0, FOLD_CUTOUT, 0);
        assertEquals(safeWidth, video.getRight());
        // A touch in the strip is clamped to the right edge of the stream.
        assertEquals(1f, touch(game, root, FOLD_WIDTH - 10, 10)[0], 0.001f);
        video = layOutStream(game, root, Insets.of(FOLD_CUTOUT, 0, 0, 0), false);
        assertPadding(root, FOLD_CUTOUT, 0, 0, 0);
        assertEquals(FOLD_CUTOUT, video.getLeft());
        // After the other rotation the stream starts after the strip, and touches follow it.
        assertEquals(0f, touch(game, root, FOLD_CUTOUT, 10)[0], 0.001f);
        assertEquals(0.5f, touch(game, root, FOLD_CUTOUT + safeWidth / 2f, 10)[0], 0.001f);
    }

    @Test
    public void aMultiWindowStreamThatCoversTheDisplayKeepsTheWholePanel() {
        // A pop-up or free-form window maximised over the display still reports multi-window mode.
        WindowedGame game = Robolectric.buildActivity(WindowedGame.class).get();
        PreferenceConfiguration preferences = new PreferenceConfiguration();
        preferences.resolutionFollowsScreen = true;
        preferences.width = FOLD_WIDTH;
        preferences.height = FOLD_HEIGHT;
        ReflectionHelpers.setField(game, "prefConfig", preferences);
        game.windowed = true;
        game.coversDisplay = true;
        View root = root(game);
        layOutStream(game, root, Insets.of(0, 0, FOLD_CUTOUT, 0), false);
        assertFalse(game.isWindowedStream());
        assertPadding(root, 0, 0, 0, 0);
        game.coversDisplay = false;
        assertTrue(game.isWindowedStream());
        applyInsets(root, Insets.of(0, 0, FOLD_CUTOUT, 0));
        assertPadding(root, 0, 24, FOLD_CUTOUT, 48);
    }

    @Test
    public void aFlatFoldNeverSplitsOrShrinksTheStream() {
        Game game = foldStream(true, false, FOLD_WIDTH, FOLD_HEIGHT);
        View root = root(game);
        StreamView video = layOutStream(game, root, Insets.NONE, false);
        androidx.core.util.Consumer<androidx.window.layout.WindowLayoutInfo> listener =
                ReflectionHelpers.getField(root.getTag(com.limelight.R.id.adaptive_layout), "listener");
        listener.accept(new androidx.window.layout.WindowLayoutInfo(java.util.Collections.singletonList(
                fold(androidx.window.layout.FoldingFeature.State.FLAT,
                        androidx.window.layout.FoldingFeature.OcclusionType.FULL, false))));
        assertPadding(root, 0, 0, 0, 0);
        assertEquals(FOLD_WIDTH, video.getWidth());
        // Half-opened like a book, the stream may still move to one side of the hinge.
        listener.accept(new androidx.window.layout.WindowLayoutInfo(java.util.Collections.singletonList(
                fold(androidx.window.layout.FoldingFeature.State.HALF_OPENED,
                        androidx.window.layout.FoldingFeature.OcclusionType.NONE, true))));
        assertTrue(root.getPaddingLeft() + root.getPaddingRight() > 0);
    }

    private static androidx.window.layout.FoldingFeature fold(androidx.window.layout.FoldingFeature.State state,
            androidx.window.layout.FoldingFeature.OcclusionType occlusion, boolean separating) {
        return new androidx.window.layout.FoldingFeature() {
            @Override public android.graphics.Rect getBounds() {
                return new android.graphics.Rect(FOLD_WIDTH / 2, 0, FOLD_WIDTH / 2, FOLD_HEIGHT);
            }
            @Override public boolean isSeparating() { return separating; }
            @Override public State getState() { return state; }
            @Override public Orientation getOrientation() { return Orientation.VERTICAL; }
            @Override public OcclusionType getOcclusionType() { return occlusion; }
        };
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
        video.setVideoScaling(VideoScaling.FIT, VideoScaling.CENTER, 1920, 1080);
        video.measure(View.MeasureSpec.makeMeasureSpec(2320, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
        assertEquals(1920, video.getMeasuredWidth());
        assertEquals(1080, video.getMeasuredHeight());
    }

    @Test
    public void everyScalingModeLaysTheViewOutWhereTheMathPutsIt() {
        Game game = game(false);
        VideoScaling expected = new VideoScaling();
        int[][] cases = {{1920, 1080, 2400, 1080}, {1920, 1080, 1080, 2400}, {1280, 720, 2400, 1080},
                {2560, 1600, 2208, 1768}, {800, 600, 2400, 1080}};
        for (int[] c : cases) {
            for (int mode = VideoScaling.FIT; mode <= VideoScaling.NATIVE; mode++) {
                for (int alignment : new int[] {VideoScaling.CENTER, VideoScaling.TOP}) {
                    FrameLayout parent = new FrameLayout(game);
                    StreamView video = new StreamView(game);
                    parent.addView(video, new FrameLayout.LayoutParams(-1, -1, android.view.Gravity.CENTER));
                    video.setVideoScaling(mode, alignment, c[0], c[1]);
                    parent.measure(View.MeasureSpec.makeMeasureSpec(c[2], View.MeasureSpec.EXACTLY),
                            View.MeasureSpec.makeMeasureSpec(c[3], View.MeasureSpec.EXACTLY));
                    parent.layout(0, 0, c[2], c[3]);
                    expected.layout(mode, alignment, c[0], c[1], c[2], c[3]);
                    String label = java.util.Arrays.toString(c) + " mode=" + mode + " alignment=" + alignment;
                    assertEquals(label, expected.left(), video.getLeft());
                    assertEquals(label, expected.top(), video.getTop());
                    assertEquals(label, expected.width(), video.getWidth());
                    assertEquals(label, expected.height(), video.getHeight());
                }
            }
        }
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
        preferences.setVideoScaling(stretch ? VideoScaling.STRETCH : VideoScaling.FIT);
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
