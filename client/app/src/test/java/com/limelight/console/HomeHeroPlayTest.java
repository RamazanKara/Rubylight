package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static com.limelight.nativeapi.NativeApiFixtures.session;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.DialogInterface;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Looper;

import androidx.appcompat.app.AlertDialog;

import com.limelight.PcViewLifecycleTest;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/** The hero's Play never closes a game unasked: it goes by what the game's page goes by. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class HomeHeroPlayTest {
    public static class Home extends ConsoleHomeActivity {
        final List<String> played = new ArrayList<>();

        @Override protected void play(NvApp app, ComputerDetails computer, String source) {
            played.add(app.getAppName());
        }

        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }
    }

    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private ConsoleRig rig;
    private Home home;
    /** A PC without HOST_CONFIRMS_REPLACEMENT: nothing but this client would ask. */
    private ComputerDetails tower;

    @Before
    public void setUp() throws Exception {
        rig = new ConsoleRig(temporary.newFolder());
        home = Robolectric.buildActivity(Home.class).setup().get();
        tower = nativeHost("tower", "Tower", true);
        tower.caps = HostCapabilities.UNKNOWN;
        HostFeed feed = ReflectionHelpers.getField(home, "feed");
        ReflectionHelpers.setField(feed, "binder", new ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) { }
            @Override public void stopPolling() { }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public String getUniqueId() { return null; }
            @Override public void waitForPollingStopped() { }
        });
        feed.update(tower);
    }

    @After
    public void tearDown() {
        rig.close();
    }

    private void read(String zeldaFields, boolean portalRuns) throws Exception {
        String zelda = game("t-zelda", 12, "Zelda", null, false);
        zelda = zelda.substring(0, zelda.length() - 1) + zeldaFields + "}";
        rig.transport.reply("tower", "library", ok(library("t1", game("t-portal", 11, "Portal", "steam:400", portalRuns), zelda)));
        rig.transport.reply("tower", "session", ok(portalRuns ? session("streaming", "you",
                "{\"install_id\":\"t-portal\",\"app_id\":11,\"title\":\"Portal\"}", "[]") : NativeApiFixtures.IDLE));
        ConsoleSnapshot snapshot = rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        ReflectionHelpers.setField(home, "shownSnapshot", snapshot);
    }

    private void pressPlayOnZelda() {
        HomeModel.GameCard zelda = new HomeModel.GameCard("k:zelda", "Zelda", "tower", "Tower", 12, HomeModel.Badge.NONE, null, 1,
                false, false, Collections.singletonList("tower"), null);
        ReflectionHelpers.callInstanceMethod(home, "playHero",
                ReflectionHelpers.ClassParameter.from(HomeModel.GameCard.class, zelda));
        shadowOf(Looper.getMainLooper()).idle();
    }

    private static AlertDialog prompt() {
        android.app.Dialog dialog = ShadowDialog.getLatestDialog();
        return dialog instanceof AlertDialog && dialog.isShowing() ? (AlertDialog) dialog : null;
    }

    @Test
    public void withNothingElseRunningPlayStartsAtOnce() throws Exception {
        read("", false);
        pressPlayOnZelda();
        assertEquals(Collections.singletonList("Zelda"), home.played);
        assertNull(prompt());
    }

    @Test
    public void anotherGameRunningAsksFirstAndNamesIt() throws Exception {
        read("", true);
        pressPlayOnZelda();
        assertTrue("nothing starts before the answer", home.played.isEmpty());
        AlertDialog dialog = prompt();
        assertNotNull("the prompt the game's page shows", dialog);
        assertEquals("Close Portal and start Zelda", dialog.getButton(DialogInterface.BUTTON_POSITIVE).getText().toString());
        assertEquals("Resume Portal", dialog.getButton(DialogInterface.BUTTON_NEUTRAL).getText().toString());
    }

    @Test
    public void whatThePcReportsRunningAsksEvenWhenHomesReadIsOlder() throws Exception {
        read("", false);
        tower.runningGameId = 11;
        pressPlayOnZelda();
        assertTrue(home.played.isEmpty());
        assertNotNull(prompt());
    }

    @Test
    public void closeAndStartStartsTheGameOnceTheOldOneIsGone() throws Exception {
        read("", true);
        pressPlayOnZelda();
        tower.runningGameId = 0;
        tower.runningGameUuid = null;
        prompt().getButton(DialogInterface.BUTTON_POSITIVE).performClick();
        shadowOf(Looper.getMainLooper()).idle();
        assertEquals(Collections.singletonList("Zelda"), home.played);
    }

    @Test
    public void aGameThePcCannotStartOpensItsPageWhichSaysWhy() throws Exception {
        read(",\"launchable\":false", false);
        pressPlayOnZelda();
        assertTrue(home.played.isEmpty());
        assertNull(prompt());
    }
}
