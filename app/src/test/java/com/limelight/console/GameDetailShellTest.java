package com.limelight.console;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Build;
import android.preference.PreferenceManager;
import android.view.View;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.StreamSettings;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.io.File;

/** A game's page under the shell bar: it belongs to Home, so Home in the bar is the page it came from. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameDetailShellTest {
    public static class Detail extends GameDetailActivity {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }
    }

    private Context context;

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        File directory = new File(context.getFilesDir(), "console-library");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        assertTrue(new File(directory, "tower.json").createNewFile());
    }

    @After
    public void tearDown() {
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    private ActivityController<Detail> detail() {
        Intent intent = new Intent(context, Detail.class)
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, "tower")
                .putExtra(GameDetailActivity.EXTRA_APP_ID, 11)
                .putExtra(GameDetailActivity.EXTRA_TITLE, "Portal")
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, "Tower");
        return Robolectric.buildActivity(Detail.class, intent).setup().visible();
    }

    @Test
    public void theBarListsHomeAsTheCurrentDestination() {
        ActivityController<Detail> controller = detail();
        try {
            Detail page = controller.get();
            assertEquals(View.VISIBLE, page.findViewById(R.id.shell_home).getVisibility());
            assertTrue(page.findViewById(R.id.shell_home).isSelected());
            assertFalse(page.findViewById(R.id.shell_pcs).isSelected());
            assertFalse(page.findViewById(R.id.shell_settings).isSelected());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void homeInTheBarClosesThePageInsteadOfStackingAnotherHome() {
        ActivityController<Detail> controller = detail();
        try {
            Detail page = controller.get();
            page.findViewById(R.id.shell_home).performClick();
            assertTrue(page.isFinishing());
            assertNull("Home is already underneath", shadowOf(page).getNextStartedActivity());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void settingsFromThePageIsTheSettingsTheBarOpensAnywhere() {
        ActivityController<Detail> controller = detail();
        try {
            Detail page = controller.get();
            page.findViewById(R.id.shell_settings).performClick();
            Intent settings = shadowOf(page).getNextStartedActivity();
            assertEquals(StreamSettings.class.getName(), settings.getComponent().getClassName());
            assertTrue(settings.getBooleanExtra(com.limelight.ui.ShellBar.EXTRA_FROM_SHELL, false));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void onTelevisionTheDpadWalksTheBarThenPlayThenTheStar() {
        ActivityController<Detail> controller = detail();
        try {
            Detail page = controller.get();
            View play = page.findViewById(R.id.detail_play);
            play.setEnabled(true);
            View home = page.findViewById(R.id.shell_home);
            View pcs = page.findViewById(R.id.shell_pcs);
            View settings = page.findViewById(R.id.shell_settings);
            assertSame(pcs, HomeShellTest.focusAfter(page, home, View.FOCUS_RIGHT));
            assertSame(settings, HomeShellTest.focusAfter(page, pcs, View.FOCUS_RIGHT));
            View below = HomeShellTest.focusAfter(page, home, View.FOCUS_DOWN);
            assertSame("down from the bar reaches Play first", play, below);
            assertSame("then the star", page.findViewById(R.id.detail_favourite),
                    HomeShellTest.focusAfter(page, play, View.FOCUS_DOWN));
            assertSame("and back up", play, HomeShellTest.focusAfter(page, page.findViewById(R.id.detail_favourite), View.FOCUS_UP));
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
