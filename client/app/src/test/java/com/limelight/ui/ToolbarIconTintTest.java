package com.limelight.ui;

import android.app.Application;
import android.content.Intent;
import android.content.res.ColorStateList;
import android.os.Bundle;
import android.view.Menu;
import android.view.MenuItem;
import android.widget.FrameLayout;

import androidx.appcompat.app.AppCompatActivity;
import androidx.core.content.ContextCompat;
import androidx.core.graphics.ColorUtils;
import androidx.core.internal.view.SupportMenuItem;

import com.google.android.material.appbar.MaterialToolbar;
import com.google.android.material.button.MaterialButton;
import com.google.android.material.color.MaterialColors;
import com.limelight.R;
import com.limelight.preferences.HostStreamSettings;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

/**
 * The toolbar vector icons are drawn white. They must be tinted with the theme's on-surface colour, otherwise they
 * vanish on the light theme's near-white surface (and stay readable on the dark theme).
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ToolbarIconTintTest {
    private static final double MINIMUM_ICON_CONTRAST = 4.5;

    public static class Screen extends AppCompatActivity {
        @Override protected void onCreate(Bundle state) {
            setTheme(R.style.AppTheme);
            super.onCreate(state);
            setContentView(new FrameLayout(this));
        }
    }

    private static void assertIconsFollowTheTheme(AppCompatActivity screen, Menu menu, int... itemIds) {
        int onSurface = ContextCompat.getColor(screen, R.color.on_surface);
        int surface = ContextCompat.getColor(screen, R.color.surface);
        for (int id : itemIds) {
            MenuItem item = menu.findItem(id);
            assertNotNull("menu item " + id, item);
            assertNotNull("menu item " + id + " has an icon", item.getIcon());
            ColorStateList tint = ((SupportMenuItem) item).getIconTintList();
            assertNotNull("menu item " + id + " must tint its white vector icon", tint);
            assertEquals("menu item " + id + " uses colorOnSurface", onSurface, tint.getDefaultColor());
            assertTrue("menu item " + id + " is readable on the toolbar surface",
                    ColorUtils.calculateContrast(tint.getDefaultColor(), surface) >= MINIMUM_ICON_CONTRAST);
        }
    }

    private static void assertComputersToolbar() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup().visible();
        try {
            Screen screen = controller.get();
            screen.setContentView(R.layout.activity_pc_view);
            ShellBar.install(screen, ShellBar.Destination.PCS);
            int surface = MaterialColors.getColor(screen, com.google.android.material.R.attr.colorSurface, 0);
            for (int id : new int[] {R.id.shell_home, R.id.shell_pcs, R.id.shell_settings}) {
                MaterialButton button = screen.findViewById(id);
                assertNotNull("shell button " + id + " must tint its white vector icon", button.getIconTint());
                // Unselected buttons sit on the surface; the selected one on its container, checked by the style.
                int tint = button.getIconTint().getColorForState(new int[0], 0);
                assertTrue("shell icon " + id + " is readable on the surface",
                        ColorUtils.calculateContrast(tint, surface) >= MINIMUM_ICON_CONTRAST);
            }
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static void assertHostProfileToolbar() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), HostStreamSettings.class)
                .putExtra("host_uuid", "host-uuid").putExtra("host_name", "PC");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            HostStreamSettings screen = controller.get();
            MaterialToolbar toolbar = screen.findViewById(R.id.host_profile_toolbar);
            assertIconsFollowTheTheme(screen, toolbar.getMenu(), R.id.hostProfileHelp);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "notnight")
    public void computersToolbarIconsAreDarkOnTheLightTheme() {
        assertComputersToolbar();
    }

    @Test
    @Config(qualifiers = "night")
    public void computersToolbarIconsAreLightOnTheDarkTheme() {
        assertComputersToolbar();
    }

    @Test
    @Config(qualifiers = "notnight")
    public void hostProfileToolbarIconsAreDarkOnTheLightTheme() {
        assertHostProfileToolbar();
    }

    @Test
    @Config(qualifiers = "night")
    public void hostProfileToolbarIconsAreLightOnTheDarkTheme() {
        assertHostProfileToolbar();
    }
}
