package com.limelight.binding.input.pad;

import android.app.Application;
import android.content.Context;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.UiNavigationTest;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi")
public class ModernControllerTest {
    private static final String GAME = "pc-1/app-7";
    private final List<int[]> reports = new ArrayList<>();
    private final HeldInputState.Sink sink = new HeldInputState.Sink() {
        @Override public void key(int key, boolean down) { fail("Gamepad only"); }
        @Override public void mouse(int button, boolean down) { fail("Gamepad only"); }
        @Override public void gamepad(int buttons, short lx, short ly, short rx, short ry, byte lt, byte rt) {
            reports.add(new int[] {buttons, lx, ly, rx, ry, lt & 0xFF, rt & 0xFF});
        }
    };
    private FrameLayout frame;
    private PreferenceConfiguration prefs;

    @Before
    public void setUp() {
        Context app = RuntimeEnvironment.getApplication();
        ModernController.clearLayouts(app);
        UiNavigationTest.Screen screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible().get();
        frame = new FrameLayout(screen);
        screen.setContentView(frame, new FrameLayout.LayoutParams(1600, 720));
        prefs = new PreferenceConfiguration();
        prefs.oscStyle = PreferenceConfiguration.OSC_STYLE_MODERN;
        prefs.oscOpacity = 90;
        // HOME follows the guide button setting; on here so every control is covered.
        prefs.showGuideButton = true;
        layout();
    }

    private void layout() {
        frame.measure(View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY));
        frame.layout(0, 0, 1600, 720);
    }

    private ModernController controller(String game) {
        ModernController controller = new ModernController(frame.getContext(), frame, sink, prefs, game);
        controller.refreshLayout();
        controller.show();
        layout();
        return controller;
    }

    private int[] last() {
        return reports.get(reports.size() - 1);
    }

    @Test public void everyControlIsItsOwnViewInTheStreamFrame() {
        ModernController controller = controller(GAME);
        assertEquals(PadLayout.Control.values().length, frame.getChildCount());
        PadControlView face = controller.view(PadLayout.Control.FACE);
        assertEquals(View.VISIBLE, face.getVisibility());
        assertTrue(face.getWidth() > 0);
        controller.destroy();
        assertEquals(0, frame.getChildCount());
    }

    @Test public void aFaceTouchSendsItsButtonAndLettingGoReleasesIt() {
        PadControlView face = controller(GAME).view(PadLayout.Control.FACE);
        float size = Math.min(face.getWidth(), face.getHeight());
        face.down(0, .5f * size, .78f * size, 100);
        assertEquals(ControllerPacket.A_FLAG, last()[0]);
        // Sliding to the gap between A and B holds both, as a thumb resting on two buttons does.
        face.move(0, .64f * size, .64f * size);
        assertEquals(ControllerPacket.A_FLAG | ControllerPacket.B_FLAG, last()[0]);
        face.up(0, 200);
        assertEquals(0, last()[0]);
    }

    @Test public void twoFingersOnTheFaceClusterHoldTwoButtons() {
        PadControlView face = controller(GAME).view(PadLayout.Control.FACE);
        float size = Math.min(face.getWidth(), face.getHeight());
        face.down(0, .5f * size, .78f * size, 100);
        face.down(1, .5f * size, .22f * size, 110);
        assertEquals(ControllerPacket.A_FLAG | ControllerPacket.Y_FLAG, last()[0]);
        face.up(0, 150);
        assertEquals(ControllerPacket.Y_FLAG, last()[0]);
    }

    @Test public void aFloatingStickCentersUnderTheThumbAndReportsItsPush() {
        PadControlView stick = controller(GAME).view(PadLayout.Control.LEFT_STICK);
        float size = Math.min(stick.getWidth(), stick.getHeight());
        float travel = PadPainter.travel(size);
        reports.clear();
        stick.down(0, size * .3f, size * .7f, 100);
        // Landing is not a push.
        assertTrue(reports.isEmpty() || (last()[1] == 0 && last()[2] == 0));
        stick.move(0, size * .3f + travel, size * .7f);
        assertEquals(32766, last()[1]);
        assertEquals(0, last()[2]);
        stick.up(0, 400);
        assertEquals(0, last()[1]);
    }

    @Test public void aFixedStickPushesFromItsDrawnCenter() {
        prefs.oscFloatingSticks = false;
        PadControlView stick = controller(GAME).view(PadLayout.Control.RIGHT_STICK);
        float size = Math.min(stick.getWidth(), stick.getHeight());
        stick.down(0, stick.getWidth() / 2f, stick.getHeight() / 2f - PadPainter.travel(size), 100);
        assertEquals(0, last()[3]);
        assertEquals(32766, last()[4]);
    }

    @Test public void tappingAStickThenTouchingItAgainHoldsItsClick() {
        PadControlView stick = controller(GAME).view(PadLayout.Control.LEFT_STICK);
        float c = stick.getWidth() / 2f;
        stick.down(0, c, c, 1000);
        stick.up(0, 1080);
        stick.down(0, c, c, 1200);
        assertEquals(ControllerPacket.LS_CLK_FLAG, last()[0] & ControllerPacket.LS_CLK_FLAG);
        stick.up(0, 1500);
        assertEquals(0, last()[0]);
        // A third touch is a new stick touch, not another click.
        stick.down(0, c, c, 1600);
        assertEquals(0, last()[0] & ControllerPacket.LS_CLK_FLAG);
    }

    @Test public void triggersPullFullyAndShouldersHoldTheirButton() {
        ModernController controller = controller(GAME);
        controller.view(PadLayout.Control.RT).down(0, 5, 5, 100);
        assertEquals(255, last()[6]);
        controller.view(PadLayout.Control.LB).down(1, 5, 5, 110);
        assertEquals(ControllerPacket.LB_FLAG, last()[0]);
        assertEquals(255, last()[6]);
        controller.releaseInput();
        assertArrayEquals(new int[7], last());
    }

    @Test public void resendsFollowAChangeLikeTheClassicController() {
        ModernController controller = controller(GAME);
        controller.view(PadLayout.Control.START).down(0, 5, 5, 100);
        int sent = reports.size();
        ShadowLooper.idleMainLooper(100, java.util.concurrent.TimeUnit.MILLISECONDS);
        assertEquals(sent + 3, reports.size());
        assertEquals(ControllerPacket.PLAY_FLAG, last()[0]);
    }

    @Test public void onlyStickClicksLeavesJustL3AndR3() {
        prefs.onlyL3R3 = true;
        ModernController controller = controller(GAME);
        assertEquals(2, frame.getChildCount());
        assertNotNull(controller.view(PadLayout.Control.L3));
        assertNull(controller.view(PadLayout.Control.FACE));
    }

    @Test public void homeShowsOnlyWithTheGuideButtonSetting() {
        prefs.showGuideButton = false;
        assertNull(controller(GAME).view(PadLayout.Control.HOME));
    }

    @Test public void aLayoutSavedForOneGameStaysWithThatGame() {
        ModernController first = controller(GAME);
        PadLayout draft = first.layout().copy();
        draft.setScale(PadLayout.Control.FACE, 1.5f);
        first.save(draft, true);
        first.destroy();
        assertEquals(1.5f, controller(GAME).layout().spec(PadLayout.Control.FACE).scale, 1e-3);
        assertTrue("Another game keeps the shared layout", controller("pc-1/app-8").layout().isDefault());
    }

    @Test public void savingForAllGamesDropsThisGamesOwnLayout() {
        ModernController controller = controller(GAME);
        PadLayout own = controller.layout().copy();
        own.setScale(PadLayout.Control.FACE, 1.5f);
        controller.save(own, true);
        PadLayout shared = controller.layout().copy();
        shared.setScale(PadLayout.Control.FACE, .7f);
        controller.save(shared, false);
        controller.destroy();
        assertEquals(.7f, controller(GAME).layout().spec(PadLayout.Control.FACE).scale, 1e-3);
        assertEquals(.7f, controller("pc-1/app-8").layout().spec(PadLayout.Control.FACE).scale, 1e-3);
        assertFalse(RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE)
                .contains(ModernController.GLOBAL_KEY + ":" + GAME));
    }

    @Test public void aStreamWithoutAGameSavesOnlyTheSharedLayout() {
        ModernController controller = controller(null);
        PadLayout draft = controller.layout().copy();
        draft.spec(PadLayout.Control.LB).hidden = true;
        controller.save(draft, true);
        assertTrue(controller("any").layout().spec(PadLayout.Control.LB).hidden);
    }

    @Test public void editingPausesInputAndCancelKeepsTheOldLayout() {
        ModernController controller = controller(GAME);
        PadControlView face = controller.view(PadLayout.Control.FACE);
        face.down(0, face.getWidth() / 2f, face.getHeight() * .78f, 100);
        controller.startEditing();
        assertTrue(controller.isEditing());
        assertEquals("Editing lets go of what was held", 0, last()[0]);
        assertEquals(View.GONE, face.getVisibility());
        controller.finishEditing(false);
        assertFalse(controller.isEditing());
        assertEquals(View.VISIBLE, face.getVisibility());
        assertTrue(controller.layout().isDefault());
    }

    @Test public void theEditorsDoneKeepsTheDraft() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.select(PadLayout.Control.DPAD);
        editor.scaleBy(.1f);
        editor.toggleHidden();
        controller.finishEditing(true);
        assertEquals(1.1f, controller.layout().spec(PadLayout.Control.DPAD).scale, 1e-3);
        assertEquals(View.GONE, controller.view(PadLayout.Control.DPAD).getVisibility());
        controller.destroy();
        assertTrue("A game without its own layout saves for every game, so another game sees it",
                controller("pc-1/app-8").layout().spec(PadLayout.Control.DPAD).hidden);
    }

    @Test public void theEditorWithNothingPickedChangesEveryControl() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.setOpacity(50);
        editor.scaleBy(-.1f);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            assertEquals(control.name(), 50, editor.draft().spec(control).opacity);
            assertEquals(control.name(), .9f, editor.draft().spec(control).scale, 1e-3);
        }
        editor.toggleHidden();
        assertFalse("Hide needs a picked control", editor.draft().spec(PadLayout.Control.FACE).hidden);
    }

    @Test public void theTopRowLeavesRoomForTheMicrophoneButton() {
        ModernController controller = controller(GAME);
        float bottom = controller.topRowBottom();
        float[] box = controller.layout().bounds(PadLayout.Control.START, 1600, 720,
                frame.getResources().getDisplayMetrics().density, new float[4]);
        assertEquals(box[1] + box[3], bottom, 1);
        controller.hide();
        assertTrue(Float.isNaN(controller.topRowBottom()));
    }

    @Test public void closingTheEditorTellsTheMicrophoneButtonToMove() {
        ModernController controller = controller(GAME);
        int[] runs = {0};
        controller.setOnLayoutChanged(() -> runs[0]++);
        controller.startEditing();
        controller.finishEditing(false);
        controller.startEditing();
        controller.finishEditing(true);
        assertEquals("Cancel and Done both report", 2, runs[0]);
        controller.finishEditing(true);
        assertEquals("Nothing to close, nothing reported", 2, runs[0]);
    }

    /** A portrait phone: Hide and Reset turn into 48 dp icons so Cancel and Done stay whole on the row. */
    @Test @Config(qualifiers = "w360dp-h800dp-port-xhdpi")
    public void aNarrowEditorKeepsEveryButtonWholeAndTouchable() {
        frame.getLayoutParams().width = 720;
        frame.getLayoutParams().height = 1600;
        ModernController controller = new ModernController(frame.getContext(), frame, sink, prefs, GAME);
        controller.refreshLayout();
        controller.startEditing();
        for (int pass = 0; pass < 2; pass++) {
            ShadowLooper.idleMainLooper();
            frame.measure(View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY));
            frame.layout(0, 0, 720, 1600);
        }
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.select(PadLayout.Control.DPAD);
        ShadowLooper.idleMainLooper();
        float density = frame.getResources().getDisplayMetrics().density;
        View row = (View) editor.findViewById(com.limelight.R.id.pad_editor_done).getParent();
        assertTrue("The editor laid out", row.getWidth() > 0);
        int[] ids = {com.limelight.R.id.pad_editor_hide, com.limelight.R.id.pad_editor_reset,
                com.limelight.R.id.pad_editor_cancel, com.limelight.R.id.pad_editor_done};
        for (int id : ids) {
            com.google.android.material.button.MaterialButton button = editor.findViewById(id);
            String name = frame.getResources().getResourceEntryName(id);
            assertTrue(name + " is " + button.getWidth() / density + " dp wide", button.getWidth() >= 48 * density - 1);
            assertTrue(name + " is " + button.getHeight() / density + " dp tall", button.getHeight() >= 48 * density - 1);
            assertTrue(name + " ends inside the panel", button.getRight() <= row.getWidth());
        }
        com.google.android.material.button.MaterialButton hide = editor.findViewById(com.limelight.R.id.pad_editor_hide);
        assertEquals("Icon alone", 0, hide.getText().length());
        assertEquals("Still named for TalkBack", frame.getContext().getString(com.limelight.R.string.pad_edit_hide),
                hide.getContentDescription());
        controller.destroy();
    }
}
