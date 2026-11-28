package com.limelight.binding.input.pad;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.graphics.drawable.LayerDrawable;
import android.view.MotionEvent;
import android.view.View;
import android.widget.TextView;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

/**
 * The layout editor's dock: it is its bar alone until a control is picked or a tray is opened, every tool on the bar
 * is named, focusable and a full touch, the Add and More trays hold what the old panel showed with nothing picked,
 * the mirror lock and Undo are one tap away, and the dock steps back while a control is moved.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi")
public class PadStudioInspectorTest extends ModernControllerHarness {
    private static final int[] TOOLS = {R.id.pad_studio_add, R.id.pad_editor_preset, R.id.pad_studio_mirror,
            R.id.pad_studio_undo, R.id.pad_studio_more, R.id.pad_editor_reset, R.id.pad_editor_cancel,
            R.id.pad_editor_done};
    private ModernController controller;
    private PadEditor editor;
    private float density;

    private void open() {
        controller = controller(GAME);
        controller.startEditing();
        settle();
        editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        density = frame.getResources().getDisplayMetrics().density;
    }

    private MaterialButton tool(int id) {
        return editor.findViewById(id);
    }

    private View view(int id) {
        return editor.findViewById(id);
    }

    /** A view that is shown, with every view it stands in. */
    private boolean shows(int id) {
        for (View view = view(id); view != editor; view = (View) view.getParent()) {
            if (view.getVisibility() != View.VISIBLE) return false;
        }
        return true;
    }

    @Test public void theDockIsItsBarUntilThereIsSomethingToShow() {
        open();
        View dock = editor.panel();
        assertFalse("No tray at first", shows(R.id.pad_editor_scroll));
        int bar = dock.getHeight();
        assertTrue("A bar: " + bar / density + " dp high", bar <= 72 * density);
        assertTrue("Baked: the fill and its rim on black, no glass and nothing reading through",
                dock.getBackground() instanceof LayerDrawable);
        assertEquals("whole until a control is moved", 1f, dock.getAlpha(), 0);
        assertEquals("and no shadow over the stream", 0, dock.getElevation(), 0);
        assertEquals(View.LAYER_TYPE_NONE, dock.getLayerType());

        // A picked control shows in the tray, with its size, opacity and Hide.
        editor.select(PadLayout.Control.RB);
        settle();
        assertTrue(shows(R.id.pad_editor_control));
        assertTrue(shows(R.id.pad_editor_opacity));
        assertTrue(shows(R.id.pad_editor_hide));
        assertFalse(shows(R.id.pad_editor_add));
        assertFalse(shows(R.id.pad_editor_this_game));
        assertTrue(dock.getHeight() > bar);
        assertEquals("RB", ((TextView) view(R.id.pad_editor_selection)).getText().toString());
        // The opacity slider is the app's: a thin track, a round thumb, and its value at the row's end.
        com.google.android.material.slider.Slider opacity = (com.google.android.material.slider.Slider) view(R.id.pad_editor_opacity);
        android.content.res.Resources res = editor.getResources();
        assertEquals(res.getDimensionPixelSize(R.dimen.nj_size_slider_track), opacity.getTrackHeight());
        assertEquals(res.getDimensionPixelSize(R.dimen.nj_size_slider_thumb) / 2, opacity.getThumbRadius());
        assertTrue("A ring around the thumb for a controller's focus", opacity.getHaloRadius() > opacity.getThumbRadius());
        TextView shown = (TextView) view(R.id.pad_studio_opacity_value);
        assertTrue(shows(R.id.pad_studio_opacity_value));
        assertEquals("100 %", shown.getText().toString());
        opacity.setValue(60);
        assertEquals("60 %", shown.getText().toString());
        opacity.setValue(100);
        // Let go, the dock is its bar again.
        editor.select(null);
        settle();
        assertFalse(shows(R.id.pad_editor_scroll));
        assertEquals(bar, dock.getHeight());
        controller.finishEditing(false);
    }

    @Test public void addOpensTheTrayOfControlsTheLayoutLeavesOut() {
        open();
        MaterialButton add = tool(R.id.pad_studio_add);
        assertTrue("The WASD stick and the pointer touchpad are left out", add.isEnabled());
        assertFalse(add.isChecked());
        assertTrue(add.performClick());
        settle();
        assertTrue(add.isChecked());
        assertTrue(shows(R.id.pad_editor_add));
        assertFalse("Only the tiles", shows(R.id.pad_editor_opacity));
        View tile = editor.tile(PadLayout.Control.WASD);
        assertNotNull(tile);
        assertTrue(tile.getWidth() >= 48 * density && tile.getHeight() >= 48 * density);
        // A tile puts its control in and picks it; the tray shows that control now.
        assertTrue(tile.performClick());
        settle();
        assertEquals(PadLayout.Control.WASD, editor.selected());
        assertFalse(add.isChecked());
        assertFalse(shows(R.id.pad_editor_add));
        assertTrue(shows(R.id.pad_editor_control));
        // Add with a control picked lets it go and opens the tray; a second tap closes it.
        assertTrue(add.performClick());
        settle();
        assertNull(editor.selected());
        assertTrue(shows(R.id.pad_editor_add));
        assertTrue(add.performClick());
        settle();
        assertFalse(shows(R.id.pad_editor_scroll));
        // Repeatable Touch zone and KBM button tiles stay addable after built-in controls are placed,
        // until both reach capacity.
        editor.addTouchpad();
        settle();
        assertTrue(add.isEnabled());
        PadLayout draft = editor.draft();
        while (draft.touchZones().size() < PadTouchZone.MAX_ZONES) {
            editor.addTouchZone();
        }
        while (draft.customButtons().size() < PadCustomButton.MAX_CUSTOM_BUTTONS) {
            editor.addCustomButton();
        }
        editor.select(null);
        settle();
        assertFalse(add.isEnabled());
        controller.finishEditing(false);
    }

    @Test public void moreHoldsEveryControlsSizeAndOpacityTheGlyphsAndTheGame() {
        open();
        MaterialButton more = tool(R.id.pad_studio_more);
        assertTrue(more.performClick());
        settle();
        assertTrue(more.isChecked());
        assertEquals(frame.getContext().getString(R.string.pad_edit_all),
                ((TextView) view(R.id.pad_editor_selection)).getText().toString());
        for (int id : new int[] {R.id.pad_editor_smaller, R.id.pad_editor_larger, R.id.pad_editor_opacity,
                R.id.pad_editor_glyphs, R.id.pad_editor_this_game}) {
            assertTrue(frame.getResources().getResourceEntryName(id), shows(id));
        }
        assertFalse(shows(R.id.pad_editor_add));
        assertFalse(shows(R.id.pad_editor_control));
        assertTrue(view(R.id.pad_editor_larger).performClick());
        assertEquals("Every control", 1.1f, editor.draft().spec(PadLayout.Control.LEFT_STICK).scale(), 1e-3);
        assertEquals(1.1f, editor.draft().spec(PadLayout.Control.RT).scale(), 1e-3);
        View dock = editor.panel();
        assertTrue("Whole on the surface", dock.getY() >= 0 && dock.getY() + dock.getHeight() <= editor.getHeight());
        // Picking a control closes the tray for good: let go, the bar is alone.
        editor.select(PadLayout.Control.LB);
        editor.select(null);
        settle();
        assertFalse(more.isChecked());
        assertFalse(shows(R.id.pad_editor_scroll));
        controller.finishEditing(false);
    }

    @Test public void theMirrorLockAndUndoAreOnTheBar() {
        open();
        MaterialButton mirror = tool(R.id.pad_studio_mirror), undo = tool(R.id.pad_studio_undo);
        assertFalse("Off when the editor opens", mirror.isChecked());
        assertFalse("Nothing to take back yet", undo.isEnabled());
        assertTrue(mirror.performClick());
        assertTrue(editor.mirror());
        assertTrue(mirror.isChecked());
        // LB grows, and RB with it.
        editor.select(PadLayout.Control.LB);
        assertTrue(view(R.id.pad_editor_larger).performClick());
        assertEquals(1.1f, editor.draft().spec(PadLayout.Control.RB).scale(), 1e-3);
        assertTrue(undo.isEnabled());
        assertTrue(undo.performClick());
        assertEquals(1, editor.draft().spec(PadLayout.Control.LB).scale(), 1e-3);
        assertEquals(1, editor.draft().spec(PadLayout.Control.RB).scale(), 1e-3);
        assertFalse(undo.isEnabled());
        assertTrue(mirror.performClick());
        assertFalse(editor.mirror());
        // Reset is a change like any other.
        assertTrue(view(R.id.pad_editor_larger).performClick());
        assertTrue(tool(R.id.pad_editor_reset).performClick());
        assertTrue(editor.draft().isDefault());
        assertTrue(undo.performClick());
        assertEquals(1.1f, editor.draft().spec(PadLayout.Control.LB).scale(), 1e-3);
        controller.finishEditing(false);
    }

    @Test public void everyToolIsNamedFocusableAndAFullTouch() {
        open();
        View bar = view(R.id.pad_studio_bar);
        for (int id : TOOLS) {
            MaterialButton tool = tool(id);
            String name = frame.getResources().getResourceEntryName(id);
            assertTrue(name + " is shown", shows(id));
            assertTrue(name + " is " + tool.getWidth() / density + "x" + tool.getHeight() / density + " dp",
                    tool.getWidth() >= 48 * density - 1 && tool.getHeight() >= 48 * density - 1);
            assertTrue(name + " takes the focus of a controller", tool.isFocusable());
            CharSequence said = tool.getText().length() > 0 ? tool.getText() : tool.getContentDescription();
            assertTrue(name + " has a name", said != null && said.length() > 0);
        }
        assertEquals("One row on a phone held sideways", view(R.id.pad_studio_tools).getTop(),
                view(R.id.pad_editor_actions).getTop());
        View done = view(R.id.pad_editor_done);
        assertTrue("Done ends inside the bar", ((View) done.getParent()).getLeft() + done.getRight() <= bar.getWidth());
        assertEquals(frame.getContext().getString(R.string.pad_edit_move_panel),
                view(R.id.pad_editor_grip).getContentDescription());
        controller.finishEditing(false);
    }

    /** Wherever the dock stands, a control that is moved shows through it with its guides, until the finger lifts. */
    @Test public void theDockStepsBackWhileAControlIsMoved() {
        open();
        View surface = editor.getChildAt(0), dock = editor.panel();
        float[] box = editor.draft().bounds(PadLayout.Control.RB, surface.getWidth(), surface.getHeight(), density,
                new float[4]);
        float x = box[0] + box[2] / 2, y = box[1] + box[3] / 2;
        touch(surface, MotionEvent.ACTION_DOWN, x, y);
        assertEquals("Picking a control leaves the dock be", 1, dock.getAlpha(), 0);
        touch(surface, MotionEvent.ACTION_MOVE, x - 80, y + 60);
        assertEquals(PadEditorPanel.STEPPED_BACK, dock.getAlpha(), 0);
        touch(surface, MotionEvent.ACTION_MOVE, x - 120, y + 90);
        assertEquals(PadEditorPanel.STEPPED_BACK, dock.getAlpha(), 0);
        touch(surface, MotionEvent.ACTION_UP, x - 120, y + 90);
        assertEquals("Let go, it is back", 1, dock.getAlpha(), 0);
        assertTrue("and the move is Undo's to take back", tool(R.id.pad_studio_undo).isEnabled());
        controller.finishEditing(false);
    }

    private static void touch(View view, int action, float x, float y) {
        long now = android.os.SystemClock.uptimeMillis();
        MotionEvent event = MotionEvent.obtain(now, now, action, x, y, 0);
        view.dispatchTouchEvent(event);
        event.recycle();
    }

    /** A phone held upright: the tools stand above the actions, and both lie whole in the dock. */
    @Test @Config(qualifiers = "w360dp-h800dp-port-xhdpi")
    public void aNarrowSurfaceStacksTheToolsAboveTheActions() {
        frame.getLayoutParams().width = 720;
        frame.getLayoutParams().height = 1600;
        controller = new ModernController(frame.getContext(), frame, sink, prefs, GAME);
        controller.refreshLayout();
        controller.startEditing();
        for (int pass = 0; pass < 3; pass++) {
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            frame.measure(View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY));
            frame.layout(0, 0, 720, 1600);
        }
        editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        density = frame.getResources().getDisplayMetrics().density;
        View dock = editor.panel(), tools = view(R.id.pad_studio_tools), actions = view(R.id.pad_editor_actions);
        assertTrue("The dock leaves the surface a margin", dock.getWidth() > 0 && dock.getWidth() <= 720 - 2 * 16 * density);
        assertTrue("The tools above the actions", tools.getBottom() <= actions.getTop());
        View more = view(R.id.pad_studio_more), done = view(R.id.pad_editor_done);
        assertTrue("The last tool lies whole in its row", more.getRight() <= tools.getWidth() && more.getWidth() >= 48 * density - 1);
        assertTrue(done.getRight() <= actions.getWidth() && done.getWidth() >= 48 * density - 1);
        controller.destroy();
    }
}
