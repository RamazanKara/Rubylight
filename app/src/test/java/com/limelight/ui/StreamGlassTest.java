package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.graphics.Color;
import android.view.ContextThemeWrapper;
import android.view.View;
import android.view.WindowManager;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

import static org.junit.Assert.*;

/**
 * Everything over a stream wears the stream menu's glass: a dialog the menu's, without a dim, and what stays up while the
 * game plays (the statistics, the keyboards) the HUD's, see-through and never over a blur.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w960dp-h540dp-land-xhdpi")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class StreamGlassTest {
    @After
    public void forget() {
        MenuBlurWindow.enabledForTest = null;
    }

    @Test
    public void theStreamsThemeSaysItIsOverAStream() {
        Application app = RuntimeEnvironment.getApplication();
        assertTrue(StreamGlass.overStream(new ContextThemeWrapper(app, R.style.StreamTheme)));
        assertFalse(StreamGlass.overStream(new ContextThemeWrapper(app, R.style.AppTheme)));
    }

    @Test
    public void aDialogOverAStreamWearsTheMenusGlassWithoutADim() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            MenuBlurWindow.enabledForTest = false;
            AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle("Touch mode").setMessage("Trackpad").create();
            StreamGlass.show(dialog);
            View decor = dialog.getWindow().getDecorView();
            GlassPanelDrawable glass = StreamGlass.glassOf(decor.getBackground());
            assertNotNull("the stream menu's glass", glass);
            assertEquals("no blur here: as opaque as the menu without one", 0xFA, Color.alpha(glass.fillColor()));
            assertEquals("the game stays undimmed around it", 0,
                    dialog.getWindow().getAttributes().flags & WindowManager.LayoutParams.FLAG_DIM_BEHIND);
            dialog.dismiss();
        }
    }

    @Test
    public void aDialogInTheAppKeepsItsOwnGlassAndDim() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.AppTheme);
            AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle("Pair").create();
            StreamGlass.show(dialog);
            assertNull(StreamGlass.glassOf(dialog.getWindow().getDecorView().getBackground()));
            assertNotEquals(0, dialog.getWindow().getAttributes().flags & WindowManager.LayoutParams.FLAG_DIM_BEHIND);
            dialog.dismiss();
        }
    }

    @Test
    public void theHudIsSeeThroughAndNeverOverABlur() {
        Activity activity = Robolectric.buildActivity(Activity.class).setup().get();
        activity.setTheme(R.style.StreamTheme);
        GlassSpec hud = GlassSpec.hud(activity);
        assertEquals("a blur under it would compose every stream frame on the GPU", 0, hud.blur, 0);
        GlassPanelDrawable panel = GlassPanelDrawable.hud(activity, new float[8]);
        int fill = Color.alpha(panel.fillColor());
        assertTrue("lets the game through", fill < 0xE0);
        assertTrue("still a panel behind the text", fill >= 0xB0);
        panel.setFillOpacity(0.5f);
        assertEquals("the user's opacity scales the fill only", fill / 2, Color.alpha(panel.fillColor()), 1);
    }
}
