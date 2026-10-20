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
