package com.limelight.binding.input.pad;

import android.annotation.SuppressLint;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.text.TextPaint;
import android.text.TextUtils;
import android.util.TypedValue;
import android.view.View;
import android.widget.FrameLayout;

import androidx.core.content.ContextCompat;

import com.limelight.R;
import com.limelight.ui.NavigationDialogBuilder;

/**
 * Edits the modern controller's layout over a dimmed stream: drag a control to move it, pinch it to resize it, drag a
 * picked control's round corner handle to size its width and height on their own up to the whole surface, tap it to
 * pick it. A dragged control lines up with another's centre or edge when it comes near, and a guide shows the line.
 * The glass panel shows the picked control (its size, opacity, Split or Join, Hide, and the pointer touchpad's Mouse
 * and Camera) or, with nothing picked, every control's size and opacity, the Add tiles of the controls the layout leaves
 * out, the preset and glyphs and whether Done saves for this game only; it changes between them smoothly. Its grip
 * moves it anywhere, and it reopens where it was left. Done keeps the draft; Cancel and Back drop it. A hidden control
 * leaves the surface for the Add tiles.
 */
@SuppressLint("ViewConstructor") // Created by ModernController.
final class PadEditor extends FrameLayout {
    /** Moved controls land on this grid, so rows and mirrored pairs line up without effort. */
    static final float GRID_DP = 8;
    /** The bits of a corner handle: 0 is the top-left one. */
    static final int RIGHT_CORNER = 1, BOTTOM_CORNER = 2;
    static final float SCALE_STEP = .1f;
    /** An Add tile's size, the gap between tiles and between groups of them, in dp. */
    static final float TILE_WIDTH_DP = 84, TILE_HEIGHT_DP = 68, TILE_GAP_DP = 8, GROUP_GAP_DP = 20;
    /** The Add groups, in the order the panel shows them. */
    static final int GAMEPAD = 0, KEYBOARD_MOUSE = 1, EXTRAS = 2;
    /** Whether the editor animates; tests and previews turn it off to see each state as it settles. */
    static boolean transitions = true;

    private final ModernController controller;
    private final PadPainter painter;
    private final PadLayout draft;
    private final boolean gameAvailable;
    private boolean perGame;
    private PadLayout.Control selected;
    private final PadEditorSurface surface;
    private final PadEditorPanel panel;
    private final float density;

    PadEditor(Context context, ModernController controller, PadPainter painter, PadLayout draft,
              boolean perGame, boolean gameAvailable) {
        // The stream's dark surfaces whatever theme the activity has, like the stream menu.
        super(new androidx.appcompat.view.ContextThemeWrapper(context, R.style.StreamTheme));
        context = getContext();
        this.controller = controller;
        this.painter = painter;
        this.draft = draft;
        this.perGame = perGame && gameAvailable;
        this.gameAvailable = gameAvailable;
        density = context.getResources().getDisplayMetrics().density;
        surface = new PadEditorSurface(context, this, painter, draft, density);
        addView(surface, new LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT));
        panel = new PadEditorPanel(context, this, surface, density, this.perGame, gameAvailable);
        setClickable(true);
        panel.update();
        panel.animateEntrance();
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        panel.onEditorSizeChanged(w, h, oldw, oldh);
    }

    /** The panel, for tests. */
    View panel() {
        return panel.view();
    }

    /** What the surface draws a control with at rest, for tests. */
    PadPainter.State previewState(PadLayout.Control control) {
        return surface.previewState(control);
    }

    /** The Add tile for {@code control}, or null when the layout shows it; for tests. */
    View tile(PadLayout.Control control) {
        return panel.tile(control);
    }

    /** How many alignment guides the surface shows now, for tests. */
    int guides() {
        return surface.guides();
    }

    PadLayout draft() {
        return draft;
    }

    boolean perGame() {
        return perGame && gameAvailable;
    }

    void setPerGame(boolean checked) {
        perGame = checked;
    }

    PadLayout.Control selected() {
        return selected;
    }

    float baseOpacity() {
        return controller.prefs().oscOpacity / 100f;
    }

    int[] panelPosition() {
        return controller.panelPosition();
    }

    void savePanelPosition(int left, int top) {
        controller.savePanelPosition(left, top);
    }

    void finishEditing(boolean save) {
        controller.finishEditing(save);
    }

    void resetDraft() {
        draft.reset();
        painter.setGlyph(draft.glyph());
        painter.setFacesSwapped(draft.facesSwapped());
        select(null);
        changed();
    }

    void select(PadLayout.Control control) {
        if (control != selected) panel.transition();
        selected = control;
        panel.update();
        surface.invalidate();
        post(panel::keepClear);
    }

    /** The controls the editor shows: everything but what the settings keep off. */
    private boolean editable(PadLayout.Control control) {
        return !controller.forcedHidden(control);
    }

    /**
     * Whether the surface draws the control and a tap can pick it: a shown control the settings allow. A cluster and
     * its members never show together: a member is off the surface while its cluster shows.
     */
    boolean onSurface(PadLayout.Control control) {
        if (!editable(control) || draft.spec(control).hidden) return false;
        PadLayout.Control cluster = control.cluster();
        return cluster == null || draft.spec(cluster).hidden;
    }

    /**
     * Whether the Add tiles offer the control: the layout leaves it out and the settings allow it. A split cluster is
     * not offered, its members are; a member only while its cluster is split, so adding never shows a cluster with
     * its members.
     */
    boolean addable(PadLayout.Control control) {
        if (!editable(control) || !draft.spec(control).hidden) return false;
        PadLayout.Control cluster = control.cluster();
        if (cluster != null) return draft.isSplit(cluster);
        return !draft.isSplit(control);
    }

    /** The Add group a control is offered in: the PC's gamepad, keyboard and mouse, or the extras beyond a gamepad. */
    static int addGroup(PadLayout.Control control) {
        if (control == PadLayout.Control.WASD || control == PadLayout.Control.POINTER_PAD) return KEYBOARD_MOUSE;
        if (control.steam || control == PadLayout.Control.TOUCHPAD) return EXTRAS;
        return GAMEPAD;
    }

    /**
     * The plus and minus buttons: the picked control, or every control, grows or shrinks by a step in both directions,
     * keeping its shape, up to the whole surface.
     */
    void scaleBy(float step) {
        float width = surfaceWidth(), height = surfaceHeight();
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (!editable(control) || (selected != null && control != selected)) continue;
            PadLayout.Spec spec = draft.spec(control);
            // Round the larger side to the step so repeated taps land on 50 %, 60 %, ... rather than drifting.
            float from = spec.scale(), to = Math.round((from + step) / SCALE_STEP) * SCALE_STEP;
            draft.scaleFrom(control, spec.widthScale, spec.heightScale, to / from, width, height, density);
        }
        changed();
    }

    /** The surface's size: the editor's own once laid out, before that the stream frame it fills. */
    private float surfaceWidth() {
        return surface.getWidth() > 0 ? surface.getWidth() : controller.frame().getWidth();
    }

    private float surfaceHeight() {
        return surface.getHeight() > 0 ? surface.getHeight() : controller.frame().getHeight();
    }

    void setOpacity(int value) {
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (!editable(control) || (selected != null && control != selected)) continue;
            draft.setOpacity(control, value);
        }
        surface.invalidate();
    }

    /** Hides the picked control, which leaves the surface for the Add tiles but stays picked, so Show undoes it. */
    void toggleHidden() {
        if (selected == null) return;
        PadLayout.Spec spec = draft.spec(selected);
        spec.hidden = !spec.hidden;
        changed();
    }

    /**
     * Splits the picked face cluster or D-pad into four controls of their own, where its buttons were and at their
     * sizes, and picks the first, so Join is at hand.
     */
    void split() {
        if (selected == null || PadLayout.members(selected).length == 0) return;
        PadLayout.Control cluster = selected;
        draft.split(cluster, surfaceWidth(), surfaceHeight(), density);
        select(PadLayout.members(cluster)[0]);
        changed();
    }

    /** Joins the picked member's cluster again where its members stand, in their arrangement, and picks it. */
    void join() {
        if (selected == null || selected.cluster() == null) return;
        PadLayout.Control cluster = selected.cluster();
        draft.join(cluster, surfaceWidth(), surfaceHeight(), density);
        painter.setFacesSwapped(draft.facesSwapped());
        select(cluster);
        changed();
    }

    /** Puts a control the layout leaves out back in, where it was, and picks it, so it can be placed at once. */
    void add(PadLayout.Control control) {
        if (!addable(control)) return;
        if (control == PadLayout.Control.FACE) {
            // Its members were hidden: it comes back in the arrangement they stood in.
            draft.takeFaceArrangement();
            painter.setFacesSwapped(draft.facesSwapped());
        }
        draft.spec(control).hidden = false;
        select(control);
        changed();
    }

    /** Puts the WASD stick in the layout and picks it. */
    void addWasd() {
        add(PadLayout.Control.WASD);
    }

    /** Puts the pointer touchpad in the layout and picks it, ready to move, resize and set its mode. */
    void addTouchpad() {
        add(PadLayout.Control.POINTER_PAD);
    }

    /** A preset replaces the layout with its selection of controls; Done keeps it, Cancel drops it. */
    void choosePreset() {
        new NavigationDialogBuilder(getContext()).setTitle(R.string.pad_edit_preset)
                .setItems(R.array.pad_preset_names, (dialog, which) -> {
                    draft.applyPreset(PadLayout.Preset.values()[which]);
                    painter.setGlyph(draft.glyph());
                    painter.setFacesSwapped(draft.facesSwapped());
                    select(null);
                    changed();
                }).setNegativeButton(android.R.string.cancel, null).create().show();
    }

    /** The face letters and shoulder names of one controller family. */
    void chooseGlyphs() {
        new NavigationDialogBuilder(getContext()).setTitle(R.string.pad_edit_glyphs)
                .setItems(R.array.pad_glyph_names, (dialog, which) -> {
                    draft.setGlyph(PadLayout.Glyph.values()[which]);
                    painter.setGlyph(draft.glyph());
                    painter.setFacesSwapped(draft.facesSwapped());
                    changed();
                }).setNegativeButton(android.R.string.cancel, null).create().show();
    }

    /** The pointer touchpad's mode, from its Mouse and Camera cards. */
    void setPointerMode(PadLayout.PointerMode mode) {
        if (draft.pointerMode() == mode) return;
        draft.setPointerMode(mode);
        changed();
    }

    /** Switches the pointer touchpad between moving the pointer and moving the camera. */
    void togglePointerMode() {
        setPointerMode(draft.pointerMode() == PadLayout.PointerMode.MOUSE ?
                PadLayout.PointerMode.CAMERA : PadLayout.PointerMode.MOUSE);
    }

    void changed() {
        panel.update();
        surface.invalidate();
    }

    void updatePanel() {
        panel.update();
    }

    void keepPanelClear() {
        panel.keepClear();
    }

    PadLayout.Spec reference() {
        if (selected != null) return draft.spec(selected);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (editable(control)) return draft.spec(control);
        }
        return draft.spec(PadLayout.Control.L3);
    }

    /** "120 %", or "150 × 80 %" for a control whose width and height differ. */
    static String sizeLabel(PadLayout.Spec spec) {
        return PadEditorPanel.sizeLabel(spec);
    }

    /** The control's name for people, with Xbox's face letters. */
    static String name(Context context, PadLayout.Control control) {
        return name(context, control, PadLayout.Glyph.XBOX);
    }

    /** The control's name for people: its label, or what it is when it has none; a face button's in its glyph set. */
    static String name(Context context, PadLayout.Control control, PadLayout.Glyph glyph) {
        return PadEditorPanel.name(context, control, glyph);
    }

    /**
     * The nudge that lines a dragged box's start, centre or end up with another control's when one is within
     * {@code tolerance}, the nearest winning; NaN when none is near.
     */
    static float alignNudge(float start, float centre, float end, float[] lines, int count, float tolerance) {
        return PadEditorSurface.alignNudge(start, centre, end, lines, count, tolerance);
    }

    /**
     * Which of the other controls' {@code lines} (as {@link #alignNudge} takes them) the box's start, centre or end lies
     * on, to half a pixel: edges on edges, centres on centres.
     */
    static int alignedLines(float start, float centre, float end, float[] lines, int count, float[] out) {
        return PadEditorSurface.alignedLines(start, centre, end, lines, count, out);
    }

    Tile newTile(PadLayout.Control control) {
        return new Tile(getContext(), control);
    }

    /** One control the layout leaves out, drawn small as it plays, with its name under it; a tap puts it in. */
    @SuppressLint("ViewConstructor")
    final class Tile extends View {
        final PadLayout.Control control;
        private final TextPaint caption = new TextPaint(Paint.ANTI_ALIAS_FLAG);
        private final float captionSize;
        private String name = "";

        Tile(Context context, PadLayout.Control control) {
            super(context);
            this.control = control;
            setBackgroundResource(R.drawable.bg_pad_editor_tile);
            setFocusable(true);
            setClickable(true);
            setOnClickListener(v -> add(control));
            caption.setTextAlign(Paint.Align.CENTER);
            captionSize = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, 11, getResources().getDisplayMetrics());
            caption.setTextSize(captionSize);
            caption.setColor(ContextCompat.getColor(context, R.color.night_on_surface));
            refresh();
        }

        /** Its name follows the glyph set (a face button's letter), its drawing that and the touchpad's mode. */
        void refresh() {
            name = name(getContext(), control, draft.glyph());
            setContentDescription(getContext().getString(R.string.pad_edit_add_control, name));
            invalidate();
        }

        @Override
        protected void onMeasure(int widthSpec, int heightSpec) {
            setMeasuredDimension(Math.round(TILE_WIDTH_DP * density), Math.round(TILE_HEIGHT_DP * density));
        }

        @Override
        protected void onDraw(Canvas canvas) {
            float pad = 6 * density, width = getWidth(), captionHeight = captionSize + 4 * density;
            // The control at its default shape, as large as the tile's picture area allows.
            float most = width - 2 * pad, tall = getHeight() - captionHeight - 2 * pad;
            float ratio = PadLayout.baseWidth(control) / PadLayout.baseHeight(control);
            float w = most, h = most / ratio;
            if (h > tall) {
                h = tall;
                w = tall * ratio;
            }
            painter.draw(canvas, control, (width - w) / 2, pad + (tall - h) / 2, w, h, surface.previewState(control));
            // A long name shrinks to fit, down to 9 sp, before it is cut short.
            float room = width - pad, size = captionSize;
            caption.setTextSize(size);
            float wide = caption.measureText(name);
            if (wide > room) caption.setTextSize(Math.max(captionSize * 9 / 11, size * room / wide));
            CharSequence text = TextUtils.ellipsize(name, caption, room, TextUtils.TruncateAt.END);
            canvas.drawText(text, 0, text.length(), width / 2, getHeight() - pad - caption.descent(), caption);
        }
    }
}
