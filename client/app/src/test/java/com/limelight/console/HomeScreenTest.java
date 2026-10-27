package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.preference.PreferenceManager;

import com.limelight.PcView;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.io.File;
import java.util.Arrays;
import java.util.Collections;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HomeScreenTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private Context context;

    @Before
    public void setUp() {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    @After
    public void tearDown() {
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    /** Puts a library in the app's own cache, as the first refresh of a native host does. */
    private void cacheALibrary() throws Exception {
        ConsoleRig rig = new ConsoleRig(new File(context.getFilesDir(), "console-library"));
        try {
            rig.transport.reply("tower", "library", ok(library("t1", game("t-portal", 11, "Portal", "steam:400", false))));
            rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
            rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        } finally {
            rig.close();
        }
    }

    @Test
    public void homeOpensOnlyWhenTheSettingIsOnAndAHostHasShownItsGames() throws Exception {
        assertTrue(HomeSettings.enabled(context));
        assertFalse("classic hosts only: the list of PCs stays the launch screen", HomeSettings.opensOnHome(context));
        cacheALibrary();
        assertTrue(HomeSettings.opensOnHome(context));
        PreferenceManager.getDefaultSharedPreferences(context).edit().putBoolean(HomeSettings.PREF_KEY, false).commit();
        assertFalse(HomeSettings.enabled(context));
        assertFalse(HomeSettings.opensOnHome(context));
    }

    @Test
    public void theWayBackToTheListOfPcsStaysOnTheList() {
        Intent intent = HomeSettings.pcList(context);
        assertEquals(PcView.class.getName(), intent.getComponent().getClassName());
        assertTrue(intent.getBooleanExtra(HomeSettings.EXTRA_STAY, false));
    }

    @Test
    public void headersTakeTheRowAndCardsKeepTheirPlaceAtEveryWidth() {
        for (int columns : new int[]{2, 3, 4, 6, 8}) {
            assertEquals(columns, ConsoleHomeActivity.spanOf(ConsoleHomeListAdapter.HEADER, columns));
            assertEquals(1, ConsoleHomeActivity.spanOf(ConsoleHomeListAdapter.COVER_GAME, columns));
            int wide = ConsoleHomeActivity.spanOf(ConsoleHomeListAdapter.WIDE_GAME, columns);
            assertEquals(wide, ConsoleHomeActivity.spanOf(ConsoleHomeListAdapter.HOST, columns));
            assertEquals("wide cards fill rows evenly", 0, columns % wide);
        }
    }

    @Test
    public void aHeaderScrolledUnderTheTopOfTheListFadesOutBeforeItsLettersPeekOut() {
        assertEquals("in view", 1f, ConsoleHomeListAdapter.headerAlpha(0, 60), 0f);
        assertEquals("just a little above", 1f, ConsoleHomeListAdapter.headerAlpha(-6, 54), 0.25f);
        assertEquals("half in view", 0f, ConsoleHomeListAdapter.headerAlpha(-30, 30), 0f);
        assertEquals("only the foot of its letters left", 0f, ConsoleHomeListAdapter.headerAlpha(-52, 8), 0f);
        assertEquals("gone", 0f, ConsoleHomeListAdapter.headerAlpha(-80, -20), 0f);
        assertTrue("fading", ConsoleHomeListAdapter.headerAlpha(-15, 45) > 0f && ConsoleHomeListAdapter.headerAlpha(-15, 45) < 1f);
    }

    @Test
    public void columnsFollowTheWidthAndStayEvenFromFour() {
        assertEquals(2, ConsoleHomeActivity.columnsFor(320));
        assertEquals(2, ConsoleHomeActivity.columnsFor(389));
        assertEquals(3, ConsoleHomeActivity.columnsFor(411));
        assertEquals(4, ConsoleHomeActivity.columnsFor(700));
        assertEquals(6, ConsoleHomeActivity.columnsFor(900));
        assertEquals(8, ConsoleHomeActivity.columnsFor(1100));
    }

    @Test
    public void theHostSignatureChangesWithWhatHomeShows() {
        ComputerDetails host = nativeHost("tower", "Tower", true);
        String online = ConsoleHomeActivity.signature(Collections.singletonList(host));
        assertEquals(online, ConsoleHomeActivity.signature(Collections.singletonList(new ComputerDetails(host))));
        assertNotEquals(online, ConsoleHomeActivity.signature(Collections.singletonList(nativeHost("tower", "Tower", false))));
        ComputerDetails playing = new ComputerDetails(host);
        playing.runningGameId = 7;
        assertNotEquals(online, ConsoleHomeActivity.signature(Collections.singletonList(playing)));
        assertNotEquals(online, ConsoleHomeActivity.signature(Arrays.asList(host, nativeHost("laptop", "Laptop", true))));
        ComputerDetails renamed = new ComputerDetails(host);
        renamed.alias = "Living room";
        assertNotEquals(online, ConsoleHomeActivity.signature(Collections.singletonList(renamed)));
    }
}
