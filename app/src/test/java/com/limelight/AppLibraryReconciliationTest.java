package com.limelight;

import android.app.Application;
import android.content.Intent;

import com.limelight.grid.AppGridAdapter;
import com.limelight.grid.assets.CachedAppAssetLoader;
import com.limelight.grid.assets.MemoryAssetLoader;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.utils.ShortcutHelper;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class AppLibraryReconciliationTest {
    private ActivityController<AppViewLifecycleTest.Library> controller;
    private AppViewLifecycleTest.Library screen;
    private AppGridAdapter adapter;
    private RecordingShortcuts shortcuts;

    private static class RecordingShortcuts extends ShortcutHelper {
        final Set<String> enabled = new HashSet<>();

        RecordingShortcuts(AppView screen) { super(screen); }

        private String identity(NvApp app) {
            return app.getAppUuid().isEmpty() ? Integer.toString(app.getAppId()) : app.getAppUuid();
        }

        @Override public void enableAppShortcut(ComputerDetails computer, NvApp app) {
            enabled.add(identity(app));
        }

        @Override public void disableAppShortcut(ComputerDetails computer, NvApp app, CharSequence reason) {
            enabled.remove(identity(app));
        }
    }

    @Before public void openLibrary() {
        Intent intent = new Intent(RuntimeEnvironment.getApplication(), AppViewLifecycleTest.Library.class)
                .putExtra(AppView.UUID_EXTRA, "library-host").putExtra(AppView.NAME_EXTRA, "PC");
        controller = Robolectric.buildActivity(AppViewLifecycleTest.Library.class, intent).setup();
        screen = controller.get();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "library-host";
        computer.name = "PC";
        adapter = new AppGridAdapter(screen, new PreferenceConfiguration(), computer, "test", false);
        ReflectionHelpers.setField(adapter, "loader",
                new CachedAppAssetLoader(computer, 1, null, new MemoryAssetLoader(), null, null) {
                    @Override public void queueCacheLoad(NvApp app) { }
                });
        shortcuts = new RecordingShortcuts(screen);
        ReflectionHelpers.setField(screen, "computer", computer);
        ReflectionHelpers.setField(screen, "appGridAdapter", adapter);
        ReflectionHelpers.setField(screen, "shortcutHelper", shortcuts);
    }

    @After public void closeLibrary() {
        controller.pause().stop().destroy();
    }

    private static NvApp app(String name, int id, String uuid) {
        NvApp app = new NvApp(name, id, false);
        app.setAppUuid(uuid);
        return app;
    }

    private void receive(NvApp... apps) {
        ReflectionHelpers.callInstanceMethod(screen, "updateUiWithAppList",
                ReflectionHelpers.ClassParameter.from(List.class, Arrays.asList(apps)));
    }

    @Test public void changedNumericIdKeepsTheGamesExistingShortcutEnabled() {
        receive(app("Game", 10, "stable-game"));
        assertTrue(shortcuts.enabled.contains("stable-game"));

        receive(app("Renamed game", 20, "stable-game"));

        assertEquals(1, adapter.getCount());
        NvApp updated = ((AppView.AppObject) adapter.getItem(0)).app;
        assertEquals(20, updated.getAppId());
        assertEquals("Renamed game", updated.getAppName());
        assertTrue("A game still on the host must keep its pinned shortcut enabled",
                shortcuts.enabled.contains("stable-game"));
    }

    @Test public void reusedNumericIdDisablesOnlyTheRemovedGamesShortcut() {
        receive(app("Old game", 10, "old-game"));

        receive(app("Replacement game", 10, "replacement-game"));

        assertEquals(1, adapter.getCount());
        assertFalse(shortcuts.enabled.contains("old-game"));
        assertTrue(shortcuts.enabled.contains("replacement-game"));
    }

    @Test public void removingAnAppStillDisablesItsShortcut() {
        receive(app("Removed game", 10, "removed-game"));

        receive();

        assertEquals(0, adapter.getCount());
        assertFalse(shortcuts.enabled.contains("removed-game"));
    }

    @Test public void hostsWithoutAppUuidsStillUpdateByNumericId() {
        receive(app("Game", 10, ""));

        receive(app("Renamed game", 10, ""));

        assertEquals(1, adapter.getCount());
        assertEquals("Renamed game", ((AppView.AppObject) adapter.getItem(0)).app.getAppName());
        assertTrue(shortcuts.enabled.contains("10"));
    }

    @Test public void hiddenAppsAreUpdatedOnceAndDoNotAccumulateDuplicates() {
        adapter.updateHiddenApps(Collections.singleton(10), true);
        receive(app("Hidden game", 10, "hidden-game"), app("Visible game", 20, "visible-game"));

        receive(app("Renamed hidden game", 10, "hidden-game"), app("Visible game", 20, "visible-game"));
        receive(app("Renamed hidden game", 10, "hidden-game"), app("Visible game", 20, "visible-game"));

        assertEquals(1, adapter.getCount());
        adapter.updateHiddenApps(Collections.emptySet(), true);
        assertEquals("Unhiding must expose exactly one current entry per game", 2, adapter.getCount());
        assertEquals("Renamed hidden game", ((AppView.AppObject) adapter.getItem(0)).app.getAppName());
    }

    @Test public void removedHiddenAppsDoNotReappearAndTheirShortcutsAreDisabled() {
        adapter.updateHiddenApps(Collections.singleton(10), true);
        receive(app("Hidden game", 10, "hidden-game"));

        receive();

        adapter.updateHiddenApps(Collections.emptySet(), true);
        assertEquals(0, adapter.getCount());
        assertFalse(shortcuts.enabled.contains("hidden-game"));
    }

    @Test public void hiddenChoiceFollowsTheGameWhenTheHostSwapsNumericIds() {
        ReflectionHelpers.setField(screen, "hiddenAppIds", new HashSet<>(Collections.singleton(10)));
        ReflectionHelpers.callInstanceMethod(screen, "updateHiddenApps",
                ReflectionHelpers.ClassParameter.from(boolean.class, true));
        receive(app("Hidden game", 10, "hidden-game"), app("Visible game", 20, "visible-game"));

        receive(app("Hidden game", 20, "hidden-game"), app("Visible game", 10, "visible-game"));

        assertEquals(1, adapter.getCount());
        assertEquals("visible-game", ((AppView.AppObject) adapter.getItem(0)).app.getAppUuid());
        assertEquals(Collections.singleton("20"), screen.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, 0)
                .getStringSet("library-host", Collections.emptySet()));
    }

    @Test public void differentGameReusingARemovedHiddenIdDoesNotInheritTheHiddenChoice() {
        ReflectionHelpers.setField(screen, "hiddenAppIds", new HashSet<>(Arrays.asList(10, 30)));
        ReflectionHelpers.callInstanceMethod(screen, "updateHiddenApps",
                ReflectionHelpers.ClassParameter.from(boolean.class, true));
        receive(app("Old game", 10, "old-game"));

        receive(app("Replacement game", 10, "replacement-game"));

        assertEquals(1, adapter.getCount());
        assertEquals("replacement-game", ((AppView.AppObject) adapter.getItem(0)).app.getAppUuid());
        assertEquals("Unrelated absent apps keep their saved hidden choice", Collections.singleton("30"),
                screen.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, 0)
                        .getStringSet("library-host", Collections.emptySet()));
    }

    @Test public void stableUuidMatchesBeforeLegacyNumbersDuringAnIdSwap() {
        ReflectionHelpers.setField(screen, "hiddenAppIds", new HashSet<>(Collections.singleton(10)));
        ReflectionHelpers.callInstanceMethod(screen, "updateHiddenApps",
                ReflectionHelpers.ClassParameter.from(boolean.class, true));
        receive(app("Hidden game", 10, "stable-game"), app("Legacy game", 20, ""));

        receive(app("Legacy game", 10, ""), app("Hidden game", 20, "stable-game"));

        assertEquals(1, adapter.getCount());
        assertEquals("Legacy game", ((AppView.AppObject) adapter.getItem(0)).app.getAppName());
        assertTrue(shortcuts.enabled.contains("stable-game"));
        assertTrue(shortcuts.enabled.contains("10"));
        assertFalse(shortcuts.enabled.contains("20"));
        assertEquals(Collections.singleton("20"), screen.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, 0)
                .getStringSet("library-host", Collections.emptySet()));
    }

    @Test public void visibleGameMovingOntoARemovedHiddenGamesIdStaysVisible() {
        ReflectionHelpers.setField(screen, "hiddenAppIds", new HashSet<>(Collections.singleton(20)));
        ReflectionHelpers.callInstanceMethod(screen, "updateHiddenApps",
                ReflectionHelpers.ClassParameter.from(boolean.class, true));
        receive(app("Visible game", 10, "visible-game"), app("Removed hidden game", 20, "hidden-game"));

        receive(app("Visible game", 20, "visible-game"));

        assertEquals(1, adapter.getCount());
        assertEquals("visible-game", ((AppView.AppObject) adapter.getItem(0)).app.getAppUuid());
        assertEquals(Collections.emptySet(), screen.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, 0)
                .getStringSet("library-host", Collections.emptySet()));
    }
    @Test public void searchIgnoresCaseAndAccentsAndKeepsTheFullCatalogue() {
        receive(app("Pokémon", 10, "pokemon"), app("Portal", 20, "portal"));
        androidx.appcompat.widget.SearchView search = screen.findViewById(R.id.library_search);
        search.setQuery(" POKEMON ", false);
        assertEquals(1, adapter.getCount());
        assertEquals("pokemon", ((AppView.AppObject) adapter.getItem(0)).app.getAppUuid());
        assertEquals(2, adapter.getAllApps().size());

        search.setQuery("no matching game", false);
        assertEquals(0, adapter.getCount());
        android.widget.TextView status = screen.findViewById(R.id.app_list_status);
        assertEquals(screen.getString(R.string.library_no_matches), status.getText().toString());
        screen.findViewById(R.id.library_retry).performClick();
        assertEquals(2, adapter.getCount());
        assertEquals("", search.getQuery().toString());
    }

    @Test public void searchReconcilesRenamedAndRemovedAppsOutsideTheVisibleResults() {
        receive(app("Portal", 10, "portal"), app("Other", 20, "other"));
        adapter.setSearchQuery("Portal");
        receive(app("Portal 2", 10, "portal"), app("Portal companion", 20, "other"));
        assertEquals(2, adapter.getCount());
        receive(app("Different name", 10, "portal"));
        assertEquals(0, adapter.getCount());
        adapter.setSearchQuery("");
        assertEquals(1, adapter.getCount());
        assertEquals("Different name", ((AppView.AppObject) adapter.getItem(0)).app.getAppName());
    }

    @Test public void hiddenFilterCanBeChangedWithoutLeavingTheLibrary() {
        receive(app("Hidden game", 10, "hidden"), app("Visible game", 20, "visible"));
        adapter.updateHiddenApps(Collections.singleton(10), true);
        assertEquals(1, adapter.getCount());
        adapter.setShowHiddenApps(true);
        assertEquals(2, adapter.getCount());
        assertTrue(((AppView.AppObject) adapter.getItem(0)).isHidden);
        adapter.setSearchQuery("hidden");
        assertEquals(1, adapter.getCount());
        adapter.setShowHiddenApps(false);
        assertEquals(0, adapter.getCount());
        adapter.updateHiddenApps(Collections.emptySet(), true);
        assertEquals(1, adapter.getCount());
    }

    @Test public void runningStateRemainsCorrectWhenGamesAreFilteredOut() {
        receive(app("First game", 10, "first"), app("Second game", 20, "second"));
        ComputerDetails state = new ComputerDetails();
        state.runningGameId = 10;
        state.runningGameUuid = "first";
        ReflectionHelpers.callInstanceMethod(screen, "updateUiWithServerinfo",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, state));
        adapter.setSearchQuery("Second");
        state.runningGameId = 20;
        state.runningGameUuid = "second";
        ReflectionHelpers.callInstanceMethod(screen, "updateUiWithServerinfo",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, state));
        adapter.setSearchQuery("");
        // The running game leads the library.
        assertEquals("Second game", ((AppView.AppObject) adapter.getItem(0)).app.getAppName());
        assertTrue(((AppView.AppObject) adapter.getItem(0)).isRunning);
        assertFalse(((AppView.AppObject) adapter.getItem(1)).isRunning);
    }

}
