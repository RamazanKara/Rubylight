package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.view.View;
import android.widget.TextView;
import com.limelight.AppViewLifecycleTest.Library;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import java.util.Collections;
import java.util.List;
import static com.limelight.AppViewLifecycleTest.idle;
import static com.limelight.AppViewLifecycleTest.libraryWithRunningGame;
import static com.limelight.AppViewLifecycleTest.openGameSheet;
import static com.limelight.AppViewLifecycleTest.position;
import static com.limelight.AppViewLifecycleTest.runningLibrary;
import static com.limelight.AppViewLifecycleTest.sheetFor;
import static com.limelight.AppViewLifecycleTest.sheetRow;
import static org.junit.Assert.*;

/**
 * Robolectric tests for {@link AppView}'s game action sheet, profile picker integration, card
 * entrance motion, running-game replacement flow, and phone/TV chrome and focus layout.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class AppViewGridUiTest {
    @Test public void theGameSheetOffersNoProfileRowWithoutProfiles() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            android.app.Dialog sheet = openGameSheet(grid, "Desktop");
            assertNull(sheetRow(sheet, screen.getString(R.string.profiles_subtitle)));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test public void theGameSheetShowsTheProfileInUseAndPickingOneSetsIt() {
        android.content.Context app = RuntimeEnvironment.getApplication();
        String home = com.limelight.preferences.StreamProfiles.create(app, "Home", new com.limelight.preferences.HostStreamProfile());
        String hotel = com.limelight.preferences.StreamProfiles.create(app, "Hotel", new com.limelight.preferences.HostStreamProfile());
        com.limelight.preferences.StreamProfiles.setActive(app, home);
        Intent intent = new Intent(app, Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            android.app.Dialog sheet = openGameSheet(grid, "Desktop");
            com.google.android.material.button.MaterialButton row = sheetRow(sheet, screen.getString(R.string.profiles_subtitle));
            assertNotNull(row);
            assertTrue(row.getText().toString().contains("Home"));
            row.performClick();
            idle();
            android.app.Dialog picker = org.robolectric.shadows.ShadowDialog.getLatestDialog();
            assertNotSame(sheet, picker);
            assertNotNull(sheetRow(picker, screen.getString(R.string.profiles_none)));
            com.google.android.material.button.MaterialButton choice = sheetRow(picker, "Hotel");
            assertNotNull(choice);
            choice.performClick();
            idle();
            assertEquals(hotel, com.limelight.preferences.StreamProfiles.load(app).activeId);
            // Picking None clears the selection, as the Profiles page does.
            android.app.Dialog again = openGameSheet(grid, "Desktop");
            sheetRow(again, screen.getString(R.string.profiles_subtitle)).performClick();
            idle();
            sheetRow(org.robolectric.shadows.ShadowDialog.getLatestDialog(), screen.getString(R.string.profiles_none)).performClick();
            idle();
            assertNull(com.limelight.preferences.StreamProfiles.load(app).activeId);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test public void withAChosenByNetworkRuleTheProfileRowOpensTheProfilesPage() {
        android.content.Context app = RuntimeEnvironment.getApplication();
        String wifi = com.limelight.preferences.StreamProfiles.create(app, "Couch", new com.limelight.preferences.HostStreamProfile());
        com.limelight.preferences.StreamProfiles.setAuto(app, true);
        com.limelight.preferences.StreamProfiles.setRule(app, com.limelight.preferences.StreamLayers.Network.WIFI, wifi);
        android.net.ConnectivityManager connectivity =
                (android.net.ConnectivityManager) app.getSystemService(android.content.Context.CONNECTIVITY_SERVICE);
        android.net.NetworkInfo info = org.robolectric.shadows.ShadowNetworkInfo.newInstance(
                android.net.NetworkInfo.DetailedState.CONNECTED, android.net.ConnectivityManager.TYPE_WIFI, 0, true,
                android.net.NetworkInfo.State.CONNECTED);
        org.robolectric.shadows.ShadowConnectivityManager shadow = org.robolectric.Shadows.shadowOf(connectivity);
        android.net.Network network = org.robolectric.shadows.ShadowNetwork.newInstance(1);
        android.net.NetworkCapabilities caps = org.robolectric.shadows.ShadowNetworkCapabilities.newInstance();
        org.robolectric.Shadows.shadowOf(caps).addTransportType(android.net.NetworkCapabilities.TRANSPORT_WIFI);
        shadow.addNetwork(network, info);
        shadow.setNetworkCapabilities(network, caps);
        shadow.setActiveNetworkInfo(info);
        Intent intent = new Intent(app, Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            android.app.Dialog sheet = openGameSheet(grid, "Desktop");
            com.google.android.material.button.MaterialButton row = sheetRow(sheet, screen.getString(R.string.profiles_subtitle));
            assertNotNull(row);
            assertTrue("shows the profile the next stream uses: " + row.getText(), row.getText().toString().contains("Couch"));
            row.performClick();
            idle();
            Intent started = org.robolectric.Shadows.shadowOf(screen).getNextStartedActivity();
            assertNotNull(started);
            assertEquals(com.limelight.preferences.StreamProfilesActivity.class.getName(), started.getComponent().getClassName());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** A PC's cached list is in the adapter before the grid exists: its cards came in without their rise. */
    @Test public void aListThatWasCachedStillBringsItsCardsIn() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            // Motion as on a phone with animations on; a test harness turns them off.
            android.provider.Settings.Global.putFloat(screen.getContentResolver(),
                    android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f);
            ReflectionHelpers.callStaticMethod(android.animation.ValueAnimator.class, "setDurationScale",
                    ReflectionHelpers.ClassParameter.from(float.class, 1f));
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            assertTrue(ReflectionHelpers.<Boolean>getField(screen, "enterPlayed"));
            // The cards really rise: the first starts low and clear, and stands in place once the rise is over.
            // As a frame does: the grid lays its cards out, then the pre-draw pass starts the rise.
            grid.measure(View.MeasureSpec.makeMeasureSpec(1280, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY));
            grid.layout(0, 0, 1280, 720);
            assertTrue("cards laid out", grid.getChildCount() > 0);
            grid.getViewTreeObserver().dispatchOnPreDraw();
            View first = grid.getChildAt(0);
            assertTrue("starts below its place: " + first.getTranslationY(), first.getTranslationY() > 0f);
            assertTrue("starts clear: " + first.getAlpha(), first.getAlpha() < 1f);
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idleFor(java.time.Duration.ofSeconds(1));
            assertEquals(0f, grid.getChildAt(0).getTranslationY(), 0f);
            assertEquals(1f, grid.getChildAt(0).getAlpha(), 0f);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test public void theHintedMoreKeysOpenTheSelectedGamesMenuWithoutLaunchingIt() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            com.limelight.ui.UiNavigation.bindGrid(grid);
            long now = android.os.SystemClock.uptimeMillis();
            for (int key : new int[] {android.view.KeyEvent.KEYCODE_BUTTON_X,
                    android.view.KeyEvent.KEYCODE_BUTTON_Y, android.view.KeyEvent.KEYCODE_F,
                    android.view.KeyEvent.KEYCODE_MENU}) {
                grid.requestFocus();
                grid.setSelection(position(grid, "Desktop"));
                org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
                for (int action : new int[] {android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.ACTION_UP}) {
                    assertTrue(grid.dispatchKeyEvent(new android.view.KeyEvent(now, now, action, key, 0)));
                }
                org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
                android.app.Dialog dialog = org.robolectric.shadows.ShadowDialog.getLatestDialog();
                assertTrue("options for key " + key, dialog instanceof com.limelight.ui.ActionSheet && dialog.isShowing());
                assertNull("More must not launch a stream", org.robolectric.Shadows.shadowOf(screen).getNextStartedActivity());
                dialog.dismiss();
            }
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test public void theRunningGameIsPinnedFirstWithARunningChipAndNoPlayLabels() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            assertEquals("Racing Game", ((AppView.AppObject) grid.getItemAtPosition(0)).app.getAppName());
            View tile = grid.getAdapter().getView(0, null, grid);
            TextView chip = tile.findViewById(R.id.grid_status);
            assertEquals(View.VISIBLE, chip.getVisibility());
            assertEquals(screen.getString(R.string.app_running), chip.getText().toString());
            assertTrue("the dot, not a chip", chip instanceof com.limelight.ui.StatusDisc);
            assertTrue(chip.isActivated());
            assertEquals("Racing Game, " + screen.getString(R.string.app_running), tile.getContentDescription().toString());
            View other = grid.getAdapter().getView(position(grid, "Desktop"), null, grid);
            assertEquals(View.GONE, ((TextView) other.findViewById(R.id.grid_status)).getVisibility());
            assertEquals("Desktop", other.getContentDescription().toString());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test public void startingAnotherGameAsksFirstWhereTheClientQuitsTheRunningOne() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            List<String> others = sheetFor(grid, "Puzzle Night");
            assertEquals(screen.getString(R.string.library_quit_named_and_play, "Racing Game"), others.get(0));
            assertTrue(others.contains(screen.getString(R.string.applist_menu_hide_app)));
            assertEquals(screen.getString(R.string.applist_menu_details), others.get(others.size() - 1));
            List<String> running = sheetFor(grid, "Racing Game");
            assertEquals(List.of(screen.getString(R.string.applist_menu_resume), screen.getString(R.string.applist_menu_quit)),
                    running.subList(0, 2));
            assertFalse(running.contains(screen.getString(R.string.applist_menu_hide_app)));

            int puzzle = position(grid, "Puzzle Night");
            grid.performItemClick(null, puzzle, puzzle);
            androidx.appcompat.app.AlertDialog dialog =
                    (androidx.appcompat.app.AlertDialog) org.robolectric.shadows.ShadowDialog.getLatestDialog();
            assertEquals(screen.getString(R.string.library_replace_message_named, "Racing Game", "Puzzle Night"),
                    ((TextView) dialog.findViewById(android.R.id.message)).getText().toString());
            assertNull(org.robolectric.Shadows.shadowOf(screen).getNextStartedActivity());
            dialog.getButton(androidx.appcompat.app.AlertDialog.BUTTON_POSITIVE).performClick();
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            assertEquals(Game.class.getName(),
                    org.robolectric.Shadows.shadowOf(screen).getNextStartedActivity().getComponent().getClassName());

            // The running game itself resumes on a tap.
            grid.performItemClick(null, 0, 0);
            assertEquals(Game.class.getName(),
                    org.robolectric.Shadows.shadowOf(screen).getNextStartedActivity().getComponent().getClassName());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test public void aHostThatConfirmsReplacementIsLeftToAskItself() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "rubylight-serverinfo.xml");
            String first = sheetFor(grid, "Puzzle Night").get(0);
            assertEquals(screen.getString(R.string.library_play) + "\n" +
                    screen.getString(R.string.library_host_asks_named, "Racing Game"), first);
            org.robolectric.shadows.ShadowDialog.reset();
            int puzzle = position(grid, "Puzzle Night");
            grid.performItemClick(null, puzzle, puzzle);
            assertNull("The host asks; the client does not", org.robolectric.shadows.ShadowDialog.getLatestDialog());
            assertEquals(Game.class.getName(),
                    org.robolectric.Shadows.shadowOf(screen).getNextStartedActivity().getComponent().getClassName());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void onAPhoneTheRunningGameHasACapsuleAndTheOneSearchSitsBesideTheTabs() {
        ActivityController<Library> controller = runningLibrary();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            assertSame("Up from the grid reaches Back", screen.findViewById(R.id.library_back), grid.focusSearch(View.FOCUS_UP));
            assertEquals(R.id.library_back, grid.getAccessibilityTraversalAfter());
            assertEquals(View.VISIBLE, screen.findViewById(R.id.library_resume).getVisibility());
            assertEquals("Racing Game", ((TextView) screen.findViewById(R.id.library_resume_name)).getText().toString());
            assertEquals(screen.getString(R.string.applist_menu_resume), ((TextView) screen.findViewById(R.id.library_resume_button)).getText().toString());
            assertSame(screen.findViewById(R.id.library_bottom_row), screen.findViewById(R.id.library_search_button).getParent());
            assertEquals(View.VISIBLE, screen.findViewById(R.id.library_back).getVisibility());
            assertEquals("no hero on a phone", View.GONE, screen.findViewById(R.id.library_focus_title).getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void thePcsTabGoesBackToTheListOfPcs() {
        ActivityController<Library> controller = runningLibrary();
        try {
            Library screen = controller.get();
            assertTrue(screen.findViewById(R.id.shell_pcs).performClick());
            assertTrue(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionTheRunningGameIsResumedFromItsTileWithTheKeysSayingSo() {
        ActivityController<Library> controller = runningLibrary();
        try {
            Library screen = controller.get();
            libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            org.robolectric.shadows.ShadowLooper.idleMainLooper(100, java.util.concurrent.TimeUnit.MILLISECONDS);
            assertEquals(View.GONE, screen.findViewById(R.id.library_resume).getVisibility());
            assertEquals("Racing Game", ((TextView) screen.findViewById(R.id.library_focus_title)).getText().toString());
            String line = ((TextView) screen.findViewById(R.id.library_focus_status)).getText().toString();
            assertTrue(line, line.startsWith(screen.getString(R.string.app_running)));
            assertTrue(line, line.contains(screen.getString(R.string.applist_menu_resume)));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    @org.robolectric.annotation.GraphicsMode(org.robolectric.annotation.GraphicsMode.Mode.NATIVE)
    public void onATelevisionTheAppGridKeepsItsTextReadableAndInsideTheOverscan() throws Exception {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup().visible();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            assertEquals(Collections.emptyList(), com.limelight.ui.TvAudit.findings(screen));
            com.limelight.ui.TvScreensPreviewTest.previewIfRequested(screen, "AppView-library");
            // The chrome stays one press away: Up from the first row reaches it. The tabs name the page, so a TV has no Back button:
            // Right walks from the capsule to search, and Left walks back.
            assertEquals(View.GONE, screen.findViewById(R.id.library_back).getVisibility());
            View search = screen.findViewById(R.id.library_search_button);
            View options = screen.findViewById(R.id.library_options);
            com.limelight.ui.TvScreensPreviewTest.leaveTouchMode(screen);
            assertTrue(grid.requestFocus());
            grid.setSelection(0);
            com.limelight.ui.TvScreensPreviewTest.settle(screen);
            View up = grid.focusSearch(View.FOCUS_UP);
            boolean inTabs = false;
            for (android.view.ViewParent at = up.getParent(); at != null; at = at.getParent()) inTabs |= at == screen.findViewById(R.id.shell_bar);
            assertTrue("Up from the grid reaches the chrome: " + up, up == search || up == options || inTabs);
            assertSame(search, options.focusSearch(View.FOCUS_RIGHT));
            assertSame(options, search.focusSearch(View.FOCUS_LEFT));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /**
     * On the real TV a focused poster on the bottom row ran into the screen's edge, ring and all: the grid kept only 12 dp
     * under its last row, too little to scroll by. It keeps the overscan plus room for the focus growth and ring, so the
     * focused card can scroll that far clear of the edge.
     */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionTheGridKeepsRoomUnderItsLastRowForTheFocusedCard() throws Exception {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup().visible();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            float density = screen.getResources().getDisplayMetrics().density;
            int edge = screen.getResources().getDimensionPixelSize(R.dimen.shell_overscan_vertical);
            assertTrue("room under the last row: " + grid.getPaddingBottom(),
                    grid.getPaddingBottom() >= edge + Math.round(24 * density));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /**
     * On the real TV the scrolled grid's top row stopped on a hard line under the title: its fade was 14 dp. Rows fade out
     * under the title over All games' length, from the grid's own top edge.
     */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionScrolledRowsFadeUnderTheTitleAsOnAllGames() throws Exception {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.UUID_EXTRA, "running-host").putExtra(AppView.NAME_EXTRA, "PC");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup().visible();
        try {
            Library screen = controller.get();
            android.widget.GridView grid = libraryWithRunningGame(screen, "sunshine-serverinfo.xml");
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            assertTrue(grid.isVerticalFadingEdgeEnabled());
            assertEquals(screen.getResources().getDimensionPixelSize(R.dimen.all_games_fade), grid.getVerticalFadingEdgeLength());
            assertFalse("the fade starts at the grid's edge, past its padding", grid.getClipToPadding());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** The tabs were installed again after the chrome styled them: larger words, another width, stacked on PCs and inline here. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionTheCapsuleIsTheSameAsOnHome() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.NAME_EXTRA, "Tower").putExtra(AppView.UUID_EXTRA, "uuid");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            assertEquals(com.limelight.console.HomeShellTest.tvCapsule(), com.limelight.ui.TvAudit.topCapsule(controller.get()));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "w412dp-h915dp-port-xxhdpi")
    public void onAPhoneTheTabsStackIconOverWord() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.NAME_EXTRA, "Tower").putExtra(AppView.UUID_EXTRA, "uuid");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            com.google.android.material.button.MaterialButton tab = controller.get().findViewById(R.id.shell_settings);
            assertEquals(com.google.android.material.button.MaterialButton.ICON_GRAVITY_TEXT_TOP, tab.getIconGravity());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** The grid's clip cut the selected card's halo in a straight line under the hints; it reaches past the grid's top now. */
    /** A remote reaches the PC capsule and search from the tabs, and they end in a wall. */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void onATelevisionRightWalksTheTopRow() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.NAME_EXTRA, "Tower").putExtra(AppView.UUID_EXTRA, "uuid");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            Library screen = controller.get();
            assertEquals(R.id.library_options, screen.findViewById(R.id.shell_settings).getNextFocusRightId());
            assertEquals(R.id.library_search_button, screen.findViewById(R.id.library_options).getNextFocusRightId());
            assertEquals(R.id.library_search_button, screen.findViewById(R.id.library_search_button).getNextFocusRightId());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** Resume's glow reaches past the button; a clip on the way up left its corners square in the capsule. */
    @Test
    public void resumesGlowIsNotCutInItsCapsule() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.NAME_EXTRA, "Tower").putExtra(AppView.UUID_EXTRA, "uuid");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            android.view.View capsule = controller.get().findViewById(R.id.library_resume);
            android.view.View view = controller.get().findViewById(R.id.library_resume_button);
            while (view != capsule) {
                android.view.ViewGroup parent = (android.view.ViewGroup) view.getParent();
                assertFalse(parent.getClipChildren());
                view = parent;
            }
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void theHaloIsNotCutWhereTheChromeEnds() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), Library.class)
                .putExtra(AppView.NAME_EXTRA, "Tower").putExtra(AppView.UUID_EXTRA, "uuid");
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class, intent).setup();
        try {
            android.view.ViewGroup frame = controller.get().findViewById(R.id.appFragmentContainer);
            assertFalse(frame.getClipChildren());
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
