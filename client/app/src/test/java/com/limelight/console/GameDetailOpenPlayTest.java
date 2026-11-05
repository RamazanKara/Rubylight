package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.app.Dialog;
import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.view.KeyEvent;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import java.util.Collections;
import java.util.concurrent.TimeUnit;
import java.util.function.BooleanSupplier;

/** The automatic start on a game's page: the countdown, its ways out, and what each ending leaves the guard as. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameDetailOpenPlayTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private Context context;
    private ConsoleRig rig;
    private static final String PORTAL_APP = "{\"install_id\":\"t-portal\",\"app_id\":11,\"title\":\"Portal\"}";
    private static final String ZELDA_APP = "{\"install_id\":\"t-zelda\",\"app_id\":22,\"title\":\"Zelda\"}";

    private String sessionThere = NativeApiFixtures.IDLE;
    private String stableThere = null;
    private String libraryThere = library("r", game("t-portal", 11, "Portal", "steam:400", false));
    private ActivityController<GameDetailWakeTest.Detail> controller;
    private GameDetailWakeTest.Detail screen;
    private HostFeed feed;

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        OpenPlayModel.use(request -> "{\"ok\":{\"countdown_ms\":3000,\"settled_ms\":10000}}");
        OpenPlay.disarm(context);
        GameDetailWakeTest.Detail.woken.clear();
        GameDetailWakeTest.Detail.played.clear();
        ShadowDialog.reset();
    }

    @After
    public void tearDown() {
        OpenPlayModel.use(null);
        GameDetailWakeTest.Detail.repository = null;
    }

    private void open(boolean online, String mac, Bundle state) throws Exception {
        rig = new ConsoleRig(temporary.newFolder());
        rig.transport.reply("tower", "library", ok(libraryThere));
        rig.transport.reply("tower", "session", sessionThere.startsWith("{\"error") ? sessionThere : ok(sessionThere));
        rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        GameDetailWakeTest.Detail.repository = rig.repository;
        Intent intent = new Intent(context, GameDetailWakeTest.Detail.class)
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, "tower")
                .putExtra(GameDetailActivity.EXTRA_APP_ID, 11)
                .putExtra(GameDetailActivity.EXTRA_TITLE, "Portal")
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, "Tower")
                .putExtra(GameDetailActivity.EXTRA_STABLE_ID, stableThere)
                .putExtra(GameDetailActivity.EXTRA_OPEN_PLAY, true);
        controller = Robolectric.buildActivity(GameDetailWakeTest.Detail.class, intent);
        controller = state == null ? controller.setup() : controller.setup(state);
        controller.visible();
        screen = controller.get();
        feed = ReflectionHelpers.getField(screen, "feed");
        ReflectionHelpers.setField(feed, "binder", binder());
        ComputerDetails tower = nativeHost("tower", "Tower", online);
        tower.macAddress = mac;
        feed.update(tower);
    }

    private static ComputerManagerService.ComputerManagerBinder binder() {
        return new ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) { }
            @Override public void stopPolling() { }
            @Override public void renameComputer(String uuid, String alias) { }
            @Override public void removeComputer(ComputerDetails computer) { }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public String getUniqueId() { return "unique"; }
            @Override public void waitForPollingStopped() { }
        };
    }

    private static void pass(long ms) {
        shadowOf(Looper.getMainLooper()).idleFor(ms, TimeUnit.MILLISECONDS);
    }

    private static void await(BooleanSupplier done) throws Exception {
        long end = System.currentTimeMillis() + 5000;
        while (!done.getAsBoolean() && System.currentTimeMillis() < end) {
            pass(50);
            Thread.sleep(20);
        }
        assertTrue("in time", done.getAsBoolean());
    }

    private static AlertDialog shown() {
        Dialog dialog = ShadowDialog.getLatestDialog();
        assertTrue("a dialog is up", dialog != null && dialog.isShowing());
        return (AlertDialog) dialog;
    }

    @Test
    public void theCountdownNamesTheGameAndThePcWithCancelFocused() throws Exception {
        open(true, null, null);
        AlertDialog dialog = shown();
        assertEquals("Starting on Tower in 3", ((TextView) dialog.findViewById(android.R.id.message)).getText().toString());
        assertTrue("Cancel takes the first press", dialog.getButton(AlertDialog.BUTTON_NEGATIVE).isFocused());
        pass(1200);
        assertEquals("Starting on Tower in 2", ((TextView) dialog.findViewById(android.R.id.message)).getText().toString());
    }

    @Test
    public void cancelClosesThePageAndStartsNothingWithTheGuardUntouched() throws Exception {
        open(true, null, null);
        shown().getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
        pass(100);
        assertTrue(screen.isFinishing());
        pass(5000);
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
        assertFalse(OpenPlay.armed(context));
    }

    @Test
    public void backAndBAndEscapeCancelToo() throws Exception {
        for (int key : new int[] {KeyEvent.KEYCODE_BACK, KeyEvent.KEYCODE_BUTTON_B, KeyEvent.KEYCODE_ESCAPE}) {
            ShadowDialog.reset();
            open(true, null, null);
            AlertDialog dialog = shown();
            dialog.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, key));
            dialog.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, key));
            pass(100);
            assertTrue("key " + key, screen.isFinishing());
            assertFalse("key " + key, dialog.isShowing());
        }
        pass(5000);
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void anOnlinePcStartsTheGameWithTheGuardArmedUntilTheStreamClearsIt() throws Exception {
        open(true, null, null);
        long end = ReflectionHelpers.<Long>getField(screen, "countdownEnd");
        pass(end - android.os.SystemClock.elapsedRealtime() - 1);
        assertTrue("not before the countdown ends", GameDetailWakeTest.Detail.played.isEmpty());
        assertFalse(OpenPlay.armed(context));
        pass(101);
        assertTrue(OpenPlay.armed(context));
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty());
        assertEquals("Portal", GameDetailWakeTest.Detail.played.get(0));
        assertEquals("once", 1, GameDetailWakeTest.Detail.played.size());
        assertTrue(OpenPlay.armed(context));
    }

    /** The page as it ships: {@code play} itself runs, up to the stream's intent. */
    public static class RealPlay extends GameDetailActivity {
        @Override public boolean bindService(Intent intent, android.content.ServiceConnection connection, int flags) {
            return false;
        }

        @Override protected com.limelight.nativeapi.ConsoleRepository createRepository(String uniqueId) {
            return GameDetailWakeTest.Detail.repository;
        }
    }

    @Test
    public void theCountdownsEndStartsTheStreamAsAnAutomaticStartAndLeavesThePage() throws Exception {
        rig = new ConsoleRig(temporary.newFolder());
        rig.transport.reply("tower", "library", ok(library("r", game("t-portal", 11, "Portal", "steam:400", false))));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
        rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        GameDetailWakeTest.Detail.repository = rig.repository;
        Intent intent = new Intent(context, RealPlay.class)
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, "tower")
                .putExtra(GameDetailActivity.EXTRA_APP_ID, 11)
                .putExtra(GameDetailActivity.EXTRA_TITLE, "Portal")
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, "Tower")
                .putExtra(GameDetailActivity.EXTRA_OPEN_PLAY, true);
        RealPlay page = Robolectric.buildActivity(RealPlay.class, intent).setup().visible().get();
        HostFeed pageFeed = ReflectionHelpers.getField(page, "feed");
        ReflectionHelpers.setField(pageFeed, "binder", binder());
        pageFeed.update(nativeHost("tower", "Tower", true));
        pass(3100);
        await(() -> shadowOf(page).peekNextStartedActivity() != null);
        Intent stream = shadowOf(page).getNextStartedActivity();
        assertEquals(com.limelight.utils.LaunchSource.OPEN_PLAY, com.limelight.utils.LaunchSource.of(stream));
        assertTrue(OpenPlay.isAutoStart(stream));
        assertEquals("Portal", stream.getStringExtra(com.limelight.Game.EXTRA_APP_NAME));
        assertTrue("the stream's end lands on Home", page.isFinishing());
        assertEquals("the automatic part is over", "OFF", ReflectionHelpers.getField(page, "auto").toString());
        assertTrue("armed until the stream clears it", OpenPlay.armed(context));
        pass(5000);
        assertEquals("once", null, shadowOf(page).getNextStartedActivity());
    }

    @Test
    public void onlyAStreamThatLastedClearsTheGuard() {
        OpenPlayModel.use(request -> request.contains("stream_end")
                ? "{\"ok\":{\"disarm\":true}}" : "{\"ok\":{\"countdown_ms\":3000,\"settled_ms\":10000}}");
        android.os.Handler handler = new android.os.Handler(Looper.getMainLooper());
        OpenPlay.arm(context);
        OpenPlay.settleAfter(context, handler);
        pass(9999);
        assertTrue("not yet", OpenPlay.armed(context));
        pass(2);
        assertFalse("it lasted", OpenPlay.armed(context));
        OpenPlay.arm(context);
        handler.removeCallbacks(OpenPlay.settleAfter(context, handler));
        pass(11000);
        assertTrue("it stopped first", OpenPlay.armed(context));
    }

    /** The countdown over with a game reported running on Tower, a host that does not ask itself, whose list fails. */
    private AlertDialog promptOverRunningGame(int runningId, String runningUuid) throws Exception {
        open(true, null, null);
        ComputerDetails tower = NativeApiFixtures.classicHost("tower", "Tower");
        tower.runningGameId = runningId;
        tower.runningGameUuid = runningUuid;
        feed.update(tower);
        Dialog countdown = shown();
        pass(3100);
        await(() -> ShadowDialog.getLatestDialog() != countdown && ShadowDialog.getLatestDialog().isShowing());
        return shown();
    }

    @Test
    public void anAutomaticStartNeverReplacesAGameTheModelMissed() throws Exception {
        AlertDialog prompt = promptOverRunningGame(99, null);
        assertEquals(context.getString(R.string.library_replace_message, "Portal"),
                ((TextView) prompt.findViewById(android.R.id.message)).getText().toString());
        pass(5000);
        Thread.sleep(200);
        pass(1000);
        assertTrue("nothing starts without the player", GameDetailWakeTest.Detail.played.isEmpty());
        assertFalse("the player decides now", OpenPlay.armed(context));
        assertFalse(screen.isFinishing());
    }

    @Test
    public void thePlayersYesInThatPromptStartsTheGame() throws Exception {
        promptOverRunningGame(99, null).getButton(AlertDialog.BUTTON_POSITIVE).performClick();
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty());
        assertEquals("Portal", GameDetailWakeTest.Detail.played.get(0));
    }

    @Test
    public void aGameOnlyItsUuidTellsApartIsAskedAboutToo() throws Exception {
        promptOverRunningGame(0, "uuid-of-another-game").getButton(AlertDialog.BUTTON_POSITIVE).performClick();
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty());
        assertEquals("the game named is still what runs, so the start goes on", "Portal", GameDetailWakeTest.Detail.played.get(0));
    }

    @Test
    public void thisGameRunningThereIsNoReplacementSoItStartsWithoutAPrompt() throws Exception {
        open(true, null, null);
        rig.classicApps.put("tower", Collections.singletonList(new NvApp("Portal", 11, false)));
        ComputerDetails tower = NativeApiFixtures.classicHost("tower", "Tower");
        tower.runningGameId = 11;
        feed.update(tower);
        pass(3100);
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void aPromptOfTheModelsOwnNamesTheGame() throws Exception {
        open(true, null, null);
        rig.classicApps.put("tower", java.util.Arrays.asList(new NvApp("Portal", 11, false), new NvApp("Zelda", 22, false)));
        ComputerDetails tower = NativeApiFixtures.classicHost("tower", "Tower");
        tower.runningGameId = 22;
        feed.update(tower);
        Dialog countdown = shown();
        pass(3100);
        await(() -> ShadowDialog.getLatestDialog() != countdown && ShadowDialog.getLatestDialog().isShowing());
        assertEquals(context.getString(R.string.library_replace_message_running, "Zelda", "Portal"),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void aPollsRedrawOfWhatThePageHasIsNotEnoughToAct() throws Exception {
        open(true, null, null);
        await(() -> ReflectionHelpers.<Boolean>getField(screen, "modelLive"));
        // What a poll's reload does: the saved page again, without a read of the PC.
        ReflectionHelpers.setField(screen, "modelLive", false);
        int reads = rig.transport.calls.size();
        pass(3100);
        assertTrue("the countdown's end", OpenPlay.armed(context));
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty());
        assertTrue("it read the PC before it started anything", rig.transport.calls.size() > reads);
    }

    @Test
    public void anOfflinePcIsWokenBeforeTheGame() throws Exception {
        open(false, "00:11:22:33:44:55", null);
        pass(3100);
        await(() -> !GameDetailWakeTest.Detail.woken.isEmpty());
        assertEquals("tower", GameDetailWakeTest.Detail.woken.get(0));
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void leavingThePageDuringTheWakeIsThePlayersChoice() throws Exception {
        open(false, "00:11:22:33:44:55", null);
        pass(3100);
        await(() -> !GameDetailWakeTest.Detail.woken.isEmpty());
        assertTrue(OpenPlay.armed(context));
        controller.pause();
        assertFalse(OpenPlay.armed(context));
    }

    @Test
    public void aFailedStartSaysSoLandsOnHomeAndKeepsTheGuardArmed() throws Exception {
        open(false, null, null);
        Dialog countdown = shown();
        pass(3100);
        assertTrue(OpenPlay.armed(context));
        await(() -> ShadowDialog.getLatestDialog() != countdown && ShadowDialog.getLatestDialog().isShowing());
        assertFalse("the page stays until the message is read", screen.isFinishing());
        shown().getButton(AlertDialog.BUTTON_POSITIVE).performClick();
        pass(100);
        assertTrue(screen.isFinishing());
        assertTrue(OpenPlay.armed(context));
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    /** What Tower's fresh answer has the automatic start say, with Tower online: {@code library} replaces its list. */
    private String failureWith(String library) throws Exception {
        open(true, null, null);
        rig.transport.reply("tower", "library", ok(library));
        Dialog countdown = shown();
        pass(3100);
        assertTrue(OpenPlay.armed(context));
        await(() -> ShadowDialog.getLatestDialog() != countdown && ShadowDialog.getLatestDialog().isShowing());
        assertFalse(screen.isFinishing());
        String said = ((TextView) shown().findViewById(android.R.id.message)).getText().toString();
        shown().getButton(AlertDialog.BUTTON_POSITIVE).performClick();
        pass(100);
        assertTrue("on to Home", screen.isFinishing());
        assertTrue("a real failure: the guard stays", OpenPlay.armed(context));
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
        return said;
    }

    @Test
    public void aGameThePcCannotStartSaysWhyAndNotThatThePcIsOffline() throws Exception {
        assertEquals("Not installed on Tower", failureWith(library("r2", portalWith("\"installed\":false"))));
    }

    @Test
    public void aGameThePcCannotLaunchSaysThat() throws Exception {
        assertEquals("Tower can't start this game", failureWith(library("r2", portalWith("\"launchable\":false"))));
    }

    /** Portal as the library lists it, with this field where it says it is installed. */
    private static String portalWith(String field) {
        return game("t-portal", 11, "Portal", "steam:400", false).replace("\"installed\":true", field);
    }

    @Test
    public void aGameGoneFromTheLibraryFailsCleanlyInsteadOfLaunchingAStaleId() throws Exception {
        assertEquals(context.getString(R.string.open_play_not_listed, "Tower"),
                failureWith(library("r2", game("t-zelda", 22, "Zelda", "steam:500", false))));
    }

    @Test
    public void aTileOnlyTheClassicListHasStillStarts() throws Exception {
        open(true, null, null);
        rig.classicApps.put("tower", Collections.singletonList(new NvApp("Desktop", 11, false)));
        rig.transport.reply("tower", "library", ok(library("r2", game("t-zelda", 22, "Zelda", "steam:500", false))));
        pass(3100);
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void aListThatCouldNotBeReadSaysSoAndStartsNoSavedId() throws Exception {
        open(true, null, null);
        feed.update(NativeApiFixtures.classicHost("tower", "Tower"));
        Dialog countdown = shown();
        pass(3100);
        await(() -> ShadowDialog.getLatestDialog() != countdown && ShadowDialog.getLatestDialog().isShowing());
        assertEquals(context.getString(R.string.open_play_no_list, "Tower"),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    /** The countdown over with this session on Tower: the one line the page lands on. */
    private String startWithSession(String sessionJson) throws Exception {
        sessionThere = sessionJson;
        open(true, null, null);
        Dialog countdown = shown();
        pass(3100);
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty()
                || (ShadowDialog.getLatestDialog() != countdown && ShadowDialog.getLatestDialog().isShowing()));
        return GameDetailWakeTest.Detail.played.isEmpty()
                ? ((TextView) shown().findViewById(android.R.id.message)).getText().toString() : null;
    }

    @Test
    public void aGamePlayingOnAnotherDeviceStopsTheStartAndLandsOnHome() throws Exception {
        for (String app : new String[] {PORTAL_APP, ZELDA_APP}) {
            ShadowDialog.reset();
            assertEquals(app, context.getString(R.string.open_play_other_device, "Tower"),
                    startWithSession(NativeApiFixtures.session("streaming", "other_device", app, "[]")));
            assertTrue("never moved", GameDetailWakeTest.Detail.played.isEmpty());
            assertFalse("nothing failed, so the next start is not skipped", OpenPlay.armed(context));
            shown().dismiss();
            pass(100);
            assertTrue(app, screen.isFinishing());
        }
    }

    @Test
    public void aSessionThatEndedOrIsThisDevicesDoesNotStopTheStart() throws Exception {
        assertEquals(null, startWithSession(NativeApiFixtures.session("streaming", "you", PORTAL_APP, "[]")));
        GameDetailWakeTest.Detail.played.clear();
        ShadowDialog.reset();
        assertEquals(null, startWithSession(NativeApiFixtures.session("stopped", "other_device", PORTAL_APP, "[]")));
    }

    @Test
    public void aNetworkThatStaysAwayOpensAsUsualAfterAMinuteWithoutACountdownOrTheGuard() throws Exception {
        android.net.ConnectivityManager connectivity = (android.net.ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        shadowOf(connectivity).setActiveNetworkInfo(null);
        open(true, null, null);
        assertEquals(context.getString(R.string.open_play_waiting_network),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        pass(59_000);
        assertEquals(context.getString(R.string.open_play_waiting_network),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        pass(1_500);
        assertEquals(context.getString(R.string.open_play_no_network),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        pass(5000);
        assertFalse(OpenPlay.armed(context));
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
        shown().dismiss();
        pass(100);
        assertTrue(screen.isFinishing());
    }

    @Test
    public void aNetworkThatComesBackGivesThePcAPollAndThenTheFullCountdown() throws Exception {
        android.net.ConnectivityManager connectivity = (android.net.ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        android.net.NetworkInfo network = connectivity.getActiveNetworkInfo();
        shadowOf(connectivity).setActiveNetworkInfo(null);
        open(true, null, null);
        pass(30_000);
        shadowOf(connectivity).setActiveNetworkInfo(network);
        pass(5_000);
        assertEquals(context.getString(R.string.open_play_waiting_network),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        pass(1_500);
        assertEquals("Starting on Tower in 3", ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertFalse(OpenPlay.armed(context));
        pass(3100);
        assertTrue(OpenPlay.armed(context));
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void whatThePcSaysIsReadAgainOnceTheNetworkIsBack() throws Exception {
        android.net.ConnectivityManager connectivity = (android.net.ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        android.net.NetworkInfo network = connectivity.getActiveNetworkInfo();
        shadowOf(connectivity).setActiveNetworkInfo(null);
        open(true, null, null);
        await(() -> {
            pass(100);
            return ReflectionHelpers.<Boolean>getField(screen, "modelLive");
        });
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.session("streaming", "other_device", ZELDA_APP, "[]")));
        shadowOf(connectivity).setActiveNetworkInfo(network);
        Dialog waiting = shown();
        await(() -> {
            pass(500);
            return ShadowDialog.getLatestDialog() != waiting && ((TextView) shown().findViewById(android.R.id.message)).getText()
                    .toString().equals(context.getString(R.string.open_play_other_device, "Tower"));
        });
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void leavingThePageEndsTheWaitForTheNetwork() throws Exception {
        android.net.ConnectivityManager connectivity = (android.net.ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        android.net.NetworkInfo network = connectivity.getActiveNetworkInfo();
        shadowOf(connectivity).setActiveNetworkInfo(null);
        open(true, null, null);
        pass(1000);
        controller.pause();
        shadowOf(connectivity).setActiveNetworkInfo(network);
        pass(70_000);
        Thread.sleep(200);
        pass(100);
        assertFalse(OpenPlay.armed(context));
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void theWaitForTheNetworkCancelsLikeTheCountdown() throws Exception {
        android.net.ConnectivityManager connectivity = (android.net.ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        shadowOf(connectivity).setActiveNetworkInfo(null);
        open(true, null, null);
        pass(1000);
        shown().getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
        pass(100);
        assertTrue(screen.isFinishing());
        pass(70_000);
        assertFalse(OpenPlay.armed(context));
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    private void unknown() {
        ComputerDetails tower = nativeHost("tower", "Tower", false);
        tower.state = ComputerDetails.State.UNKNOWN;
        feed.update(tower);
    }

    @Test
    public void aPcNotHeardFromYetIsWaitedForAndNotCalledOffline() throws Exception {
        open(true, null, null);
        unknown();
        Dialog countdown = shown();
        pass(3100);
        Thread.sleep(200);
        pass(10_000);
        Thread.sleep(200);
        pass(10_000);
        assertTrue("no failure yet", ShadowDialog.getLatestDialog() == countdown);
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
        feed.update(nativeHost("tower", "Tower", true));
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void aPcNeverHeardFromFailsAtTheLimit() throws Exception {
        open(true, null, null);
        unknown();
        Dialog countdown = shown();
        pass(3100);
        Thread.sleep(200);
        pass(44_000);
        assertTrue("within the limit", ShadowDialog.getLatestDialog() == countdown);
        pass(1_100);
        await(() -> ShadowDialog.getLatestDialog() != countdown);
        assertEquals(context.getString(R.string.error_pc_offline),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertTrue(OpenPlay.armed(context));
    }

    private static final String SESSION_FAILS = "{\"error\":{\"kind\":\"status\",\"message\":\"boom\",\"http_status\":500}}";

    @Test
    public void aSessionThatCouldNotBeReadIsAskedAgainAndAGameOfAnotherDeviceStillStopsTheStart() throws Exception {
        sessionThere = SESSION_FAILS;
        open(true, null, null);
        Dialog countdown = shown();
        pass(3100);
        Thread.sleep(300);
        pass(100);
        Thread.sleep(300);
        pass(100);
        assertTrue("not read as free", GameDetailWakeTest.Detail.played.isEmpty());
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.session("streaming", "other_device", ZELDA_APP, "[]")));
        await(() -> {
            pass(2100);
            return ShadowDialog.getLatestDialog() != countdown;
        });
        assertEquals(context.getString(R.string.open_play_other_device, "Tower"),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void aSessionReadThatRecoversStartsTheGame() throws Exception {
        sessionThere = SESSION_FAILS;
        open(true, null, null);
        pass(3100);
        Thread.sleep(300);
        pass(100);
        assertTrue("a failed read is not a free PC", GameDetailWakeTest.Detail.played.isEmpty());
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
        await(() -> {
            pass(2100);
            return !GameDetailWakeTest.Detail.played.isEmpty();
        });
    }

    @Test
    public void aSessionThatNeverReadsFailsAtTheLimitWithTheGuardArmed() throws Exception {
        sessionThere = SESSION_FAILS;
        open(true, null, null);
        Dialog countdown = shown();
        pass(3100);
        await(() -> {
            pass(2100);
            return ShadowDialog.getLatestDialog() != countdown;
        });
        assertEquals(context.getString(R.string.open_play_no_answer, "Tower"),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
        assertTrue(OpenPlay.armed(context));
    }

    @Test
    public void anotherGameUnderTheSavedAppIdIsNotStartedUnderTheSavedTitle() throws Exception {
        stableThere = "t-portal";
        libraryThere = library("r", game("t-zelda", 11, "Zelda", "steam:500", false));
        open(true, null, null);
        Dialog countdown = shown();
        pass(3100);
        await(() -> ShadowDialog.getLatestDialog() != countdown && ShadowDialog.getLatestDialog().isShowing());
        assertEquals(context.getString(R.string.open_play_not_listed, "Tower"),
                ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
    }

    @Test
    public void aSavedGameTheHostMovedToAnotherAppIdStillStarts() throws Exception {
        stableThere = "t-portal";
        libraryThere = library("r", game("t-zelda", 11, "Zelda", "steam:500", false), game("t-portal", 33, "Portal", "steam:400", false));
        open(true, null, null);
        pass(3100);
        await(() -> !GameDetailWakeTest.Detail.played.isEmpty());
        assertEquals("Portal", GameDetailWakeTest.Detail.played.get(0));
        assertEquals(33, ReflectionHelpers.<GameDetailModel>getField(screen, "model").appId);
    }

    @Test
    public void aRecreatedPageDoesNothingByItself() throws Exception {
        open(true, null, new Bundle());
        assertNull(ShadowDialog.getLatestDialog());
        pass(5000);
        assertTrue(GameDetailWakeTest.Detail.played.isEmpty());
        assertFalse(OpenPlay.armed(context));
    }

    @Test
    public void theAppPinComesFirstAndTheFullCountdownFollowsIt() throws Exception {
        com.limelight.pin.PinGate.resetForTests();
        com.limelight.pin.PinStore store = new com.limelight.pin.PinStore(context);
        store.setPin(new byte[] {1, 2, 3, 4});
        store.setMode(com.limelight.pin.PinStore.Mode.APP);
        try {
            open(true, null, null);
            assertNull("no countdown behind the PIN", ShadowDialog.getLatestDialog());
            pass(5000);
            assertFalse(OpenPlay.armed(context));
            assertTrue(GameDetailWakeTest.Detail.played.isEmpty());

            com.limelight.pin.PinGate.unlockApp();
            controller.pause().resume();
            assertEquals("Starting on Tower in 3", ((TextView) shown().findViewById(android.R.id.message)).getText().toString());
        } finally {
            store.removePin();
            com.limelight.pin.PinGate.resetForTests();
        }
    }
}
