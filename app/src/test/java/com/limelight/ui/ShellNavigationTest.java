package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.os.Build;
import android.preference.PreferenceManager;
import android.view.View;
import android.view.ViewGroup;
import android.widget.LinearLayout;

import com.limelight.PcView;
import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.console.ConsoleHomeActivity;
import com.limelight.console.HomeSettings;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.StreamSettings;
import com.limelight.ui.ShellBar.Destination;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.io.File;
import java.util.ArrayList;
import java.util.List;

/** Home, PCs and Settings: the bar, which one is current, and where each destination and Back lead. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class ShellNavigationTest {
    private Context context;

    @Before
    public void setUp() {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        context.getSharedPreferences("FirstRun", 0).edit().putBoolean("pairing_guide_shown", true).commit();
    }

    /** What a host's refresh leaves behind once it has shown its games. */
    private void cacheALibrary() throws Exception {
        File directory = new File(context.getFilesDir(), "console-library");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        assertTrue(new File(directory, "tower.json").createNewFile());
    }

    private void turnHomeOff() {
        PreferenceManager.getDefaultSharedPreferences(context).edit().putBoolean(HomeSettings.PREF_KEY, false).commit();
    }

    private ActivityController<PcViewLifecycleTest.Library> pcs() {
        // Not the launcher intent, so a start with a library stays on this screen instead of forwarding to Home.
        return Robolectric.buildActivity(PcViewLifecycleTest.Library.class, HomeSettings.pcList(context)).setup().visible();
    }

    private static List<Integer> visibleButtons(Context screen, ViewGroup bar) {
        List<Integer> ids = new ArrayList<>();
        LinearLayout items = bar.findViewById(R.id.shell_bar_items);
        for (int i = 0; i < items.getChildCount(); i++) {
            if (items.getChildAt(i).getVisibility() == View.VISIBLE) ids.add(items.getChildAt(i).getId());
        }
        return ids;
    }

    private static List<Integer> selectedButtons(ViewGroup bar) {
        List<Integer> ids = new ArrayList<>();
        LinearLayout items = bar.findViewById(R.id.shell_bar_items);
        for (int i = 0; i < items.getChildCount(); i++) {
            if (items.getChildAt(i).isSelected()) ids.add(items.getChildAt(i).getId());
        }
        return ids;
    }

    private static Intent nextStarted(android.app.Activity activity) {
        return shadowOf(activity).getNextStartedActivity();
    }

    private static void assertRootFlags(Intent intent) {
        assertEquals(Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP,
                intent.getFlags() & (Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP));
    }

    @Test
    public void theRootIsHomeOnlyWhenTheAppOpensOnIt() throws Exception {
        assertEquals("no library yet", Destination.PCS, ShellBar.root(context));
        assertFalse(ShellBar.homeAvailable(context));
        cacheALibrary();
        assertEquals(Destination.HOME, ShellBar.root(context));
        assertTrue(ShellBar.homeAvailable(context));
        turnHomeOff();
        assertEquals("the setting turns Home off as the launch screen", Destination.PCS, ShellBar.root(context));
        assertTrue("but its games are still one tap away", ShellBar.homeAvailable(context));
    }

    @Test
    public void thePcsScreenListsThePcsDestinationAsCurrentAndHidesHomeWithoutALibrary() {
        ActivityController<PcViewLifecycleTest.Library> controller = pcs();
        try {
            ViewGroup bar = controller.get().findViewById(R.id.shell_bar);
            assertNotNull(bar);
            assertEquals(java.util.Arrays.asList(R.id.shell_pcs, R.id.shell_settings), visibleButtons(controller.get(), bar));
            assertEquals(java.util.Collections.singletonList(R.id.shell_pcs), selectedButtons(bar));
            assertEquals(controller.get().getString(R.string.library_computers),
                    ((com.google.android.material.appbar.MaterialToolbar) controller.get().findViewById(R.id.home_toolbar))
                            .getTitle().toString());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void theBarOrdersHomeThenPcsThenSettingsOnceThereIsALibrary() throws Exception {
        cacheALibrary();
        ActivityController<PcViewLifecycleTest.Library> controller = pcs();
        try {
            ViewGroup bar = controller.get().findViewById(R.id.shell_bar);
            assertEquals(java.util.Arrays.asList(R.id.shell_home, R.id.shell_pcs, R.id.shell_settings),
                    visibleButtons(controller.get(), bar));
            assertEquals(controller.get().getString(R.string.console_title),
                    ((android.widget.TextView) bar.findViewById(R.id.shell_home)).getText().toString());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void homeAppearsWhenALibraryArrivesWhileTheScreenIsAway() throws Exception {
        ActivityController<PcViewLifecycleTest.Library> controller = pcs();
        try {
            View home = controller.get().findViewById(R.id.shell_home);
            assertEquals(View.GONE, home.getVisibility());
            controller.pause();
            cacheALibrary();
            controller.resume();
            assertEquals(View.VISIBLE, home.getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void thePcsRootKeepsItselfUnderSettingsAndSettingsGoesBackToIt() {
        ActivityController<PcViewLifecycleTest.Library> controller = pcs();
        try {
            PcViewLifecycleTest.Library screen = controller.get();
            screen.findViewById(R.id.shell_settings).performClick();
            Intent settings = nextStarted(screen);
            assertEquals(StreamSettings.class.getName(), settings.getComponent().getClassName());
            assertNull("no extras opens the root", settings.getStringExtra(StreamSettings.EXTRA_SECTION));
            assertFalse("the root stays under what it opens", screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void aScreenThatIsNotTheRootMakesWayForItsSibling() throws Exception {
        cacheALibrary();
        ActivityController<PcViewLifecycleTest.Library> controller = pcs();
        try {
            PcViewLifecycleTest.Library screen = controller.get();
            screen.findViewById(R.id.shell_settings).performClick();
            assertEquals(StreamSettings.class.getName(), nextStarted(screen).getComponent().getClassName());
            assertTrue("PCs is not the root now, so Settings replaces it", screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void homeIsBroughtBackInsteadOfStacked() throws Exception {
        cacheALibrary();
        ActivityController<PcViewLifecycleTest.Library> controller = pcs();
        try {
            PcViewLifecycleTest.Library screen = controller.get();
            screen.findViewById(R.id.shell_home).performClick();
            Intent home = nextStarted(screen);
            assertEquals(ConsoleHomeActivity.class.getName(), home.getComponent().getClassName());
            assertRootFlags(home);
            assertTrue(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void pcsIsTheRootWhenHomeIsOffAndSettingsAndHomeStackOnIt() throws Exception {
        cacheALibrary();
        turnHomeOff();
        ActivityController<PcViewLifecycleTest.Library> controller = pcs();
        try {
            PcViewLifecycleTest.Library screen = controller.get();
            screen.findViewById(R.id.shell_home).performClick();
            Intent home = nextStarted(screen);
            assertEquals(ConsoleHomeActivity.class.getName(), home.getComponent().getClassName());
            assertEquals("Home is not the root, so it does not clear the task", 0,
                    home.getFlags() & Intent.FLAG_ACTIVITY_CLEAR_TOP);
            assertFalse(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void backFromPcsGoesToHomeWhenHomeIsTheRoot() throws Exception {
        cacheALibrary();
        ActivityController<PcViewLifecycleTest.Library> controller = pcs();
        try {
            PcViewLifecycleTest.Library screen = controller.get();
            screen.getOnBackPressedDispatcher().onBackPressed();
            Intent home = nextStarted(screen);
            assertEquals(ConsoleHomeActivity.class.getName(), home.getComponent().getClassName());
            assertRootFlags(home);
            assertTrue(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void backOnTheRootLeavesWithoutStartingAnything() {
        ActivityController<PcViewLifecycleTest.Library> controller = pcs();
        try {
            PcViewLifecycleTest.Library screen = controller.get();
            screen.getOnBackPressedDispatcher().onBackPressed();
            assertNull(nextStarted(screen));
            assertTrue(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void settingsShowsTheBarOnTheListOnlyAndBackLeavesToThePcsRoot() {
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup().visible();
        try {
            StreamSettings screen = controller.get();
            View bar = screen.findViewById(R.id.shell_bar);
            assertEquals(View.VISIBLE, bar.getVisibility());
            assertEquals(java.util.Collections.singletonList(R.id.shell_settings), selectedButtons((ViewGroup) bar));

            org.robolectric.util.ReflectionHelpers.callInstanceMethod(screen, "showSection",
                    org.robolectric.util.ReflectionHelpers.ClassParameter.from(String.class, "audio"));
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals("a section keeps the back arrow and its title", View.GONE, bar.getVisibility());
            screen.onBackPressed();
            assertNull("Back in a section returns to the list", nextStarted(screen));
            assertFalse(screen.isFinishing());
            assertEquals(View.VISIBLE, bar.getVisibility());

            screen.onBackPressed();
            Intent root = nextStarted(screen);
            assertEquals(PcView.class.getName(), root.getComponent().getClassName());
            assertRootFlags(root);
            assertTrue(root.getBooleanExtra(HomeSettings.EXTRA_STAY, false));
            assertTrue(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void backFromSettingsGoesToHomeWhenHomeIsTheRoot() throws Exception {
        cacheALibrary();
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup().visible();
        try {
            controller.get().onBackPressed();
            Intent root = nextStarted(controller.get());
            assertEquals(ConsoleHomeActivity.class.getName(), root.getComponent().getClassName());
            assertRootFlags(root);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void settingsOffersPcsAndHomeFromItsBar() throws Exception {
        cacheALibrary();
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup().visible();
        try {
            StreamSettings screen = controller.get();
            screen.findViewById(R.id.shell_pcs).performClick();
            Intent pcs = nextStarted(screen);
            assertEquals(PcView.class.getName(), pcs.getComponent().getClassName());
            assertTrue("it must not forward back to Home", pcs.getBooleanExtra(HomeSettings.EXTRA_STAY, false));
            assertTrue(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static void assertOverscan(Context context, ViewGroup column, int horizontalDp, int verticalDp) {
        float density = context.getResources().getDisplayMetrics().density;
        assertEquals(horizontalDp * density, context.getResources().getDimension(R.dimen.shell_overscan_horizontal), 0.5f);
        assertEquals(verticalDp * density, context.getResources().getDimension(R.dimen.shell_overscan_vertical), 0.5f);
        assertEquals(Math.round(horizontalDp * density), column.getPaddingLeft());
        assertEquals(Math.round(horizontalDp * density), column.getPaddingRight());
        assertEquals(Math.round(verticalDp * density), column.getPaddingTop());
        assertEquals(Math.round(verticalDp * density), column.getPaddingBottom());
    }

    @Test
    public void shellScreensAddNoOverscanMarginOffTelevision() {
        ActivityController<PcViewLifecycleTest.Library> controller = pcs();
        try {
            assertOverscan(context, (ViewGroup) controller.get().findViewById(R.id.home_toolbar).getParent(), 0, 0);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void shellScreensKeepOverscanMarginsOnTelevision() {
        ActivityController<PcViewLifecycleTest.Library> pcs = pcs();
        ActivityController<StreamSettings> settings = Robolectric.buildActivity(StreamSettings.class).setup().visible();
        try {
            assertOverscan(context, (ViewGroup) pcs.get().findViewById(R.id.home_toolbar).getParent(), 48, 27);
            assertOverscan(context, (ViewGroup) settings.get().findViewById(R.id.settings_toolbar).getParent(), 48, 27);
        } finally {
            pcs.pause().stop().destroy();
            settings.pause().stop().destroy();
        }
    }
}
