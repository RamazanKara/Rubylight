package com.limelight.ui;

import android.app.Application;
import android.os.Bundle;
import android.preference.PreferenceManager;
import android.view.View;
import android.widget.FrameLayout;

import com.google.android.material.color.MaterialColors;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.utils.UiHelper;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 30, qualifiers = "notnight", application = Application.class)
public class AppThemeTest {
    public static class Screen extends NightjarActivity {
        int recreationRequests;
        @Override protected void onCreate(Bundle state) {
            super.onCreate(state);
            setContentView(new FrameLayout(this));
            UiHelper.notifyNewRootView(this);
        }
        @Override public void recreate() { recreationRequests++; }
    }

    private static void black(boolean enabled) {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication())
                .edit().putBoolean(AppTheme.OLED_BLACK, enabled).commit();
    }

    @Test
    public void directEntryAndDialogsAreBlackEvenInSystemLightMode() {
        black(true);
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup();
        try {
            Screen screen = controller.get();
            assertEquals(0xff000000, MaterialColors.getColor(screen,
                    com.google.android.material.R.attr.colorSurface, 0));
            assertEquals(0, screen.getWindow().getDecorView().getSystemUiVisibility() &
                    (View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR | View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR));
            androidx.appcompat.app.AlertDialog dialog = new MaterialAlertDialogBuilder(screen)
                    .setTitle("OLED").setMessage("Dialog").create();
            assertEquals(0xff000000, MaterialColors.getColor(dialog.getContext(),
                    com.google.android.material.R.attr.colorSurfaceContainerHigh, 0));
            ActionSheet sheet = new ActionSheet(screen, "Menu");
            assertEquals(0xff000000, MaterialColors.getColor(sheet.getContext(),
                    com.google.android.material.R.attr.colorSurface, 0));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void alwaysDarkUsesTheGreyNightPaletteInSystemLightMode() {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication())
                .edit().putBoolean(AppTheme.ALWAYS_DARK, true).commit();
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup();
        try {
            Screen screen = controller.get();
            int surface = MaterialColors.getColor(screen, com.google.android.material.R.attr.colorSurface, 0);
            assertEquals(screen.getColor(com.limelight.R.color.night_surface), surface);
            assertNotEquals(0xff000000, surface);
            assertEquals(0, screen.getWindow().getDecorView().getSystemUiVisibility() &
                    (View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR | View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR));
            PreferenceManager.getDefaultSharedPreferences(screen).edit().putBoolean(AppTheme.ALWAYS_DARK, false).commit();
            assertEquals(1, screen.recreationRequests);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void blackDialogsAndSheetsGetAnOutline() {
        black(true);
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup();
        try {
            Screen screen = controller.get();
            NavigationDialogBuilder builder = new NavigationDialogBuilder(screen);
            builder.setTitle("Info").setMessage("Dialog").create();
            com.google.android.material.shape.MaterialShapeDrawable background =
                    (com.google.android.material.shape.MaterialShapeDrawable) builder.getBackground();
            assertTrue(background.getStrokeWidth() > 0);
            assertEquals(MaterialColors.getColor(screen, com.google.android.material.R.attr.colorOutlineVariant, 0),
                    background.getStrokeColor().getDefaultColor());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void changesRefreshTheVisibleScreenAndStoppedScreensOnReturn() {
        black(false);
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup();
        try {
            Screen screen = controller.get();
            black(true);
            assertEquals(1, screen.recreationRequests);
            controller.recreate();
            assertEquals(0xff000000, MaterialColors.getColor(controller.get(),
                    com.google.android.material.R.attr.colorSurface, 0));
            controller.pause().stop();
            black(false);
            assertEquals(0, controller.get().recreationRequests);
            controller.start().resume();
            assertEquals(1, controller.get().recreationRequests);
            controller.recreate();
            assertNotEquals(0xff000000, MaterialColors.getColor(controller.get(),
                    com.google.android.material.R.attr.colorSurface, 0));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void resettingPreferencesRefreshesTheCurrentScreen() {
        black(true);
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup();
        try {
            PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
            assertFalse(AppTheme.isOledBlack(controller.get()));
            assertEquals(1, controller.get().recreationRequests);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(sdk = 23)
    public void olderAndroidKeepsPreferenceTextReadableAndNavigationBarBlack() {
        black(true);
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup();
        try {
            Screen screen = controller.get();
            android.view.View row = screen.getLayoutInflater().inflate(com.limelight.R.layout.settings_preference, null);
            assertEquals(MaterialColors.getColor(screen, com.google.android.material.R.attr.colorOnSurface, 0),
                    ((android.widget.TextView) row.findViewById(android.R.id.title)).getCurrentTextColor());
            assertEquals(android.graphics.Color.TRANSPARENT, screen.getWindow().getStatusBarColor());
            assertEquals(0xff000000, screen.getWindow().getNavigationBarColor());
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
