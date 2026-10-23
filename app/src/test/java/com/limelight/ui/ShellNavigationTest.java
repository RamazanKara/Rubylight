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

    private ActivityController<StreamSettings> settingsFromTheBar() {
        return Robolectric.buildActivity(StreamSettings.class, ShellBar.intentFor(context, Destination.SETTINGS))
                .setup().visible();
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
        ActivityController<StreamSettings> controller = settingsFromTheBar();
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
        ActivityController<StreamSettings> controller = settingsFromTheBar();
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
        ActivityController<StreamSettings> controller = settingsFromTheBar();
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

    @Test
    public void settingsOpenedFromTheBarCarriesTheMarkerAndOthersDoNot() {
        assertTrue(ShellBar.intentFor(context, Destination.SETTINGS).getBooleanExtra(ShellBar.EXTRA_FROM_SHELL, false));
        assertFalse(ShellBar.intentFor(context, Destination.PCS).hasExtra(ShellBar.EXTRA_FROM_SHELL));
        assertFalse(ShellBar.intentFor(context, Destination.HOME).hasExtra(ShellBar.EXTRA_FROM_SHELL));
    }

    @Test
    public void ctrlCommaOpensSettingsWithoutTheMarkerAndBackJustFinishesIt() throws Exception {
        cacheALibrary();
        ActivityController<PcViewLifecycleTest.Library> from = pcs();
        try {
            // PCs is not the root here (Home is), so a shell Back would replace it; Back must only close Settings.
            assertTrue(UiNavigation.handleKey(from.get(), new android.view.KeyEvent(0, 0, android.view.KeyEvent.ACTION_UP,
                    android.view.KeyEvent.KEYCODE_COMMA, 0, android.view.KeyEvent.META_CTRL_ON)));
            Intent open = nextStarted(from.get());
            assertEquals(StreamSettings.class.getName(), open.getComponent().getClassName());
            assertFalse(open.getBooleanExtra(ShellBar.EXTRA_FROM_SHELL, false));

            ActivityController<StreamSettings> settings = Robolectric.buildActivity(StreamSettings.class, open).setup().visible();
            try {
                settings.get().onBackPressed();
                assertNull("no screen is started; the one underneath is shown again", nextStarted(settings.get()));
                assertTrue(settings.get().isFinishing());
            } finally {
                settings.pause().stop().destroy();
            }
        } finally {
            from.pause().stop().destroy();
        }
    }

    @Test
    public void homeJoinsTheBarAndBecomesTheRootWhenTheFirstLibraryIsSeeded() throws Exception {
        ActivityController<PcViewLifecycleTest.Library> controller = pcs();
        try {
            PcViewLifecycleTest.Library screen = controller.get();
            View home = screen.findViewById(R.id.shell_home);
            assertEquals(View.GONE, home.getVisibility());
            assertEquals(Destination.PCS, ShellBar.root(context));

            cacheALibrary();
            org.robolectric.util.ReflectionHelpers.callInstanceMethod(screen, "onLibrarySeeded");
            assertEquals("Home shows without waiting for a resume", View.VISIBLE, home.getVisibility());
            assertEquals(Destination.HOME, ShellBar.root(context));

            screen.getOnBackPressedDispatcher().onBackPressed();
            Intent root = nextStarted(screen);
            assertEquals("Back now goes to Home", ConsoleHomeActivity.class.getName(), root.getComponent().getClassName());
            assertRootFlags(root);
            assertTrue(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void theBackCallbackIsEnabledAgainAfterHandingTheRootPressToTheActivity() {
        ActivityController<PcViewLifecycleTest.Library> controller = pcs();
        try {
            PcViewLifecycleTest.Library screen = controller.get();
            screen.getOnBackPressedDispatcher().onBackPressed();
            assertTrue(screen.isFinishing());
            assertTrue("the shell takes Back over again for the next press",
                    screen.getOnBackPressedDispatcher().hasEnabledCallbacks());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void settingsRestartsTheTaskOnANewRootWhenItChangedWhileInSettings() throws Exception {
        for (boolean fromBar : new boolean[] {true, false}) {
            File library = new File(context.getFilesDir(), "console-library");
            if (library.exists()) new File(library, "tower.json").delete();
            Intent intent = fromBar ? ShellBar.intentFor(context, Destination.SETTINGS) : new Intent(context, StreamSettings.class);
            ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class, intent).setup().visible();
            try {
                assertEquals(Destination.PCS, ShellBar.root(context));
                cacheALibrary();
                assertEquals(Destination.HOME, ShellBar.root(context));
                controller.get().onBackPressed();
                Intent root = nextStarted(controller.get());
                assertEquals(ConsoleHomeActivity.class.getName(), root.getComponent().getClassName());
                assertEquals(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK,
                        root.getFlags() & (Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK));
                assertTrue(controller.get().isFinishing());
            } finally {
                controller.pause().stop().destroy();
            }
        }
    }

    @Test
    public void upFromTheSettingsListLeavesTheSearchLinkOnlyWhileSearchIsShown() {
        ActivityController<StreamSettings> controller = settingsFromTheBar();
        try {
            StreamSettings screen = controller.get();
            android.widget.ListView list = screen.findViewById(android.R.id.list);
            assertNotNull(list);
            assertEquals("hidden search is not a focus target", View.NO_ID, list.getNextFocusUpId());
            screen.openSearch();
            assertEquals(R.id.settings_search, list.getNextFocusUpId());
            screen.onBackPressed();
            assertEquals(View.GONE, screen.findViewById(R.id.settings_search).getVisibility());
            assertEquals(View.NO_ID, list.getNextFocusUpId());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void dpadCenterOnTheHelpRowOfTheAppSectionOpensHelp() {
        Intent intent = new Intent(context, StreamSettings.class).putExtra(StreamSettings.EXTRA_SECTION, "app");
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class, intent).setup().visible();
        try {
            StreamSettings screen = controller.get();
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            android.widget.ListView list = screen.findViewById(android.R.id.list);
            int help = -1;
            for (int i = 0; i < list.getAdapter().getCount(); i++) {
                Object item = list.getAdapter().getItem(i);
                if (item instanceof android.preference.Preference && "help".equals(((android.preference.Preference) item).getKey())) help = i;
            }
            assertTrue("the App section lists Help", help >= 0);
            list.requestFocus();
            list.setSelection(help);
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertTrue(list.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_CENTER)));
            assertTrue(list.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_UP, android.view.KeyEvent.KEYCODE_DPAD_CENTER)));
            assertEquals(com.limelight.HelpTopicsActivity.class.getName(), nextStarted(screen).getComponent().getClassName());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void theFirstPcTakesFocusFromTheShellBarOnTelevision() {
        ActivityController<PcViewLifecycleTest.Library> controller = pcs();
        try {
            PcViewLifecycleTest.Library screen = controller.get();
            android.widget.GridView grid = screen.findViewById(R.id.fragmentView);
            assertNotNull(grid);
            // The empty state held focus; whatever took it over, the shell bar must not keep it.
            screen.findViewById(R.id.shell_settings).requestFocus();
            assertTrue(screen.findViewById(R.id.shell_settings).hasFocus());

            com.limelight.nvstream.http.ComputerDetails computer = new com.limelight.nvstream.http.ComputerDetails();
            computer.uuid = "host-uuid";
            computer.name = "PC";
            computer.state = com.limelight.nvstream.http.ComputerDetails.State.ONLINE;
            computer.pairState = com.limelight.nvstream.http.PairingManager.PairState.PAIRED;
            computer.activeAddress = new com.limelight.nvstream.http.ComputerDetails.AddressTuple("127.0.0.1", 47989);
            org.robolectric.util.ReflectionHelpers.callInstanceMethod(screen, "updateComputer",
                    org.robolectric.util.ReflectionHelpers.ClassParameter.from(com.limelight.nvstream.http.ComputerDetails.class, computer));
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertTrue("the list has focus, not the shell bar", grid.hasFocus());
            assertFalse(screen.findViewById(R.id.shell_settings).hasFocus());
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
