package com.limelight.binding.input.pad;

import android.app.Application;
import android.content.Context;
import android.view.MotionEvent;
import android.view.View;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;

import static org.junit.Assert.*;

/**
 * {@link ModernController} layout editor tests: Add tiles, corner resize handles, Split/Join,
 * movable panel placement, alignment guides, and portrait/tallest-state panel bounds.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi")
public class ModernControllerEditorTest extends ModernControllerHarness {
    @Test public void theEditorAddsTheWasdStickFromItsTile() {
        ModernController controller = controller(GAME);
        assertEquals(View.GONE, controller.view(PadLayout.Control.WASD).getVisibility());
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View tile = editor.tile(PadLayout.Control.WASD);
        assertNotNull("Left out, it has an Add tile", tile);
        assertEquals("Add WASD stick", tile.getContentDescription().toString());
        assertTrue(tile.isFocusable());
        assertTrue(tile.performClick());
        assertEquals(PadLayout.Control.WASD, editor.selected());
        assertFalse(editor.draft().spec(PadLayout.Control.WASD).hidden);
        assertNull("Added, the tile goes", editor.tile(PadLayout.Control.WASD));
        editor.toggleHidden();
        assertNotNull("Hidden again, it comes back", editor.tile(PadLayout.Control.WASD));
        assertTrue(editor.tile(PadLayout.Control.WASD).performClick());
        controller.finishEditing(true);
        layout();
        assertEquals(View.VISIBLE, controller.view(PadLayout.Control.WASD).getVisibility());
        assertFalse(controller("pc-1/app-8").layout().spec(PadLayout.Control.WASD).hidden);
    }

    /** Left out, a control is off the surface: a tap where it would sit is empty space and drops the pick. */
    @Test public void theEditorsEmptyMiddleDeselectsUntilTheWasdStickIsAdded() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        layout();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View surface = editor.getChildAt(0);
        float density = frame.getContext().getResources().getDisplayMetrics().density;
        float[] box = new float[4];
        editor.draft().bounds(PadLayout.Control.WASD, surface.getWidth(), surface.getHeight(), density, box);
        float x = box[0] + box[2] / 2, y = box[1] + box[3] / 2;

        editor.select(PadLayout.Control.FACE);
        tap(surface, x, y);
        assertNull("Empty space deselects", editor.selected());

        // Hidden, a control leaves the surface for its Add tile: a tap where it was is empty space too.
        editor.select(PadLayout.Control.HOME);
        editor.toggleHidden();
        assertEquals("Still picked, so Show undoes it", PadLayout.Control.HOME, editor.selected());
        editor.draft().bounds(PadLayout.Control.HOME, surface.getWidth(), surface.getHeight(), density, box);
        tap(surface, box[0] + box[2] / 2, box[1] + box[3] / 2);
        assertNull(editor.selected());
        assertNotNull(editor.tile(PadLayout.Control.HOME));

        editor.addWasd();
        editor.select(null);
        tap(surface, x, y);
        assertEquals("Added, it is picked like any control", PadLayout.Control.WASD, editor.selected());
        controller.finishEditing(false);
    }

    /**
     * The picked control's corner handles size its width and height independently, the opposite corner staying put,
     * and the editor's Done keeps the shape.
     */
    @Test public void aCornerHandleSizesWidthAndHeightIndependently() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        layout();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View surface = editor.getChildAt(0);
        float density = frame.getContext().getResources().getDisplayMetrics().density;
        float outline = PadPainter.OUTLINE_DP * density;
        editor.select(PadLayout.Control.FACE);
        float[] before = editor.draft().bounds(PadLayout.Control.FACE, surface.getWidth(), surface.getHeight(), density,
                new float[4]);

        // The top-left handle, dragged 200 px to the left: wider, as tall as before, the bottom-right corner kept.
        drag(surface, before[0] - outline, before[1] - outline, before[0] - outline - 200, before[1] - outline);
        assertEquals("The pick stays", PadLayout.Control.FACE, editor.selected());
        float[] wide = editor.draft().bounds(PadLayout.Control.FACE, surface.getWidth(), surface.getHeight(), density,
                new float[4]);
        assertEquals("The right edge stays", before[0] + before[2], wide[0] + wide[2], .5f);
        assertEquals("The bottom edge stays", before[1] + before[3], wide[1] + wide[3], .5f);
        assertEquals("200 px wider, on the 8 dp grid", before[2] + 200, wide[2], PadEditor.GRID_DP * density / 2 + .5f);
        assertEquals("As tall, give or take the grid", before[3], wide[3], PadEditor.GRID_DP * density / 2 + .5f);
        PadLayout.Spec spec = editor.draft().spec(PadLayout.Control.FACE);
        assertTrue(spec.widthScale + " x " + spec.heightScale, spec.widthScale > spec.heightScale + .5f);

        // The same handle dragged down: only the height changes.
        float widthScale = spec.widthScale;
        drag(surface, wide[0] - outline, wide[1] - outline, wide[0] - outline, wide[1] - outline + 100);
        float[] low = editor.draft().bounds(PadLayout.Control.FACE, surface.getWidth(), surface.getHeight(), density,
                new float[4]);
        assertEquals(widthScale, spec.widthScale, 1e-4);
        assertEquals(wide[3] - 100, low[3], PadEditor.GRID_DP * density / 2 + .5f);
        assertEquals("The bottom edge still stays", before[1] + before[3], low[1] + low[3], .5f);

        // A handle cannot shrink a control below a full touch, nor drag it past the surface.
        drag(surface, low[0] - outline, low[1] - outline, low[0] + 2000, low[1] + 2000);
        float[] least = editor.draft().bounds(PadLayout.Control.FACE, surface.getWidth(), surface.getHeight(), density,
                new float[4]);
        assertTrue(least[2] >= PadLayout.MIN_TOUCH_DP * density - .5f && least[3] >= PadLayout.MIN_TOUCH_DP * density - .5f);
        drag(surface, least[0] - outline, least[1] - outline, -500, -500);
        float[] most = editor.draft().bounds(PadLayout.Control.FACE, surface.getWidth(), surface.getHeight(), density,
                new float[4]);
        assertEquals("Up to the surface's corner", 0, most[0], .5f);
        assertEquals(0, most[1], .5f);
        assertEquals(before[0] + before[2], most[0] + most[2], .5f);

        PadLayout.Spec kept = new PadLayout().spec(PadLayout.Control.FACE);
        kept.copyFrom(spec);
        controller.finishEditing(true);
        PadLayout.Spec saved = controller(GAME).layout().spec(PadLayout.Control.FACE);
        assertEquals(kept.widthScale, saved.widthScale, 1e-3);
        assertEquals(kept.heightScale, saved.heightScale, 1e-3);
    }

    /**
     * The editor's Split turns the picked face cluster into its four buttons, picked and named in the glyph set, and
     * Join turns them back; Done keeps either, and the stream shows the cluster or its members, never both.
     */
    @Test public void theEditorSplitsAClusterAndJoinsItAgain() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        settle();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View split = editor.findViewById(com.limelight.R.id.pad_editor_split);
        View join = editor.findViewById(com.limelight.R.id.pad_editor_join);
        assertEquals(View.GONE, split.getVisibility());
        editor.select(PadLayout.Control.LB);
        assertEquals("Only the clusters split", View.GONE, split.getVisibility());
        editor.select(PadLayout.Control.FACE);
        assertEquals(View.VISIBLE, split.getVisibility());
        assertEquals(View.GONE, join.getVisibility());
        assertTrue(split.performClick());
        assertTrue(editor.draft().isSplit(PadLayout.Control.FACE));
        assertEquals("The first member is picked", PadLayout.Control.A, editor.selected());
        assertEquals(View.GONE, split.getVisibility());
        assertEquals(View.VISIBLE, join.getVisibility());
        android.widget.TextView name = editor.findViewById(com.limelight.R.id.pad_editor_selection);
        assertEquals("A button", name.getText().toString());
        editor.draft().setGlyph(PadLayout.Glyph.PLAYSTATION);
        editor.select(PadLayout.Control.B);
        assertEquals("◯ button", name.getText().toString());
        editor.draft().setGlyph(PadLayout.Glyph.XBOX);
        controller.finishEditing(true);
        layout();
        assertEquals(View.GONE, controller.view(PadLayout.Control.FACE).getVisibility());
        assertEquals(View.VISIBLE, controller.view(PadLayout.Control.X).getVisibility());

        controller.startEditing();
        settle();
        editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.select(PadLayout.Control.Y);
        join = editor.findViewById(com.limelight.R.id.pad_editor_join);
        assertTrue(join.performClick());
        assertEquals(PadLayout.Control.FACE, editor.selected());
        assertFalse(editor.draft().isSplit(PadLayout.Control.FACE));
        controller.finishEditing(true);
        layout();
        assertEquals(View.VISIBLE, controller.view(PadLayout.Control.FACE).getVisibility());
        for (PadLayout.Control member : PadLayout.members(PadLayout.Control.FACE)) {
            assertEquals(member.name(), View.GONE, controller.view(member).getVisibility());
        }
        controller.destroy();
    }

    /**
     * The editor's panel moves by its grip anywhere on the screen, then no longer moves out of a picked control's way
     * by itself, and the editor reopens with it where it was left.
     */
    @Test public void theEditorPanelMovesByItsGripAndReopensWhereItWasLeft() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        settle();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View panel = editor.panel();
        View grip = editor.findViewById(com.limelight.R.id.pad_editor_grip);
        assertEquals(frame.getContext().getString(com.limelight.R.string.pad_edit_move_panel), grip.getContentDescription());
        float x = panel.getX(), y = panel.getY();
        assertTrue("The panel laid out in the middle", panel.getWidth() > 0 && x >= 300);

        drag(grip, 100, 10, 100 - 300, 10 + 20);
        assertEquals(x - 300, panel.getX(), 1);
        assertEquals(y + 20, panel.getY(), 1);
        settle();
        assertEquals("Laid out again, it stays", x - 300, panel.getX(), 1);
        assertEquals(y + 20, panel.getY(), 1);
        // The left stick sits under it now; picking it leaves the placed panel alone.
        editor.select(PadLayout.Control.LEFT_STICK);
        settle();
        assertEquals(x - 300, panel.getX(), 1);
        assertEquals(y + 20, panel.getY(), 1);

        // Dragged far past an edge, it stops there, whole.
        drag(grip, 100, 10, 100 - 5000, 10 - 5000);
        assertEquals(0, panel.getX(), 1);
        assertEquals(0, panel.getY(), 1);
        drag(grip, 100, 10, 100 + 300, 10 + 40);
        settle();
        float leftX = panel.getX(), leftY = panel.getY();
        assertEquals(300, leftX, 1);
        assertEquals(40, leftY, 1);

        // Cancel or Done, the editor opens again with the panel where it was left.
        controller.finishEditing(false);
        controller.startEditing();
        settle();
        PadEditor again = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        assertNotSame(editor, again);
        assertEquals(leftX, again.panel().getX(), 1);
        assertEquals(leftY, again.panel().getY(), 1);
        controller.finishEditing(true);
    }

    /** A remembered place that no longer fits (a larger screen, a fold) shows the panel whole at the nearest edge. */
    @Test public void aRememberedPanelPlaceOffTheScreenKeepsThePanelOnIt() {
        RuntimeEnvironment.getApplication().getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE).edit()
                .putString(PadLayoutKeys.panelKey(true), "5000,-300").commit();
        ModernController controller = controller(GAME);
        controller.startEditing();
        settle();
        View panel = ((PadEditor) frame.getChildAt(frame.getChildCount() - 1)).panel();
        assertTrue(panel.getWidth() > 0);
        assertEquals(frame.getWidth() - panel.getWidth(), panel.getX(), 1);
        assertEquals(0, panel.getY(), 1);
        assertTrue(panel.getY() + panel.getHeight() <= frame.getHeight());
        controller.finishEditing(false);
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
        assertEquals(1.1f, controller.layout().spec(PadLayout.Control.DPAD).scale(), 1e-3);
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
            // The Steam Controller controls are off without their setting, so the editor leaves them alone.
            if (control.steam) continue;
            assertEquals(control.name(), 50, editor.draft().spec(control).opacity);
            assertEquals(control.name(), .9f, editor.draft().spec(control).scale(), 1e-3);
        }
        editor.toggleHidden();
        assertFalse("Hide needs a picked control", editor.draft().spec(PadLayout.Control.FACE).hidden);
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
        // Reset, Cancel and Done share the bottom row, Reset as its icon; Hide keeps its name in the picked control's row.
        for (int id : new int[] {com.limelight.R.id.pad_editor_reset, com.limelight.R.id.pad_editor_cancel}) {
            assertSame(row, editor.findViewById(id).getParent());
        }
        com.google.android.material.button.MaterialButton reset = editor.findViewById(com.limelight.R.id.pad_editor_reset);
        assertEquals("Icon alone", 0, reset.getText().length());
        assertEquals("Still named for TalkBack", frame.getContext().getString(com.limelight.R.string.pad_edit_reset),
                reset.getContentDescription());
        com.google.android.material.button.MaterialButton hide = editor.findViewById(com.limelight.R.id.pad_editor_hide);
        assertEquals(frame.getContext().getString(com.limelight.R.string.pad_edit_hide), hide.getText().toString());
        controller.destroy();
    }

    @Test public void theEditorAddsThePointerTouchpadAndSetsItsMode() {
        ModernController controller = controller(GAME);
        assertEquals(View.GONE, controller.view(PadLayout.Control.POINTER_PAD).getVisibility());
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View tile = editor.tile(PadLayout.Control.POINTER_PAD);
        assertNotNull(tile);
        assertEquals(PadEditor.KEYBOARD_MOUSE, PadEditor.addGroup(PadLayout.Control.POINTER_PAD));
        assertTrue(tile.performClick());
        assertEquals(PadLayout.Control.POINTER_PAD, editor.selected());
        assertFalse(editor.draft().spec(PadLayout.Control.POINTER_PAD).hidden);
        View modes = editor.findViewById(com.limelight.R.id.pad_editor_pointer_mode);
        assertEquals("Added, the pad stays picked with its modes at hand", View.VISIBLE, modes.getVisibility());
        View mouse = editor.findViewById(com.limelight.R.id.pad_editor_mode_mouse);
        View camera = editor.findViewById(com.limelight.R.id.pad_editor_mode_camera);
        assertTrue(mouse.isFocusable() && camera.isFocusable());
        assertTrue("The card of the mode it has is marked", mouse.isSelected());
        assertFalse(camera.isSelected());
        assertTrue(camera.performClick());
        assertEquals(PadLayout.PointerMode.CAMERA, editor.draft().pointerMode());
        assertTrue(camera.isSelected());
        assertFalse(mouse.isSelected());
        assertTrue(mouse.performClick());
        assertEquals(PadLayout.PointerMode.MOUSE, editor.draft().pointerMode());
        assertTrue(mouse.isSelected());
        controller.finishEditing(true);
        layout();
        assertEquals(View.VISIBLE, controller.view(PadLayout.Control.POINTER_PAD).getVisibility());
    }

    /**
     * Camera mode is easy to find: the editor's touchpad shows its mode's word as it does in play, and with the pad
     * picked the panel's line says what Camera does, even on a phone held sideways, where the line is otherwise left out.
     */
    @Test public void theEditorShowsTheTouchpadsModeAndSaysWhatCameraDoes() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        settle();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        android.widget.TextView hint = editor.findViewById(com.limelight.R.id.pad_editor_hint);
        assertEquals("A phone held sideways leaves the line out", View.GONE, hint.getVisibility());
        editor.addTouchpad();
        assertEquals("MOUSE", editor.previewState(PadLayout.Control.POINTER_PAD).label);
        assertEquals("", editor.previewState(PadLayout.Control.FACE).label);
        assertEquals(View.VISIBLE, hint.getVisibility());
        assertEquals(frame.getContext().getString(com.limelight.R.string.pad_edit_hint_camera), hint.getText().toString());
        editor.togglePointerMode();
        assertEquals("CAM", editor.previewState(PadLayout.Control.POINTER_PAD).label);
        editor.select(PadLayout.Control.LB);
        assertEquals(View.GONE, hint.getVisibility());
        assertEquals(frame.getContext().getString(com.limelight.R.string.pad_edit_hint), hint.getText().toString());
        controller.finishEditing(false);
    }

    /**
     * The Add tiles offer what the layout leaves out, grouped: the WASD stick and the pointer touchpad at first, a
     * hidden button once hidden. A split cluster is not offered; its members are once hidden, and adding one never
     * shows the cluster with its members.
     */
    @Test public void theAddTilesOfferWhatTheLayoutLeavesOut() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            boolean offered = control == PadLayout.Control.WASD || control == PadLayout.Control.POINTER_PAD;
            assertEquals(control.name(), offered, editor.tile(control) != null);
        }
        assertEquals(PadEditor.KEYBOARD_MOUSE, PadEditor.addGroup(PadLayout.Control.WASD));
        assertEquals(PadEditor.GAMEPAD, PadEditor.addGroup(PadLayout.Control.L3));
        assertEquals(PadEditor.GAMEPAD, PadEditor.addGroup(PadLayout.Control.A));
        assertEquals(PadEditor.EXTRAS, PadEditor.addGroup(PadLayout.Control.TOUCHPAD));
        assertEquals(PadEditor.EXTRAS, PadEditor.addGroup(PadLayout.Control.LEFT_PAD));

        editor.select(PadLayout.Control.L3);
        editor.toggleHidden();
        assertNotNull("Hidden, it is offered", editor.tile(PadLayout.Control.L3));

        editor.select(PadLayout.Control.FACE);
        editor.split();
        assertNull("A split cluster is not offered", editor.tile(PadLayout.Control.FACE));
        for (PadLayout.Control member : PadLayout.members(PadLayout.Control.FACE)) assertNull(editor.tile(member));
        editor.select(PadLayout.Control.A);
        editor.toggleHidden();
        assertNotNull("A hidden member is", editor.tile(PadLayout.Control.A));
        assertTrue(editor.tile(PadLayout.Control.A).performClick());
        assertFalse(editor.draft().spec(PadLayout.Control.A).hidden);
        assertTrue("and comes back without its cluster", editor.draft().spec(PadLayout.Control.FACE).hidden);

        editor.join();
        for (PadLayout.Control member : PadLayout.members(PadLayout.Control.FACE)) {
            assertNull("Joined, its members are not offered", editor.tile(member));
            editor.add(member);
            assertTrue("nor added beside it", editor.draft().spec(member).hidden);
        }
        controller.finishEditing(false);
    }

    @Test public void alignmentLinesUpCentresAndEdgesWithinHalfAGridStep() {
        float[] other = {0, 50, 100};
        assertEquals("Every line 3 px off: one nudge", -3, PadEditor.alignNudge(3, 53, 103, other, 3, 4), 0);
        assertTrue("Too far", Float.isNaN(PadEditor.alignNudge(10, 60, 110, other, 3, 4)));
        assertEquals("An edge meets the other side's", -2, PadEditor.alignNudge(102, 152, 202, other, 3, 4), 0);
        assertEquals("The nearest wins", .5f, PadEditor.alignNudge(-1, 49, 99, new float[] {0, 50, 100, 99.5f, 100, 100.5f}, 6, 4), 0);
        float[] guides = new float[3];
        assertEquals("Edge on edge", 1, PadEditor.alignedLines(100, 150, 200, other, 3, guides));
        assertEquals(100, guides[0], 0);
        assertEquals("A centre never lines up with an edge", 0, PadEditor.alignedLines(25, 50.4f, 75.6f, new float[] {
                10, 20, 50}, 3, guides));
        assertEquals("All three", 3, PadEditor.alignedLines(0, 50, 100, other, 3, guides));
    }

    /** A control dragged near another's centre line lands on it and shows a guide until the finger lifts. */
    @Test public void aDraggedControlLinesUpWithAnotherAndShowsAGuide() {
        ModernController controller = controller(GAME);
        controller.startEditing();
        settle();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        View surface = editor.getChildAt(0);
        float density = frame.getResources().getDisplayMetrics().density;
        float[] left = editor.draft().bounds(PadLayout.Control.LEFT_STICK, surface.getWidth(), surface.getHeight(), density,
                new float[4]);
        float[] right = editor.draft().bounds(PadLayout.Control.RIGHT_STICK, surface.getWidth(), surface.getHeight(), density,
                new float[4]);
        float fromX = right[0] + right[2] / 2, fromY = right[1] + right[3] / 2;
        float toX = fromX - 40, toY = left[1] + left[3] / 2 + 3;
        long now = android.os.SystemClock.uptimeMillis();
        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, fromX, fromY, 0);
        surface.dispatchTouchEvent(down);
        down.recycle();
        MotionEvent move = MotionEvent.obtain(now, now + 16, MotionEvent.ACTION_MOVE, toX, toY, 0);
        surface.dispatchTouchEvent(move);
        move.recycle();
        editor.draft().bounds(PadLayout.Control.RIGHT_STICK, surface.getWidth(), surface.getHeight(), density, right);
        assertEquals("Its centre on the left stick's", left[1] + left[3] / 2, right[1] + right[3] / 2, .5f);
        assertTrue(editor.guides() > 0);
        MotionEvent up = MotionEvent.obtain(now, now + 32, MotionEvent.ACTION_UP, toX, toY, 0);
        surface.dispatchTouchEvent(up);
        up.recycle();
        assertEquals("Let go, the guides go", 0, editor.guides());
        controller.finishEditing(false);
    }

    /**
     * On a phone held sideways the panel stays whole on the surface, its Done in reach, in its tallest states: many
     * controls left out with the Steam Controller's on, and the pointer touchpad picked with its mode cards; the tiles
     * and cards take a full touch.
     */
    @Test public void thePanelStaysOnTheSurfaceInItsTallestStates() {
        prefs.oscSteamControls = true;
        ModernController controller = controller(GAME);
        controller.startEditing();
        settle();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        for (PadLayout.Control control : new PadLayout.Control[] {PadLayout.Control.L3, PadLayout.Control.R3,
                PadLayout.Control.HOME, PadLayout.Control.SELECT, PadLayout.Control.START, PadLayout.Control.LB,
                PadLayout.Control.RB, PadLayout.Control.LEFT_PAD, PadLayout.Control.RIGHT_PAD, PadLayout.Control.L4,
                PadLayout.Control.R4, PadLayout.Control.LEFT_GRIP}) {
            editor.select(control);
            editor.toggleHidden();
        }
        editor.select(null);
        settle();
        float density = frame.getResources().getDisplayMetrics().density;
        assertWhole(editor, "many left out");
        for (PadLayout.Control control : new PadLayout.Control[] {PadLayout.Control.L3, PadLayout.Control.LEFT_PAD,
                PadLayout.Control.WASD}) {
            View tile = editor.tile(control);
            assertTrue(control + " tile is " + tile.getWidth() / density + "x" + tile.getHeight() / density + " dp",
                    tile.getWidth() >= 48 * density - 1 && tile.getHeight() >= 48 * density - 1);
        }
        editor.addTouchpad();
        settle();
        assertWhole(editor, "touchpad picked");
        for (int id : new int[] {com.limelight.R.id.pad_editor_mode_mouse, com.limelight.R.id.pad_editor_mode_camera}) {
            View card = editor.findViewById(id);
            assertTrue(card.getHeight() >= 48 * density - 1 && card.getWidth() >= 48 * density - 1);
        }
        controller.finishEditing(false);
    }

    private static void assertWhole(PadEditor editor, String state) {
        View panel = editor.panel();
        assertTrue(state + ": the panel lies on the surface", panel.getX() >= 0 && panel.getY() >= 0
                && panel.getX() + panel.getWidth() <= editor.getWidth() && panel.getY() + panel.getHeight() <= editor.getHeight());
        View done = editor.findViewById(com.limelight.R.id.pad_editor_done);
        float y = done.getY();
        for (View parent = (View) done.getParent(); parent != panel; parent = (View) parent.getParent()) y += parent.getY();
        assertTrue(state + ": Done is whole in the panel", done.getHeight() > 0 && y >= 0
                && y + done.getHeight() <= panel.getHeight());
    }
}
