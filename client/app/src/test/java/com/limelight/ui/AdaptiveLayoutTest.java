package com.limelight.ui;

import android.app.Application;
import android.graphics.Rect;
import android.os.Bundle;
import android.view.View;
import android.widget.FrameLayout;

import androidx.appcompat.app.AppCompatActivity;
import androidx.core.graphics.Insets;
import androidx.core.util.Consumer;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.window.core.layout.WindowSizeClass;
import androidx.window.core.layout.WindowWidthSizeClass;
import androidx.window.layout.FoldingFeature;
import androidx.window.layout.WindowLayoutInfo;

import com.limelight.R;
import com.limelight.utils.UiHelper;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.util.Collections;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class AdaptiveLayoutTest {
    public static class Screen extends AppCompatActivity {
        @Override protected void onCreate(Bundle state) {
            setTheme(R.style.AppTheme);
            super.onCreate(state);
            setContentView(new FrameLayout(this));
            UiHelper.notifyNewRootView(this);
        }
    }

    @Test
    public void materialBreakpointsAndLandscapeColumnsFollowAvailableSpace() {
        assertEquals(WindowWidthSizeClass.COMPACT, WindowSizeClass.compute(599, 800).getWindowWidthSizeClass());
        assertEquals(WindowWidthSizeClass.MEDIUM, WindowSizeClass.compute(600, 800).getWindowWidthSizeClass());
        assertEquals(WindowWidthSizeClass.MEDIUM, WindowSizeClass.compute(839, 800).getWindowWidthSizeClass());
        assertEquals(WindowWidthSizeClass.EXPANDED, WindowSizeClass.compute(840, 800).getWindowWidthSizeClass());
        assertEquals(1, AdaptiveLayout.gridColumns(360, 720, true, false, false));
        assertEquals(2, AdaptiveLayout.gridColumns(600, 360, true, false, false));
        assertEquals(4, AdaptiveLayout.gridColumns(1200, 800, true, false, false));
        assertEquals(2, AdaptiveLayout.gridColumns(360, 720, false, false, false));
        assertEquals(4, AdaptiveLayout.gridColumns(800, 360, false, false, false));
        assertEquals(6, AdaptiveLayout.gridColumns(800, 360, false, true, false));
        assertEquals(5, AdaptiveLayout.gridColumns(800, 360, false, true, true));
        assertEquals(1, AdaptiveLayout.gridColumns(180, 200, false, false, false));
        for (int width = 550; width < 1700; width++) {
            assertTrue(AdaptiveLayout.gridColumns(width + 1, 360, true, false, false) >=
                    AdaptiveLayout.gridColumns(width, 360, true, false, false));
        }
    }

    @Test
    public void separatingHingesChooseAContiguousPaneIncludingZeroWidthFolds() {
        Rect window = new Rect(10, 24, 1010, 800);
        assertEquals(new Rect(10, 24, 500, 800), AdaptiveLayout.usablePane(window, new Rect(500, 0, 520, 900), true, false));
        assertEquals(new Rect(520, 24, 1010, 800), AdaptiveLayout.usablePane(window, new Rect(500, 0, 520, 900), true, true));
        assertEquals(new Rect(400, 24, 1010, 800), AdaptiveLayout.usablePane(window, new Rect(400, 0, 400, 900), true, false));
        assertEquals(new Rect(10, 420, 1010, 800), AdaptiveLayout.usablePane(window, new Rect(0, 400, 1200, 420), false, false));
        assertEquals(window, AdaptiveLayout.usablePane(window, new Rect(1200, 0, 1220, 900), true, false));
        assertEquals(window, AdaptiveLayout.usablePane(window, null, false, false));
        assertEquals(new Rect(30, 24, 1010, 800), AdaptiveLayout.usablePane(window, new Rect(-10, 0, 30, 900), true, false));
        assertEquals(new Rect(10, 24, 1010, 780), AdaptiveLayout.usablePane(window, new Rect(0, 780, 1200, 820), false, false));
    }

    @Test
    @Config(sdk = 30)
    public void insetsBelongToEachWindowAndDoNotAccumulateOnResize() {
        ActivityController<Screen> first = Robolectric.buildActivity(Screen.class).setup();
        ActivityController<Screen> second = Robolectric.buildActivity(Screen.class).setup();
        try {
            View a = first.get().findViewById(android.R.id.content);
            View b = second.get().findViewById(android.R.id.content);
            a.layout(0, 0, 1000, 800);
            b.layout(0, 0, 500, 800);
            WindowInsetsCompat insets = new WindowInsetsCompat.Builder()
                    .setInsets(WindowInsetsCompat.Type.systemBars(), Insets.of(0, 24, 0, 32))
                    .setInsets(WindowInsetsCompat.Type.displayCutout(), Insets.of(40, 0, 0, 0))
                    .setInsets(WindowInsetsCompat.Type.ime(), Insets.of(0, 0, 0, 250)).build();
            ViewCompat.dispatchApplyWindowInsets(a, insets);
            ViewCompat.dispatchApplyWindowInsets(a, insets);
            a.layout(0, 0, 900, 700);
            assertEquals(40, a.getPaddingLeft());
            assertEquals(24, a.getPaddingTop());
            assertEquals(250, a.getPaddingBottom());
            assertEquals(0, b.getPaddingLeft());
            assertEquals(0, b.getPaddingBottom());
            assertSame(a.getTag(R.id.adaptive_layout), AdaptiveLayout.attach(first.get(), a));
        } finally {
            first.pause().stop().destroy();
            second.pause().stop().destroy();
        }
    }

    @Test
    @Config(sdk = 30)
    public void insetsDispatchedBeforeTheFirstLayoutAreAppliedImmediately() {
        // Cold start: the window dispatches insets before the root has a frame. Deferring them to a
        // layout listener lost the padding and left the toolbar under the status bar.
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            View root = new FrameLayout(controller.get());
            AdaptiveLayout.attach(controller.get(), root);
            assertEquals(0, root.getWidth());
            ViewCompat.dispatchApplyWindowInsets(root, new WindowInsetsCompat.Builder()
                    .setInsets(WindowInsetsCompat.Type.systemBars(), Insets.of(0, 63, 0, 48)).build());
            assertEquals(63, root.getPaddingTop());
            assertEquals(48, root.getPaddingBottom());
            root.layout(0, 0, 786, 1704);
            ShadowLooper.idleMainLooper();
            assertEquals(63, root.getPaddingTop());
            assertEquals(48, root.getPaddingBottom());
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void televisionMarginsExistBeforeADialogIsShown() {
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            androidx.appcompat.app.AlertDialog dialog = new NavigationDialogBuilder(controller.get())
                    .setMessage("Explanation").create();
            dialog.create();
            View content = dialog.findViewById(android.R.id.content);
            AdaptiveLayout.attach(controller.get(), content);
            assertFalse(dialog.isShowing());
            assertEquals(Math.round(24 * content.getResources().getDisplayMetrics().density), content.getPaddingBottom());
        }
    }

    @Test
    public void contentFrameCentresOneReadableColumnOnWideWindows() {
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            ContentFrame frame = new ContentFrame(controller.get());
            View child = new View(controller.get());
            frame.addView(child, new FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT,
                    FrameLayout.LayoutParams.MATCH_PARENT));
            frame.setMaxContentWidth(800);
            frame.measure(View.MeasureSpec.makeMeasureSpec(1200, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(600, View.MeasureSpec.EXACTLY));
            frame.layout(0, 0, 1200, 600);
            assertEquals(1200, frame.getWidth());
            assertEquals(200, child.getLeft());
            assertEquals(1000, child.getRight());
            frame.measure(View.MeasureSpec.makeMeasureSpec(700, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(600, View.MeasureSpec.EXACTLY));
            frame.layout(0, 0, 700, 600);
            assertEquals(0, child.getLeft());
            assertEquals(700, child.getRight());
        }
    }

    @Test
    @Config(sdk = 30)
    public void bottomSheetPassesBarsThroughAndAddsOnlyTheRemainingCutoutSafeArea() {
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            View root = new android.widget.FrameLayout(controller.get());
            AdaptiveLayout.attachBottomSheet(controller.get(), root);
            root.layout(0, 0, 1000, 800);
            WindowInsetsCompat insets = new WindowInsetsCompat.Builder()
                    .setInsets(WindowInsetsCompat.Type.systemBars(), Insets.of(12, 24, 0, 96))
                    .setInsets(WindowInsetsCompat.Type.displayCutout(), Insets.of(52, 24, 0, 0)).build();
            for (int i = 0; i < 2; i++) {
                WindowInsetsCompat dispatched = ViewCompat.dispatchApplyWindowInsets(root, insets);
                assertEquals(96, dispatched.getInsets(WindowInsetsCompat.Type.systemBars()).bottom);
                assertEquals(40, root.getPaddingLeft());
                assertEquals(0, root.getPaddingTop());
                assertEquals(0, root.getPaddingBottom());
            }
            Consumer<WindowLayoutInfo> listener = ReflectionHelpers.getField(root.getTag(R.id.adaptive_layout), "listener");
            listener.accept(new WindowLayoutInfo(Collections.singletonList(fold(true))));
            assertEquals(510, root.getPaddingLeft());
            assertEquals(0, root.getPaddingRight());
        }
    }

    @Test
    public void unfoldingRestoresTheFullWindowAndFlatUnoccludedFoldsDoNotSplitIt() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup();
        try {
            View root = controller.get().findViewById(android.R.id.content);
            root.layout(0, 0, 1000, 800);
            Consumer<WindowLayoutInfo> listener = ReflectionHelpers.getField(root.getTag(R.id.adaptive_layout), "listener");
            listener.accept(new WindowLayoutInfo(Collections.singletonList(fold(true))));
            assertEquals(510, root.getPaddingRight());
            listener.accept(new WindowLayoutInfo(Collections.singletonList(fold(false))));
            assertEquals(0, root.getPaddingRight());
            listener.accept(new WindowLayoutInfo(Collections.emptyList()));
            assertEquals(0, root.getPaddingLeft());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static FoldingFeature fold(boolean separating) {
        return new FoldingFeature() {
            @Override public Rect getBounds() { return new Rect(490, 0, 510, 800); }
            @Override public boolean isSeparating() { return separating; }
            @Override public State getState() { return separating ? State.HALF_OPENED : State.FLAT; }
            @Override public Orientation getOrientation() { return Orientation.VERTICAL; }
            @Override public OcclusionType getOcclusionType() { return OcclusionType.NONE; }
        };
    }
}
