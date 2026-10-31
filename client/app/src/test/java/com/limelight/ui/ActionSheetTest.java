package com.limelight.ui;

import android.app.Application;
import android.view.View;
import com.limelight.R;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class ActionSheetTest {
    @Test @Config(qualifiers = "television")
    public void focusedTvTogglesHaveRoomToGrow() {
        try (var controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ActionSheet sheet = new ActionSheet(controller.get(), "Menu");
            var row = sheet.addToggleRow();
            var first = sheet.addToggle(row, R.drawable.ic_play, "Play", false);
            var second = sheet.addToggle(row, R.drawable.ic_settings, "Settings", false);
            sheet.show();
            row.measure(View.MeasureSpec.makeMeasureSpec(600, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(200, View.MeasureSpec.AT_MOST));
            row.layout(0, 0, 600, row.getMeasuredHeight());
            first.requestFocus();
            first.jumpDrawablesToCurrentState();
            assertTrue(second.getLeft() - first.getRight() > first.getWidth() * (first.getScaleX() - 1) / 2);
            sheet.dismiss();
        }
    }

    /**
     * Emulator run, phone held upright: the stream menu's four toggles stacked into four full-width bars that took
     * half the screen. Too wide for one line, they form two columns of equal tiles; narrower still, they stack.
     */
    @Test @org.robolectric.annotation.GraphicsMode(org.robolectric.annotation.GraphicsMode.Mode.NATIVE)
    public void fourTogglesTooWideForOneLineFormTwoColumns() {
        try (var controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ActionSheet sheet = new ActionSheet(controller.get(), "Stream");
            var row = (ActionSheet.ToggleRow) sheet.addToggleRow();
            var buttons = new com.google.android.material.button.MaterialButton[] {
                    sheet.addToggle(row, R.drawable.ic_play, "System keyboard", false),
                    sheet.addToggle(row, R.drawable.ic_settings, "PC keyboard", false),
                    sheet.addToggle(row, R.drawable.ic_play, "Overlay", true),
                    sheet.addToggle(row, R.drawable.ic_settings, "Microphone", true)};
            sheet.show();
            float density = row.getResources().getDisplayMetrics().density;
            int width = Math.round(380 * density);
            row.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED));
            row.layout(0, 0, width, row.getMeasuredHeight());
            assertEquals(2, row.columns());
            assertEquals(buttons[0].getTop(), buttons[1].getTop());
            assertEquals(buttons[0].getWidth(), buttons[1].getWidth());
            assertTrue(buttons[1].getLeft() > buttons[0].getRight());
            assertTrue(buttons[2].getTop() > buttons[0].getBottom());
            assertEquals(buttons[0].getLeft(), buttons[2].getLeft());
            assertEquals(buttons[3].getRight(), buttons[1].getRight());
            assertTrue(row.getMeasuredHeight() >= buttons[3].getBottom());
            int wide = Math.round(900 * density);
            row.measure(View.MeasureSpec.makeMeasureSpec(wide, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED));
            assertEquals(4, row.columns());
            int narrow = Math.round(120 * density);
            row.measure(View.MeasureSpec.makeMeasureSpec(narrow, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED));
            assertEquals(1, row.columns());
            sheet.dismiss();
        }
    }

    @Test @Config(qualifiers = "television")
    public void tvMenuFocusFollowsTheRemoteAndClearsThePreviousRow() {
        try (var controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ActionSheet sheet = new ActionSheet(controller.get(), "Menu");
            var first = sheet.addAction(R.drawable.ic_play, "Play");
            var second = sheet.addAction(R.drawable.ic_settings, "Settings");
            sheet.show();
            int ring = Math.round(3 * first.getResources().getDisplayMetrics().density);
            int rest = Math.round(first.getResources().getDisplayMetrics().density);
            first.requestFocus();
            first.jumpDrawablesToCurrentState();
            assertEquals(ring, first.getStrokeWidth());
            second.requestFocus();
            first.jumpDrawablesToCurrentState();
            second.jumpDrawablesToCurrentState();
            assertEquals(rest, first.getStrokeWidth());
            assertEquals(ring, second.getStrokeWidth());
            assertEquals(R.style.TvSheetAnimation, sheet.getWindow().getAttributes().windowAnimations);
            sheet.dismiss();
        }
    }

    /**
     * TV run on a TCL C6K: the focused row of the PC menu grew 2.5 % and its left edge stood about 12 px out of line
     * with the rows under it. A row takes the ring and keeps its size; a side-by-side toggle may still grow.
     */
    @Test @Config(qualifiers = "television")
    public void tvMenuRowsTakeTheRingWithoutZooming() {
        try (var controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ActionSheet sheet = new ActionSheet(controller.get(), "Menu");
            var row = sheet.addAction(R.drawable.ic_play, "Play");
            var page = sheet.addPageAction(R.drawable.ic_settings, "Settings");
            var toggles = sheet.addToggleRow();
            var toggle = sheet.addToggle(toggles, R.drawable.ic_play, "Toggle", false);
            sheet.show();
            for (var button : new com.google.android.material.button.MaterialButton[] {row, page}) {
                button.requestFocus();
                button.jumpDrawablesToCurrentState();
                assertTrue(button.isFocused());
                assertEquals(1f, button.getScaleX(), 0.001f);
                assertEquals(1f, button.getScaleY(), 0.001f);
                assertEquals(Math.round(3 * button.getResources().getDisplayMetrics().density), button.getStrokeWidth());
            }
            toggle.requestFocus();
            toggle.jumpDrawablesToCurrentState();
            assertEquals(1.025f, toggle.getScaleX(), 0.001f);
            sheet.dismiss();
        }
    }

    /** High contrast keeps its thicker ring on a row, still without the zoom. */
    @Test @Config(qualifiers = "television")
    public void tvMenuRowsInHighContrastTakeTheThickRingWithoutZooming() {
        try (var controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            controller.get().getTheme().applyStyle(R.style.ThemeOverlay_Nightjar_HighContrast, true);
            ActionSheet sheet = new ActionSheet(controller.get(), "Menu");
            var row = sheet.addAction(R.drawable.ic_play, "Play");
            sheet.addAction(R.drawable.ic_settings, "Settings");
            sheet.show();
            row.requestFocus();
            row.jumpDrawablesToCurrentState();
            assertEquals(1f, row.getScaleX(), 0.001f);
            assertEquals(Math.round(4 * row.getResources().getDisplayMetrics().density), row.getStrokeWidth());
            sheet.dismiss();
        }
    }

    /** A phone's menu rows keep their look: no TV ring and no zoom. */
    @Test
    public void phoneMenuRowsTakeNoTvRing() {
        try (var controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ActionSheet sheet = new ActionSheet(controller.get(), "Menu");
            var row = sheet.addAction(R.drawable.ic_play, "Play");
            sheet.addAction(R.drawable.ic_settings, "Settings");
            sheet.show();
            row.requestFocus();
            row.jumpDrawablesToCurrentState();
            assertEquals(0, row.getStrokeWidth());
            assertEquals(1f, row.getScaleX(), 0.001f);
            sheet.dismiss();
        }
    }
    @Test @Config(qualifiers = "television")
    public void tvPageChangesStayVisibleWhenSystemAnimationsAreDisabled() {
        org.robolectric.util.ReflectionHelpers.callStaticMethod(android.animation.ValueAnimator.class, "setDurationScale",
                org.robolectric.util.ReflectionHelpers.ClassParameter.from(float.class, 0f));
        try (var controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ActionSheet sheet = new ActionSheet(controller.get(), "Menu");
            sheet.addAction(R.drawable.ic_settings, "Settings");
            sheet.show();
            sheet.clearActions();
            var action = sheet.addAction(R.drawable.ic_play, "Play");
            sheet.focusFirstAction();
            org.robolectric.shadows.ShadowLooper.idleMainLooper(32, java.util.concurrent.TimeUnit.MILLISECONDS);
            View content = (View) action.getParent();
            assertEquals(1f, content.getAlpha(), 0.001f);
            assertEquals(0f, content.getTranslationY(), 0.001f);
            assertTrue(action.hasFocus());
            sheet.dismiss();
        } finally {
            org.robolectric.util.ReflectionHelpers.callStaticMethod(android.animation.ValueAnimator.class, "setDurationScale",
                    org.robolectric.util.ReflectionHelpers.ClassParameter.from(float.class, 1f));
        }
    }

    @Test @Config(qualifiers = "television")
    public void tvSafeAreaStaysInsideTheSheetInsteadOfLiftingItOffTheScreen() {
        try (var controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup()) {
            ActionSheet sheet = new ActionSheet(controller.get(), "Menu");
            sheet.addAction(R.drawable.ic_settings, "Settings");
            sheet.show();
            View content = sheet.findViewById(android.R.id.content);
            assertEquals(0, content.getPaddingBottom());
            View body = org.robolectric.util.ReflectionHelpers.getField(sheet, "body");
            assertEquals(Math.round(24 * body.getResources().getDisplayMetrics().density), body.getPaddingBottom());
            sheet.dismiss();
        }
    }

    @Test @Config(sdk = 35) public void navigationInsetsReachTheMaterialSheet() {
        try (var controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup()) {
            ActionSheet sheet = new ActionSheet(controller.get(), "PC actions");
            sheet.addAction(R.drawable.ic_settings, "Last action");
            sheet.show();
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            View decor = sheet.getWindow().getDecorView();
            decor.measure(View.MeasureSpec.makeMeasureSpec(786, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(1704, View.MeasureSpec.EXACTLY));
            decor.layout(0, 0, 786, 1704);
            var insets = new androidx.core.view.WindowInsetsCompat.Builder()
                    .setInsets(androidx.core.view.WindowInsetsCompat.Type.systemBars(),
                            androidx.core.graphics.Insets.of(0, 48, 0, 96)).build();
            androidx.core.view.ViewCompat.dispatchApplyWindowInsets(decor, insets);
            View surface = sheet.findViewById(com.google.android.material.R.id.design_bottom_sheet);
            assertEquals(96, surface.getPaddingBottom());
            // Repeated dispatch must not accumulate padding.
            androidx.core.view.ViewCompat.dispatchApplyWindowInsets(decor, insets);
            assertEquals(96, surface.getPaddingBottom());
            sheet.dismiss();
        }
    }

    @Test public void controllerBackAndEscapeDismissWithoutActivatingAnAction() {
        try (var controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup()) {
            for (int key : new int[]{android.view.KeyEvent.KEYCODE_BUTTON_B, android.view.KeyEvent.KEYCODE_ESCAPE}) {
                ActionSheet sheet = new ActionSheet(controller.get(), "PC actions");
                boolean[] activated = {false};
                sheet.addAction(R.drawable.ic_settings, "Action").setOnClickListener(v -> activated[0] = true);
                sheet.show();
                sheet.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, key));
                sheet.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_UP, key));
                assertFalse(sheet.isShowing());
                assertFalse(activated[0]);
            }
        }
    }

    @Test public void longMenusHaveAnAccessibleCloseOutsideTheScrollableActions() {
        try (var controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup()) {
            var activity = controller.get();
            ActionSheet sheet = new ActionSheet(activity, "PC actions");
            var action = sheet.addAction(R.drawable.ic_settings, "A long translated menu action");
            for (int i = 0; i < 20; i++) sheet.addAction(R.drawable.ic_settings, "Action " + i);
            sheet.show();
            View close = sheet.findViewById(R.id.action_sheet_close);
            assertEquals(activity.getString(R.string.sheet_close), close.getContentDescription());
            assertNotSame(action.getParent(), close.getParent());
            assertTrue(action.getMaxLines() > 3);
            close.performClick();
            assertFalse(sheet.isShowing());
        }
    }

    @Test @Config(sdk = 30)
    public void sheetsHideTheSystemBarsOnlyOverAnImmersiveStream() {
        androidx.core.view.WindowInsetsCompat shown = new androidx.core.view.WindowInsetsCompat.Builder()
                .setVisible(androidx.core.view.WindowInsetsCompat.Type.statusBars(), true)
                .setVisible(androidx.core.view.WindowInsetsCompat.Type.navigationBars(), true)
                .setVisible(androidx.core.view.WindowInsetsCompat.Type.captionBar(), false).build();
        androidx.core.view.WindowInsetsCompat immersive = new androidx.core.view.WindowInsetsCompat.Builder()
                .setVisible(androidx.core.view.WindowInsetsCompat.Type.statusBars(), false)
                .setVisible(androidx.core.view.WindowInsetsCompat.Type.navigationBars(), false).build();
        // A phone never shows a caption bar; that must not read as "bars hidden" on the home screen.
        assertFalse(ActionSheet.hidesSystemBars(shown));
        assertTrue(ActionSheet.hidesSystemBars(immersive));
        assertFalse(ActionSheet.hidesSystemBars(null));
    }
}
