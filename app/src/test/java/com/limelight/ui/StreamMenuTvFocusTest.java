package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.view.View;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static com.limelight.ui.StreamMenu.Item.*;
import static org.junit.Assert.*;

/**
 * nightjar#52 on a 1080p TV: Back opens the stream menu with focus on its first toggle, one press down reaches
 * Disconnect, and Back again closes the menu without leaving. Set NIGHTJAR_PREVIEW_OUTPUT to capture the sheet.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w960dp-h540dp-land-television-xhdpi")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class StreamMenuTvFocusTest {
    @Test
    public void oneDownFromTheFirstToggleIsDisconnect() throws Exception {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            ComputerDetails host = TestHosts.details("rubylight-serverinfo.xml");
            StreamMenu.Options state = new StreamMenu.Options(NvApp.Role.STREAM, host);
            state.webUiAddress = true;
            state.hostMenu = true;
            List<StreamMenu.Item> chosen = new ArrayList<>();
            StreamMenu menu = new StreamMenu(activity, () -> state, chosen::add);
            menu.show();
            ShadowLooper.idleMainLooper(100, TimeUnit.MILLISECONDS);
            // A remote's press leaves the sheet's own window in focus mode before its first focus lands.
            Object root = ReflectionHelpers.callInstanceMethod(menu.sheet().getWindow().getDecorView(), "getViewRootImpl");
            ReflectionHelpers.callInstanceMethod(root, "ensureTouchMode", ReflectionHelpers.ClassParameter.from(boolean.class, false));
            layOut(menu);
            // What the sheet does once it is attached, or a page is shown, outside touch mode.
            menu.sheet().focusFirstAction();
            View focused = menu.sheet().getWindow().getDecorView().findFocus();
            assertSame("focus starts on the first toggle, not on Disconnect", menu.row(KEYBOARD), focused);

            View next = focused.focusSearch(View.FOCUS_DOWN);
            assertSame("one press down is Disconnect", menu.row(DISCONNECT), next);
            assertTrue(next.requestFocus(View.FOCUS_DOWN));
            ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
            String output = System.getenv("NIGHTJAR_PREVIEW_OUTPUT");
            if (output != null && !output.isBlank()) {
                File directory = new File(output, "day");
                assertTrue(directory.isDirectory() || directory.mkdirs());
                menu.row(DISCONNECT).jumpDrawablesToCurrentState();
                TvScreensPreviewTest.captureDialog(activity, menu.sheet(), new File(directory, "tv-stream-menu-disconnect.png"));
            }
            assertSame("Reconnect follows", menu.row(RECONNECT), next.focusSearch(View.FOCUS_DOWN));

            menu.sheet().onBackPressed();
            assertFalse(menu.sheet().isShowing());
            assertTrue("Back closes the menu and nothing else", chosen.isEmpty());
        }
    }

    // The sheet's window draws no frames here: measure and lay it out at 1080p so focus search sees the rows.
    private static void layOut(StreamMenu menu) {
        ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
        View sheet = menu.sheet().getWindow().getDecorView();
        sheet.measure(View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.AT_MOST),
                View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.AT_MOST));
        sheet.layout(0, 0, sheet.getMeasuredWidth(), sheet.getMeasuredHeight());
        ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
    }
}
