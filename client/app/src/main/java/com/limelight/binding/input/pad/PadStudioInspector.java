package com.limelight.binding.input.pad;

import android.view.View;
import android.widget.LinearLayout;

import androidx.appcompat.widget.TooltipCompat;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;

/**
 * The Touch Controller Studio's floating micro-toolbar: the bar of {@link PadEditor}'s dock. Beside the grip it holds
 * the layout's tools (Add, which opens the tray of the controls the layout leaves out; Preset; the mirror lock; Undo;
 * More, which opens the tray with every control's size and opacity, the glyphs and where Done saves) and Reset, Cancel
 * and Done. Picking a control closes the trays and shows that control in their place, so the dock is only its bar
 * until there is something to show.
 *
 * <p>Every tool is an icon with its name as description and tooltip, takes a full 48 dp touch and the focus of a
 * controller or keyboard; one that is on is lit amber like a pressed control, one with nothing to do is faint. On a
 * surface too narrow for one row the tools stand above the actions. {@link PadEditorPanel} places the dock, moves it
 * by the grip and fills the tray.
 */
final class PadStudioInspector {
    /** What the tray under the bar shows while no control is picked: nothing, the Add tiles, the layout's other choices. */
    static final int NONE = 0, ADD = 1, MORE = 2;
    /**
     * In dp, and not in the tokens: the dock's width with tools and actions on one row and on two, and the room it
     * leaves to each edge of the surface.
     */
    static final float ROW_WIDTH_DP = 496, STACKED_WIDTH_DP = 400, MARGIN_DP = 16;

    private final PadEditor editor;
    private final PadEditorPanel panel;
    private final LinearLayout bar;
    private final View tools, actions;
    private final MaterialButton add, mirror, undo, more;
    private int tray = NONE;

    PadStudioInspector(View dock, PadEditor editor, PadEditorPanel panel) {
        this.editor = editor;
        this.panel = panel;
        bar = dock.findViewById(R.id.pad_studio_bar);
        tools = dock.findViewById(R.id.pad_studio_tools);
        actions = dock.findViewById(R.id.pad_editor_actions);
        add = tool(dock, R.id.pad_studio_add);
        mirror = tool(dock, R.id.pad_studio_mirror);
        undo = tool(dock, R.id.pad_studio_undo);
        more = tool(dock, R.id.pad_studio_more);
        add.setOnClickListener(v -> show(tray == ADD ? NONE : ADD));
        more.setOnClickListener(v -> show(tray == MORE ? NONE : MORE));
        // A checkable button has turned itself by the time its click is heard.
        mirror.setOnClickListener(v -> editor.setMirror(mirror.isChecked()));
        undo.setOnClickListener(v -> editor.undo());
        tool(dock, R.id.pad_editor_preset).setOnClickListener(v -> editor.choosePreset());
        tool(dock, R.id.pad_editor_reset).setOnClickListener(v -> editor.resetDraft());
        tool(dock, R.id.pad_editor_cancel).setOnClickListener(v -> editor.finishEditing(false));
        dock.findViewById(R.id.pad_editor_done).setOnClickListener(v -> editor.finishEditing(true));
    }

    /** An icon tool: its description is its tooltip too, for a long press and a pointer that hovers. */
    private static MaterialButton tool(View dock, int id) {
        MaterialButton button = dock.findViewById(id);
        TooltipCompat.setTooltipText(button, button.getContentDescription());
        return button;
    }

    /** The tray that shows while no control is picked. */
    int tray() {
        return tray;
    }

    /** Opens the {@code which} tray, or closes them with {@link #NONE}. The trays are the layout's: a picked control is let go. */
    void show(int which) {
        tray = which;
        if (which != NONE && editor.hasSelection()) {
            // Letting go changes the dock, smoothly, by itself.
            editor.select(null);
        } else {
            panel.transition();
            panel.update();
        }
    }

    /** A control is picked: it shows in the trays' place, and letting it go leaves the bar alone. */
    void picked() {
        tray = NONE;
    }

    /** The tools' states: which is on and which has nothing to do. {@code addable} says the layout leaves a control out. */
    void update(boolean addable) {
        if (!addable && tray == ADD) tray = NONE;
        add.setEnabled(addable);
        add.setChecked(tray == ADD);
        more.setChecked(tray == MORE);
        mirror.setChecked(editor.mirror());
        undo.setEnabled(editor.canUndo());
    }

    /** Whether tools and actions share one row on a surface {@code surfaceWidth} pixels wide. */
    static boolean oneRow(int surfaceWidth, float density) {
        return surfaceWidth - 2 * MARGIN_DP * density >= ROW_WIDTH_DP * density;
    }

    /** The dock's width on that surface, in pixels: never wider than the surface leaves room for. */
    static int width(int surfaceWidth, float density) {
        float most = surfaceWidth - 2 * MARGIN_DP * density;
        return Math.max(0, Math.round(Math.min((oneRow(surfaceWidth, density) ? ROW_WIDTH_DP : STACKED_WIDTH_DP) * density, most)));
    }

    /** Arranges the bar: tools and actions on one row, or the tools above the actions. */
    void arrange(boolean row) {
        bar.setOrientation(row ? LinearLayout.HORIZONTAL : LinearLayout.VERTICAL);
        LinearLayout.LayoutParams above = (LinearLayout.LayoutParams) tools.getLayoutParams();
        above.width = row ? LinearLayout.LayoutParams.WRAP_CONTENT : LinearLayout.LayoutParams.MATCH_PARENT;
        tools.setLayoutParams(above);
        LinearLayout.LayoutParams below = (LinearLayout.LayoutParams) actions.getLayoutParams();
        below.width = row ? 0 : LinearLayout.LayoutParams.MATCH_PARENT;
        below.weight = row ? 1 : 0;
        actions.setLayoutParams(below);
    }
}
