package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.classicHost;
import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static com.limelight.nativeapi.NativeApiFixtures.session;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.Arrays;
import java.util.Collections;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameDetailModelTest {
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

    private ConsoleSnapshot tower(String sessionJson, boolean portalRunning) throws Exception {
        rig.transport.reply("tower", "library", ok(library("t1",
                game("t-portal", 11, "Portal", "steam:400", portalRunning), game("t-zelda", 12, "Zelda", null, false))));
        rig.transport.reply("tower", "session", ok(sessionJson));
        return rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true)));
    }

    private static String running(String state, int appId, String title) {
        return session(state, "you", "{\"install_id\":\"x\",\"app_id\":" + appId + ",\"title\":\"" + title + "\"}", "[]");
    }

    @Test
    public void anIdleGameOnAnOnlineHostIsPlayWithNothingToEnd() throws Exception {
        ConsoleSnapshot snapshot = tower(NativeApiFixtures.IDLE, false);
        GameDetailModel model = GameDetailModel.of(snapshot, "tower", 11, "x", "y", true, true, true);
        assertEquals(GameDetailModel.State.IDLE, model.state);
        assertFalse(model.resumes());
        assertTrue(model.canStart());
        assertEquals(GameDetailModel.EndAction.NONE, model.endAction);
        assertEquals("Portal", model.title);
        assertEquals("Tower", model.hostName);
        assertNotNull(model.source);
        assertNull(model.otherRunningTitle);
    }

    @Test
    public void aRunningGameResumesAndClosesWithHostControl() throws Exception {
        ConsoleSnapshot snapshot = tower(running("streaming", 11, "Portal"), true);
        GameDetailModel model = GameDetailModel.of(snapshot, "tower", 11, "x", "y", true, true, true);
        assertEquals(GameDetailModel.State.RUNNING, model.state);
        assertTrue(model.resumes());
        assertEquals(GameDetailModel.EndAction.CLOSE_GAME, model.endAction);
    }

    @Test
    public void withoutHostControlTheEndActionIsTheClassicQuit() throws Exception {
        ConsoleSnapshot snapshot = tower(running("streaming", 11, "Portal"), true);
        GameDetailModel model = GameDetailModel.of(snapshot, "tower", 11, "x", "y", true, true, false);
        assertEquals(GameDetailModel.State.RUNNING, model.state);
        assertEquals(GameDetailModel.EndAction.QUIT, model.endAction);
    }

    @Test
    public void aSuspendedGameResumes() throws Exception {
        ConsoleSnapshot snapshot = tower(running("suspended", 11, "Portal"), true);
        GameDetailModel model = GameDetailModel.of(snapshot, "tower", 11, "x", "y", true, true, true);
        assertEquals(GameDetailModel.State.SUSPENDED, model.state);
        assertTrue(model.resumes());
        assertEquals(GameDetailModel.EndAction.CLOSE_GAME, model.endAction);
    }

    @Test
    public void aClassicHostAlwaysQuitsTheClassicWay() throws Exception {
        rig.classicApps.put("old", Arrays.asList(new NvApp("Portal", 11, false), new NvApp("Zelda", 12, false)));
        ComputerDetails old = classicHost("old", "Old PC");
        old.runningGameId = 11;
        ConsoleSnapshot snapshot = rig.repository.refresh(Collections.singletonList(old));
        GameDetailModel model = GameDetailModel.of(snapshot, "old", 11, "x", "y", true, false, true);
        assertEquals(GameDetailModel.State.RUNNING, model.state);
        assertEquals(GameDetailModel.EndAction.QUIT, model.endAction);
    }

    @Test
    public void anOfflineHostCannotStartOrEndAnythingEvenIfItsSavedLibraryHadTheGameRunning() throws Exception {
        tower(running("streaming", 11, "Portal"), true);
        ConsoleSnapshot saved = rig.repository.cached(Collections.singletonList(nativeHost("tower", "Tower", false)));
        GameDetailModel model = GameDetailModel.of(saved, "tower", 11, "x", "y", false, true, true);
        assertEquals(GameDetailModel.State.IDLE, model.state);
        assertFalse(model.canStart());
        assertEquals(GameDetailModel.EndAction.NONE, model.endAction);
        assertNull(model.otherRunningTitle);
        assertEquals("Portal", model.title);
    }

    @Test
    public void playingWhileAnotherGameRunsNamesTheGameItReplaces() throws Exception {
        ConsoleSnapshot snapshot = tower(running("streaming", 12, "Zelda"), false);
        GameDetailModel model = GameDetailModel.of(snapshot, "tower", 11, "x", "y", true, true, true);
        assertEquals(GameDetailModel.State.IDLE, model.state);
        assertEquals("Zelda", model.otherRunningTitle);
        assertEquals(GameDetailModel.EndAction.NONE, model.endAction);
        // The running game itself replaces nothing.
        GameDetailModel zelda = GameDetailModel.of(snapshot, "tower", 12, "x", "y", true, true, true);
        assertNull(zelda.otherRunningTitle);
        assertEquals(GameDetailModel.State.RUNNING, zelda.state);
    }

    @Test
    public void aGameTheLibraryDoesNotListKeepsTheTitleFromTheCard() throws Exception {
        ConsoleSnapshot snapshot = tower(NativeApiFixtures.IDLE, false);
        GameDetailModel model = GameDetailModel.of(snapshot, "tower", 99, "Old Game", "Tower", true, true, false);
        assertNull(model.source);
        assertEquals("Old Game", model.title);
        assertTrue(model.canStart());
    }

    @Test
    public void aHostThatIsGoneKeepsTheNameFromTheCard() {
        GameDetailModel model = GameDetailModel.of(ConsoleSnapshot.empty(), "gone", 1, "Game", "Gone PC", false, true, false);
        assertEquals("Gone PC", model.hostName);
        assertEquals("Game", model.title);
        assertFalse(model.canStart());
    }
}
