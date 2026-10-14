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
}
