package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static com.limelight.nativeapi.NativeApiFixtures.session;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.NvApp;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.Collections;

/** What Play does: start at once, or ask because another game runs; and how the answer is checked again. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class PlayPlanTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private ConsoleRig rig;

    @Before
    public void setUp() throws Exception {
        rig = new ConsoleRig(temporary.newFolder());
    }

    @After
    public void tearDown() {
        rig.close();
    }

    /** Portal (11) runs; the detail screen is Zelda's (12). */
    private GameDetailModel zeldaWhilePortalRuns() throws Exception {
        rig.transport.reply("tower", "library", ok(library("t1",
                game("t-portal", 11, "Portal", "steam:400", true), game("t-zelda", 12, "Zelda", null, false))));
        rig.transport.reply("tower", "session", ok(session("streaming", "you",
                "{\"install_id\":\"t-portal\",\"app_id\":11,\"title\":\"Portal\"}", "[]")));
        ConsoleSnapshot snapshot = rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        return GameDetailModel.of(snapshot, "tower", 12, "Zelda", "Tower", true, true, true);
    }

    private GameDetailModel zeldaWhileNothingRuns() throws Exception {
        rig.transport.reply("tower", "library", ok(library("t1",
                game("t-portal", 11, "Portal", "steam:400", false), game("t-zelda", 12, "Zelda", null, false))));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
        ConsoleSnapshot snapshot = rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
        return GameDetailModel.of(snapshot, "tower", 12, "Zelda", "Tower", true, true, true);
    }

    @Test
    public void nothingElseRunningStartsAtOnce() throws Exception {
        PlayPlan plan = PlayPlan.of(zeldaWhileNothingRuns(), false, true, true, true);
        assertEquals(PlayPlan.Kind.START, plan.kind);
        assertFalse(plan.closeNatively);
    }

    @Test
    public void anotherGameRunningAsksAndNamesItWithTheFacts() throws Exception {
        PlayPlan plan = PlayPlan.of(zeldaWhilePortalRuns(), false, true, true, true);
        assertEquals(PlayPlan.Kind.ASK, plan.kind);
        assertEquals("Portal", plan.runningTitle);
        assertEquals(11, plan.runningAppId);
        assertTrue(plan.canResume);
        assertEquals("Portal", plan.runningApp().getAppName());
        assertEquals(11, plan.runningApp().getAppId());
    }

    @Test
    public void aNativeHostWithHostControlClosesNativelyAndEveryOtherCaseLeavesItToTheClassicQuit() throws Exception {
        GameDetailModel model = zeldaWhilePortalRuns();
        assertTrue(PlayPlan.of(model, false, true, true, true).closeNatively);
        assertFalse(PlayPlan.of(model, false, true, true, false).closeNatively);
        assertFalse(PlayPlan.of(model, false, true, false, true).closeNatively);
    }

    @Test
    public void aHostThatConfirmsReplacementOrADeviceWithoutLaunchPermissionNeverAsksHere() throws Exception {
        GameDetailModel model = zeldaWhilePortalRuns();
        assertEquals(PlayPlan.Kind.START, PlayPlan.of(model, true, true, true, true).kind);
        assertEquals(PlayPlan.Kind.START, PlayPlan.of(model, false, false, true, true).kind);
    }

    @Test
    public void anAutomaticStartTakesThePcsOwnWordForAGameTheModelMissed() throws Exception {
        GameDetailModel idle = GameDetailModel.of(ConsoleSnapshot.empty(), "tower", 11, "Portal", "Tower", true, true, true);
        assertEquals("a tap goes by the model", PlayPlan.Kind.START, PlayPlan.of(idle, false, true, true, true).kind);

        PlayPlan plan = PlayPlan.automatic(idle, false, true, true, true, true, 99, "uuid-other");
        assertEquals(PlayPlan.Kind.ASK, plan.kind);
        assertTrue("through the native API, as the model's own would", plan.closeNatively);
        assertNull(plan.runningTitle);
        assertFalse(plan.canResume);
        assertEquals("the check before closing it passes", PlayPlan.Check.SAME,
                PlayPlan.recheck(plan.runningApp(), 99, "uuid-other"));

        assertEquals("nothing else runs", PlayPlan.Kind.START,
                PlayPlan.automatic(idle, false, true, true, true, false, 0, null).kind);
        assertEquals("the host asks itself", PlayPlan.Kind.START,
                PlayPlan.automatic(idle, true, true, true, true, true, 99, null).kind);
        assertEquals("nothing can be closed", PlayPlan.Kind.START,
                PlayPlan.automatic(idle, false, false, true, true, true, 99, null).kind);
    }

    @Test
    public void theRunningGameIsCheckedAgainBeforeAnythingIsDoneToIt() {
        NvApp portal = new NvApp("Portal", 11, false);
        assertEquals(PlayPlan.Check.SAME, PlayPlan.recheck(portal, 11, ""));
        assertEquals(PlayPlan.Check.GONE, PlayPlan.recheck(portal, 0, ""));
        assertEquals(PlayPlan.Check.GONE, PlayPlan.recheck(portal, 0, null));
        assertEquals(PlayPlan.Check.CHANGED, PlayPlan.recheck(portal, 12, ""));

        NvApp withUuid = new NvApp("Portal", 11, false);
        withUuid.setAppUuid("uuid-portal");
        assertEquals(PlayPlan.Check.SAME, PlayPlan.recheck(withUuid, 99, "uuid-portal"));
        assertEquals(PlayPlan.Check.CHANGED, PlayPlan.recheck(withUuid, 11, "uuid-other"));
    }
}
