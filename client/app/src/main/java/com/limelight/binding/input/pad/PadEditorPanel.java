package com.limelight.binding.input.pad;

import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.RectF;
import android.transition.ChangeBounds;
import android.transition.Fade;
import android.transition.Transition;
import android.transition.TransitionManager;
import android.transition.TransitionSet;
import android.view.Gravity;
import android.view.LayoutInflater;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.view.animation.Interpolator;
import android.view.animation.PathInterpolator;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.core.content.ContextCompat;
import androidx.core.graphics.ColorUtils;
import androidx.core.widget.TextViewCompat;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.chip.Chip;
import com.google.android.material.materialswitch.MaterialSwitch;
import com.google.android.material.slider.Slider;
import com.limelight.R;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

/**
 * Manages {@link PadEditor}'s floating dock under its bar ({@link PadStudioInspector}): where it stands on tall and
 * short surfaces, its grip drag and per-orientation saved placement, how it steps back while a control is moved, the
 * tray with the picked control or one of the layout's trays, smooth transitions between them, the grouped Add tiles
 * and control/glyph labels.
 */
final class PadEditorPanel {
    /** A surface lower than this (a phone held sideways) gets the tray without the hint and its options in one row. */
    private static final float SHORT_SURFACE_DP = 480;
    /** How long the panel takes to come in and to change between the picked control and the layout. */
    private static final long PANEL_MS = 200;
    private static final Interpolator EASE = new PathInterpolator(.2f, 0, 0, 1);
    /** How much of the dock shows while a finger moves or sizes a control: that control and its guides show through. */
    static final float STEPPED_BACK = .3f;

    private final Context context;
    private final PadEditor editor;
    private final View surface;
    private final float density;
    private final PadStudioInspector inspector;
    private PadCustomInspector customInspector;
    private final View panel, tray, sizeRow, opacityRow, controlSection, pointerMode, mouseCard, cameraCard, thisGame;
    private final MaterialSwitch thisController;
    /** The Touch area row (#172): its slider in percent of the box, and the value at its end. */
    private final View reachRow;
    private final Slider reach;
    private final TextView reachText;
    private boolean settingReach;
    private final LinearLayout layoutSection, options, addSection;
    private final TextView selection, sizeText, opacityText, hint;
    private final MaterialButton hide, split, join;
    private final Chip glyphs;
    private final Slider opacity;
    private boolean oneRow = true, shortSurface, settingOpacity, steppedBack;
    /** The controls the Add tiles show, so they are built again only when that changes. */
    private String addShown = "";
    private final List<PadEditor.Tile> tiles = new ArrayList<>();
    /**
     * Whether the player has placed the panel with its grip, in this orientation now or before. Then it stays where
     * it was put, its top-left corner at {@link #panelX}, {@link #panelY}, shown kept on the surface, and no longer
     * moves out of the picked control's way by itself.
     */
    private boolean panelPlaced;
    private float panelX, panelY, gripRawX, gripRawY, gripFromX, gripFromY;
    /** Once a change of the panel has played, it moves out of the picked control's way with its final size. */
    private final Transition.TransitionListener settled = new Transition.TransitionListener() {
        @Override public void onTransitionEnd(Transition transition) {
            keepClear();
        }

        @Override public void onTransitionStart(Transition transition) { }

        @Override public void onTransitionCancel(Transition transition) { }

        @Override public void onTransitionPause(Transition transition) { }

        @Override public void onTransitionResume(Transition transition) { }
    };

    PadEditorPanel(Context context, PadEditor editor, View surface, float density, boolean perGame, boolean gameAvailable) {
        this.context = context;
        this.editor = editor;
        this.surface = surface;
        this.density = density;
        // Its surface is baked into the layout: an opaque fill and a rim, nothing that blurs or shades the stream.
        panel = LayoutInflater.from(context).inflate(R.layout.pad_editor_panel, editor, false);
        inspector = new PadStudioInspector(panel, editor, this);
        editor.addView(panel, new FrameLayout.LayoutParams(FrameLayout.LayoutParams.WRAP_CONTENT,
                FrameLayout.LayoutParams.WRAP_CONTENT, Gravity.CENTER));
        panel.findViewById(R.id.pad_editor_grip).setOnTouchListener((grip, event) -> dragPanel(event));
        // A panel that grows or shrinks (the picked control's rows), or a surface that turns or folds, keeps a
        // placed panel whole on the surface.
        panel.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> showPlacedPanel());
        int[] placed = editor.panelPosition();
        if (placed != null) placePanel(placed[0], placed[1]);
        tray = panel.findViewById(R.id.pad_editor_scroll);
        sizeRow = panel.findViewById(R.id.pad_studio_size_row);
        opacityRow = panel.findViewById(R.id.pad_studio_opacity_row);
        selection = panel.findViewById(R.id.pad_editor_selection);
        sizeText = panel.findViewById(R.id.pad_editor_size);
        controlSection = panel.findViewById(R.id.pad_editor_control);
        layoutSection = panel.findViewById(R.id.pad_editor_layout);
        addSection = panel.findViewById(R.id.pad_editor_add);
        options = panel.findViewById(R.id.pad_editor_options);
        hide = panel.findViewById(R.id.pad_editor_hide);
        glyphs = panel.findViewById(R.id.pad_editor_glyphs);
        glyphs.setContentDescription(context.getString(R.string.pad_edit_glyphs));
        glyphs.setOnClickListener(v -> editor.chooseGlyphs());
        pointerMode = panel.findViewById(R.id.pad_editor_pointer_mode);
        mouseCard = panel.findViewById(R.id.pad_editor_mode_mouse);
        mouseCard.setOnClickListener(v -> editor.setPointerMode(PadLayout.PointerMode.MOUSE));
        cameraCard = panel.findViewById(R.id.pad_editor_mode_camera);
        cameraCard.setOnClickListener(v -> editor.setPointerMode(PadLayout.PointerMode.CAMERA));
        split = panel.findViewById(R.id.pad_editor_split);
        split.setOnClickListener(v -> editor.split());
        join = panel.findViewById(R.id.pad_editor_join);
        join.setOnClickListener(v -> editor.join());
        hint = panel.findViewById(R.id.pad_editor_hint);
        opacity = panel.findViewById(R.id.pad_editor_opacity);
        panel.findViewById(R.id.pad_editor_smaller).setOnClickListener(v -> editor.scaleBy(-PadEditor.SCALE_STEP));
        panel.findViewById(R.id.pad_editor_larger).setOnClickListener(v -> editor.scaleBy(PadEditor.SCALE_STEP));
        hide.setOnClickListener(v -> editor.toggleHidden());
        MaterialSwitch gameSwitch = panel.findViewById(R.id.pad_editor_this_game);
        thisGame = gameSwitch;
        gameSwitch.setVisibility(gameAvailable ? View.VISIBLE : View.GONE);
        gameSwitch.setChecked(perGame);
        gameSwitch.setOnCheckedChangeListener((button, checked) -> editor.setPerGame(checked));
        thisController = panel.findViewById(R.id.pad_editor_this_controller);
        opacityText = panel.findViewById(R.id.pad_studio_opacity_value);
        reachRow = panel.findViewById(R.id.pad_studio_reach_row);
        reach = panel.findViewById(R.id.pad_editor_reach);
        reachText = panel.findViewById(R.id.pad_studio_reach_value);
        // The app's 7.0 slider: a round thumb on the thin track, and a ring around it while a controller or a
        // keyboard has it, as wide as a finger's touch. Its value stands at the row's end, so none floats over it;
        // TalkBack reads the same text.
        int thumb = context.getResources().getDimensionPixelSize(R.dimen.nj_size_slider_thumb);
        opacity.setThumbRadius(thumb / 2);
        opacity.setHaloRadius(Math.round(PadLayout.MIN_TOUCH_DP / 2 * density));
        opacity.setHaloTintList(ColorStateList.valueOf(ColorUtils.setAlphaComponent(
                ContextCompat.getColor(context, R.color.nj_color_moon), 0x55)));
        opacity.setLabelFormatter(value -> percent(Math.round(value)));
        opacity.addOnChangeListener((slider, value, fromUser) -> {
            opacityText.setText(percent(Math.round(value)));
            if (fromUser && !settingOpacity) editor.setOpacity(Math.round(value));
        });
        // The Touch area slider looks and reads as the opacity one.
        reach.setThumbRadius(thumb / 2);
        reach.setHaloRadius(Math.round(PadLayout.MIN_TOUCH_DP / 2 * density));
        reach.setHaloTintList(ColorStateList.valueOf(ColorUtils.setAlphaComponent(
                ContextCompat.getColor(context, R.color.nj_color_moon), 0x55)));
        reach.setLabelFormatter(value -> percent(Math.round(value)));
        reach.addOnChangeListener((slider, value, fromUser) -> {
            reachText.setText(percent(Math.round(value)));
            if (fromUser && !settingReach) editor.setReach(Math.round(value));
        });
    }

    private static String percent(int value) {
        return String.format(Locale.getDefault(), "%d %%", value);
    }

    View view() {
        return panel;
    }

    void animateEntrance() {
        if (!PadEditor.transitions) return;
        // The dimmed surface and the panel come in quickly rather than at once.
        surface.setAlpha(0);
        surface.animate().alpha(1).setDuration(PANEL_MS).setInterpolator(EASE).start();
        panel.setAlpha(0);
        panel.setScaleX(.96f);
        panel.setScaleY(.96f);
        panel.animate().alpha(1).scaleX(1).scaleY(1).setDuration(PANEL_MS).setInterpolator(EASE).start();
    }

    /**
     * The dock steps back while a finger moves or sizes a control, and comes forward again when it lifts: wherever
     * the dock stands, the control under the finger and the lines it lands on are seen.
     */
    void stepBack(boolean back) {
        if (back == steppedBack) return;
        steppedBack = back;
        float alpha = back ? STEPPED_BACK : 1;
        if (!PadEditor.transitions) {
            panel.setAlpha(alpha);
            return;
        }
        panel.animate().alpha(alpha).setDuration(PANEL_MS / 2).setInterpolator(EASE).start();
    }

    /** The Add tile for {@code control}, or null when the layout shows it; for tests. */
    View tile(PadLayout.Control control) {
        for (PadEditor.Tile tile : tiles) {
            if (tile.control == control) return tile;
        }
        return null;
    }

    void onEditorSizeChanged(int w, int h, int oldw, int oldh) {
        // As wide as its bar on one row where the surface has the room, else with the tools above the actions and
        // never wider than a phone in portrait leaves room for. A short surface (a phone in landscape) gets a lower
        // tray without the hint, so it covers fewer controls.
        boolean shortSurface = h < SHORT_SURFACE_DP * density, oneRow = PadStudioInspector.oneRow(w, density);
        int width = PadStudioInspector.width(w, density);
        FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) panel.getLayoutParams();
        if (params.width != width || oneRow != this.oneRow || shortSurface != this.shortSurface) {
            params.width = width;
            editor.post(() -> {
                this.oneRow = oneRow;
                this.shortSurface = shortSurface;
                inspector.arrange(oneRow);
                update();
                panel.setLayoutParams(params);
                keepClear();
            });
        }
        if (oldw != 0 && oldh != 0 && (w > h) != (oldw > oldh)) {
            // Turned while editing: the panel goes where it was left in this orientation, or back to the middle.
            editor.post(() -> {
                int[] placed = editor.panelPosition();
                if (placed != null) {
                    placePanel(placed[0], placed[1]);
                } else {
                    unplacePanel();
                }
            });
        } else {
            editor.post(this::showPlacedPanel);
        }
    }

    /**
     * The Only with switch for the connected controller {@code name}, on when the layout is already its own; hidden
     * without a controller. While it is on the layout is that controller's in every game, so the game switch rests.
     */
    void offerController(String name, boolean on) {
        if (name == null) return;
        thisController.setText(thisController.getContext().getString(R.string.pad_edit_this_controller, name));
        thisController.setVisibility(View.VISIBLE);
        thisController.setChecked(on);
        thisGame.setEnabled(!on);
        thisController.setOnCheckedChangeListener((button, checked) -> {
            editor.setForController(checked);
            thisGame.setEnabled(!checked);
        });
        // The layout showing is that controller's own: it can go without touching the other layouts.
        if (!on) return;
        MaterialButton forget = panel.findViewById(R.id.pad_editor_forget_controller);
        forget.setText(forget.getContext().getString(R.string.pad_edit_forget_controller, name));
        forget.setVisibility(View.VISIBLE);
        forget.setOnClickListener(v -> editor.forgetControllerLayout());
    }

    /**
     * Where the glyph chip and the game switch go in the More tray, so it stays low on a short surface: in one row
     * there, the switch taking what the chip leaves; on a tall surface the chip above the switch.
     */
    private void arrangeLayout() {
        options.setOrientation(shortSurface ? LinearLayout.HORIZONTAL : LinearLayout.VERTICAL);
        options.setGravity(shortSurface ? Gravity.CENTER_VERTICAL : Gravity.NO_GRAVITY);
        LinearLayout.LayoutParams game = (LinearLayout.LayoutParams) thisGame.getLayoutParams();
        game.width = shortSurface ? 0 : LinearLayout.LayoutParams.MATCH_PARENT;
        game.weight = shortSurface ? 1 : 0;
        game.setMarginStart(shortSurface ? Math.round(16 * density) : 0);
        thisGame.setLayoutParams(game);
    }

    /** The grip: dragging it moves the panel anywhere on the surface; letting go remembers where, per orientation. */
    private boolean dragPanel(MotionEvent event) {
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                // Raw coordinates: the grip moves with the panel, so its own would chase the finger.
                gripRawX = event.getRawX();
                gripRawY = event.getRawY();
                gripFromX = panel.getX();
                gripFromY = panel.getY();
                return true;
            case MotionEvent.ACTION_MOVE:
                placePanel(gripFromX + event.getRawX() - gripRawX, gripFromY + event.getRawY() - gripRawY);
                return true;
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_CANCEL:
                if (panelPlaced) editor.savePanelPosition(Math.round(panelX), Math.round(panelY));
                return true;
            default:
                return false;
        }
    }

    /** Puts the panel's top-left corner at ({@code x}, {@code y}), kept whole on the surface, and leaves it there. */
    private void placePanel(float x, float y) {
        FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) panel.getLayoutParams();
        if (!panelPlaced || params.gravity != (Gravity.TOP | Gravity.START)) {
            // The layout puts it in the top-left corner; its translation carries the place, so a drag lays out nothing.
            params.gravity = Gravity.TOP | Gravity.START;
            params.leftMargin = params.topMargin = params.rightMargin = params.bottomMargin = 0;
            panel.setLayoutParams(params);
        }
        panelPlaced = true;
        panelX = Math.max(0, Math.min(maxPanelX(), x));
        panelY = Math.max(0, Math.min(maxPanelY(), y));
        showPlacedPanel();
    }

    /** Shows a placed panel where it was put, moved only as far as it takes to keep it whole on the surface. */
    private void showPlacedPanel() {
        if (!panelPlaced) return;
        float x = Math.max(0, Math.min(maxPanelX(), panelX)), y = Math.max(0, Math.min(maxPanelY(), panelY));
        panel.setTranslationX(x - panel.getLeft());
        panel.setTranslationY(y - panel.getTop());
    }

    /** Back to the panel's own place in the middle, moving out of the picked control's way by itself. */
    private void unplacePanel() {
        panelPlaced = false;
        panel.setTranslationX(0);
        panel.setTranslationY(0);
        FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) panel.getLayoutParams();
        params.gravity = Gravity.CENTER;
        params.leftMargin = params.topMargin = params.rightMargin = params.bottomMargin = 0;
        panel.setLayoutParams(params);
        keepClear();
    }

    private float maxPanelX() {
        return editor.getWidth() > 0 && panel.getWidth() > 0 ? Math.max(0, editor.getWidth() - panel.getWidth()) : Float.MAX_VALUE;
    }

    private float maxPanelY() {
        return editor.getHeight() > 0 && panel.getHeight() > 0 ? Math.max(0, editor.getHeight() - panel.getHeight()) : Float.MAX_VALUE;
    }

    /**
     * Animates the panel's next change: what goes fades out, what comes fades in and the rest, the panel too, moves to
     * its new place; once played, the panel moves out of the picked control's way.
     */
    void transition() {
        if (!PadEditor.transitions || !editor.isLaidOut()) return;
        TransitionSet set = new TransitionSet().setOrdering(TransitionSet.ORDERING_TOGETHER)
                .addTransition(new Fade()).addTransition(new ChangeBounds());
        set.setDuration(PANEL_MS);
        set.setInterpolator(EASE);
        set.excludeTarget(surface, true);
        set.addListener(settled);
        TransitionManager.beginDelayedTransition(editor, set);
    }

    /** Moves the panel to the other half when the picked control sits under it, until the player places it. */
    void keepClear() {
        PadLayout.Control selected = editor.selected();
        if ((selected == null && !editor.hasSelection()) || panel.getWidth() == 0 || panelPlaced) return;
        float[] box = selected != null
                ? editor.draft().bounds(selected, editor.getWidth(), editor.getHeight(), density, new float[4])
                : PadCustomInspector.selectedBox(editor, editor.getWidth(), editor.getHeight(), density, new float[4]);
        if (box == null) return;
        RectF control = new RectF(box[0], box[1], box[0] + box[2], box[1] + box[3]);
        RectF under = new RectF(panel.getLeft(), panel.getTop(), panel.getRight(), panel.getBottom());
        if (!RectF.intersects(control, under)) return;
        FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) panel.getLayoutParams();
        boolean controlHigh = control.centerY() < editor.getHeight() / 2f;
        int gravity = Gravity.CENTER_HORIZONTAL | (controlHigh ? Gravity.BOTTOM : Gravity.TOP);
        int margin = Math.round(16 * density);
        if (params.gravity == gravity && params.topMargin == margin && params.bottomMargin == margin) return;
        transition();
        params.gravity = gravity;
        params.topMargin = params.bottomMargin = margin;
        panel.setLayoutParams(params);
    }

    void update() {
        PadLayout draft = editor.draft();
        PadLayout.Control selected = editor.selected();
        PadTouchZone selectedZone = editor.selectedZone();
        PadCustomButton selectedButton = editor.selectedButton();
        PadLayout.Spec spec = editor.reference();
        String name;
        if (selectedZone != null) {
            name = PadCustomInspector.kindName(context, selectedZone.kind);
        } else if (selectedButton != null) {
            name = selectedButton.effectiveLabel();
        } else {
            name = selected == null ? context.getString(R.string.pad_edit_all) :
                    name(context, selected, draft.glyph());
            if (selected != null && spec.hidden) name = context.getString(R.string.pad_edit_hidden, name);
        }
        selection.setText(name);
        if (selectedZone != null) {
            sizeText.setText(Math.round((selectedZone.right - selectedZone.left) * 100f) + "% × "
                    + Math.round((selectedZone.bottom - selectedZone.top) * 100f) + "%");
        } else if (selectedButton != null) {
            sizeText.setText(percent(Math.round(selectedButton.scale() * 100f)));
        } else {
            sizeText.setText(sizeLabel(spec));
        }
        settingOpacity = true;
        opacity.setValue(Math.max(PadLayout.MIN_OPACITY, Math.min(PadLayout.MAX_OPACITY, Math.round(spec.opacity / 5f) * 5)));
        settingOpacity = false;
        opacityText.setText(percent(Math.round(opacity.getValue())));
        settingReach = true;
        reach.setValue(Math.max(100, Math.min(300, Math.round(spec.reach * 4) * 25)));
        settingReach = false;
        reachText.setText(percent(Math.round(reach.getValue())));
        // The tray shows the picked control, or with none one of the layout's trays: the Add tiles, or every
        // control's size and opacity with the glyphs and the game. With neither the dock is its bar alone.
        boolean picked = selected != null || selectedZone != null || selectedButton != null;
        if (picked) inspector.picked();
        inspector.update(updateAdd());
        boolean adding = !picked && inspector.tray() == PadStudioInspector.ADD;
        boolean more = !picked && inspector.tray() == PadStudioInspector.MORE;
        tray.setVisibility(picked || adding || more ? View.VISIBLE : View.GONE);
        sizeRow.setVisibility(picked || more ? View.VISIBLE : View.GONE);
        opacityRow.setVisibility(selected != null || more ? View.VISIBLE : View.GONE);
        reachRow.setVisibility(selected != null || more ? View.VISIBLE : View.GONE);
        controlSection.setVisibility(selected != null ? View.VISIBLE : View.GONE);
        if (customInspector == null) {
            customInspector = new PadCustomInspector(context, editor,
                    panel.findViewById(R.id.pad_editor_zone_section),
                    panel.findViewById(R.id.pad_editor_kbm_section), density);
        }
        customInspector.update(selectedZone, selectedButton);
        layoutSection.setVisibility(adding || more ? View.VISIBLE : View.GONE);
        addSection.setVisibility(adding ? View.VISIBLE : View.GONE);
        options.setVisibility(more ? View.VISIBLE : View.GONE);
        boolean showing = selected != null && spec.hidden;
        hide.setIconResource(showing ? R.drawable.ic_visibility : R.drawable.ic_visibility_off);
        hide.setText(showing ? R.string.pad_edit_show : R.string.pad_edit_hide);
        boolean pointerPad = selected == PadLayout.Control.POINTER_PAD;
        pointerMode.setVisibility(pointerPad ? View.VISIBLE : View.GONE);
        boolean camera = draft.pointerMode() == PadLayout.PointerMode.CAMERA;
        mouseCard.setSelected(!camera);
        cameraCard.setSelected(camera);
        // The pointer touchpad's line says what Camera does, so the mode is found; the More tray says how to edit
        // where the surface is tall enough for the line.
        hint.setText(pointerPad ? R.string.pad_edit_hint_camera : R.string.pad_edit_hint);
        hint.setVisibility(pointerPad || (more && !shortSurface) ? View.VISIBLE : View.GONE);
        // Split for a shown face cluster or D-pad; Join for one of the members that stand in for it.
        split.setVisibility(selected != null && PadLayout.members(selected).length > 0 && !spec.hidden ? View.VISIBLE : View.GONE);
        join.setVisibility(selected != null && selected.cluster() != null ? View.VISIBLE : View.GONE);
        glyphs.setText(glyphName(context, draft.glyph()));
        arrangeLayout();
    }

    /**
     * Builds the Add tiles again when the controls the layout leaves out changed, and redraws them otherwise. Says
     * whether there is any control to add.
     */
    private boolean updateAdd() {
        PadLayout draft = editor.draft();
        StringBuilder shown = new StringBuilder();
        List<PadLayout.Control> out = new ArrayList<>();
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (!editor.addable(control)) continue;
            out.add(control);
            shown.append(control.name()).append(',');
        }
        shown.append(PadCustomInspector.addStateKey(draft));
        boolean hasAny = !out.isEmpty() || PadCustomInspector.hasAddableExtras(draft);
        if (shown.toString().equals(addShown)) {
            for (PadEditor.Tile tile : tiles) tile.refresh();
            return hasAny;
        }
        addShown = shown.toString();
        addSection.removeAllViews();
        tiles.clear();
        if (!hasAny) return false;
        // Each group its name over its tiles; the groups side by side as far as the dock is wide.
        int gap = Math.round(PadEditor.TILE_GAP_DP * density);
        TileFlow groups = new TileFlow(context, Math.round(PadEditor.GROUP_GAP_DP * density), gap);
        addSection.addView(groups, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        for (int group : new int[] {PadEditor.GAMEPAD, PadEditor.KEYBOARD_MOUSE, PadEditor.EXTRAS}) {
            TileFlow flow = null;
            for (PadLayout.Control control : out) {
                if (PadEditor.addGroup(control) != group) continue;
                if (flow == null) {
                    flow = ensureAddGroup(groups, group, gap);
                }
                PadEditor.Tile tile = editor.newTile(control);
                tiles.add(tile);
                flow.addView(tile);
            }
            if (group == PadEditor.KEYBOARD_MOUSE) {
                if (draft.touchZones().size() < PadTouchZone.MAX_ZONES) {
                    if (flow == null) flow = ensureAddGroup(groups, group, gap);
                    flow.addView(PadCustomInspector.makeAddTile(context, editor, density, true));
                }
                if (draft.customButtons().size() < PadCustomButton.MAX_CUSTOM_BUTTONS) {
                    if (flow == null) flow = ensureAddGroup(groups, group, gap);
                    flow.addView(PadCustomInspector.makeAddTile(context, editor, density, false));
                }
            }
        }
        return true;
    }

    private TileFlow ensureAddGroup(TileFlow groups, int group, int gap) {
        LinearLayout block = new LinearLayout(context);
        block.setOrientation(LinearLayout.VERTICAL);
        block.addView(heading(groupName(group), R.style.TextAppearance_Nightjar_Caption,
                R.color.nj_color_label_secondary));
        TileFlow flow = new TileFlow(context, gap, gap);
        block.addView(flow);
        groups.addView(block);
        return flow;
    }

    private TextView heading(int text, int appearance, int color) {
        TextView view = new TextView(context);
        TextViewCompat.setTextAppearance(view, appearance);
        view.setTextColor(ContextCompat.getColor(context, color));
        view.setText(text);
        view.setPadding(0, Math.round(6 * density), 0, Math.round(4 * density));
        return view;
    }

    private static int groupName(int group) {
        switch (group) {
            case PadEditor.KEYBOARD_MOUSE: return R.string.pad_edit_group_keyboard_mouse;
            case PadEditor.EXTRAS: return R.string.pad_edit_group_extras;
            default: return R.string.pad_edit_group_gamepad;
        }
    }

    /** "120 %", or "150 × 80 %" for a control whose width and height differ. */
    static String sizeLabel(PadLayout.Spec spec) {
        int w = Math.round(spec.widthScale * 100), h = Math.round(spec.heightScale * 100);
        return w == h ? String.format(Locale.getDefault(), "%d %%", w) :
                String.format(Locale.getDefault(), "%d × %d %%", w, h);
    }

    private static String glyphName(Context context, PadLayout.Glyph glyph) {
        switch (glyph) {
            case PLAYSTATION: return context.getString(R.string.pad_glyph_playstation);
            case NINTENDO: return context.getString(R.string.pad_glyph_nintendo);
            default: return context.getString(R.string.pad_glyph_xbox);
        }
    }

    /** The control's name for people: its label, or what it is when it has none; a face button's in its glyph set. */
    static String name(Context context, PadLayout.Control control, PadLayout.Glyph glyph) {
        if (control.shape == PadLayout.Shape.FACE_BUTTON) {
            return context.getString(R.string.pad_face_button, PadGlyphs.face(glyph, control.memberIndex()));
        }
        switch (control) {
            case DPAD_UP: return context.getString(R.string.pad_dpad_up);
            case DPAD_DOWN: return context.getString(R.string.pad_dpad_down);
            case DPAD_LEFT: return context.getString(R.string.pad_dpad_left);
            case DPAD_RIGHT: return context.getString(R.string.pad_dpad_right);
            case LEFT_STICK: return context.getString(R.string.pad_left_stick);
            case RIGHT_STICK: return context.getString(R.string.pad_right_stick);
            case DPAD: return context.getString(R.string.pad_dpad);
            case FACE: return context.getString(R.string.pad_face);
            case SELECT: return context.getString(R.string.pad_view);
            case START: return context.getString(R.string.pad_menu);
            case HOME: return context.getString(R.string.pad_home);
            case TOUCHPAD: return context.getString(R.string.pad_touchpad);
            case POINTER_PAD: return context.getString(R.string.pad_pointer_touchpad);
            case WASD: return context.getString(R.string.pad_wasd);
            case LEFT_PAD: return context.getString(R.string.pad_left_trackpad);
            case RIGHT_PAD: return context.getString(R.string.pad_right_trackpad);
            case LEFT_GRIP: return context.getString(R.string.pad_left_grip);
            case RIGHT_GRIP: return context.getString(R.string.pad_right_grip);
            default: return control.name();
        }
    }

    /**
     * Lays its children out in rows, as many to a row as fit, {@code across} apart in a row and {@code down} between
     * rows; it is as wide as its widest row unless told otherwise, so a flow of flows wraps each in turn.
     */
    static final class TileFlow extends ViewGroup {
        private final int across, down;

        TileFlow(Context context, int across, int down) {
            super(context);
            this.across = across;
            this.down = down;
        }

        @Override
        protected void onMeasure(int widthSpec, int heightSpec) {
            int mode = MeasureSpec.getMode(widthSpec), size = MeasureSpec.getSize(widthSpec);
            int limit = mode == MeasureSpec.UNSPECIFIED ? Integer.MAX_VALUE : size;
            int childWidth = mode == MeasureSpec.UNSPECIFIED ? widthSpec : MeasureSpec.makeMeasureSpec(size, MeasureSpec.AT_MOST);
            int x = 0, y = 0, row = 0, widest = 0;
            for (int i = 0; i < getChildCount(); i++) {
                View child = getChildAt(i);
                if (child.getVisibility() == GONE) continue;
                measureChild(child, childWidth, MeasureSpec.makeMeasureSpec(0, MeasureSpec.UNSPECIFIED));
                if (x > 0 && x + child.getMeasuredWidth() > limit) {
                    x = 0;
                    y += row + down;
                    row = 0;
                }
                x += child.getMeasuredWidth() + across;
                widest = Math.max(widest, x - across);
                row = Math.max(row, child.getMeasuredHeight());
            }
            setMeasuredDimension(mode == MeasureSpec.EXACTLY ? size : Math.min(limit, widest), resolveSize(y + row, heightSpec));
        }

        @Override
        protected void onLayout(boolean changed, int l, int t, int r, int b) {
            int width = r - l, x = 0, y = 0, row = 0;
            for (int i = 0; i < getChildCount(); i++) {
                View child = getChildAt(i);
                if (child.getVisibility() == GONE) continue;
                int w = child.getMeasuredWidth(), h = child.getMeasuredHeight();
                if (x > 0 && x + w > width) {
                    x = 0;
                    y += row + down;
                    row = 0;
                }
                child.layout(x, y, x + w, y + h);
                x += w + across;
                row = Math.max(row, h);
            }
        }
    }
}
