package com.limelight.binding.input.pad;

import android.content.Context;
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
 * Manages {@link PadEditor}'s floating glass panel: its responsive layout on tall and short surfaces, its grip drag
 * and per-orientation saved placement, smooth transitions between picked-control and layout sections, the grouped Add
 * tiles and control/glyph labels.
 */
final class PadEditorPanel {
    private static final float PANEL_WIDTH_DP = 400, PANEL_MARGIN_DP = 16, SHORT_SURFACE_DP = 480;
    /**
     * On a short surface the panel is wider and lower: the preset, glyphs and game choice sit beside the Add tiles, or
     * in one row below them when the tiles need the width.
     */
    private static final float SHORT_PANEL_WIDTH_DP = 480;
    /** How long the panel takes to come in and to change between the picked control and the layout. */
    private static final long PANEL_MS = 200;
    private static final Interpolator EASE = new PathInterpolator(.2f, 0, 0, 1);

    private final Context context;
    private final PadEditor editor;
    private final View surface;
    private final float density;
    private final View panel, controlSection, pointerMode, mouseCard, cameraCard, thisGame;
    private final LinearLayout layoutSection, options, addSection;
    /** The Add groups side by side, for measuring whether they fit beside the options. */
    private TileFlow groups;
    private final TextView selection, sizeText, hint;
    private final MaterialButton hide, reset, split, join;
    private final Chip glyphs;
    private final Slider opacity;
    private boolean compactButtons, shortSurface, settingOpacity;
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
        panel = LayoutInflater.from(context).inflate(R.layout.pad_editor_panel, editor, false);
        // The stream's glass for a modal panel without a blur: its text never sits on a control or the game.
        float corner = 16 * context.getResources().getDisplayMetrics().density;
        panel.setBackground(com.limelight.ui.GlassPanelDrawable.stream(context,
                new float[] {corner, corner, corner, corner, corner, corner, corner, corner}));
        editor.addView(panel, new FrameLayout.LayoutParams(FrameLayout.LayoutParams.WRAP_CONTENT,
                FrameLayout.LayoutParams.WRAP_CONTENT, Gravity.CENTER));
        panel.findViewById(R.id.pad_editor_grip).setOnTouchListener((grip, event) -> dragPanel(event));
        // A panel that grows or shrinks (the picked control's rows), or a surface that turns or folds, keeps a
        // placed panel whole on the surface.
        panel.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> showPlacedPanel());
        int[] placed = editor.panelPosition();
        if (placed != null) placePanel(placed[0], placed[1]);
        selection = panel.findViewById(R.id.pad_editor_selection);
        sizeText = panel.findViewById(R.id.pad_editor_size);
        controlSection = panel.findViewById(R.id.pad_editor_control);
        layoutSection = panel.findViewById(R.id.pad_editor_layout);
        addSection = panel.findViewById(R.id.pad_editor_add);
        options = panel.findViewById(R.id.pad_editor_options);
        hide = panel.findViewById(R.id.pad_editor_hide);
        reset = panel.findViewById(R.id.pad_editor_reset);
        Chip preset = panel.findViewById(R.id.pad_editor_preset);
        preset.setOnClickListener(v -> editor.choosePreset());
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
        reset.setOnClickListener(v -> editor.resetDraft());
        panel.findViewById(R.id.pad_editor_cancel).setOnClickListener(v -> editor.finishEditing(false));
        panel.findViewById(R.id.pad_editor_done).setOnClickListener(v -> editor.finishEditing(true));
        MaterialSwitch gameSwitch = panel.findViewById(R.id.pad_editor_this_game);
        thisGame = gameSwitch;
        gameSwitch.setVisibility(gameAvailable ? View.VISIBLE : View.GONE);
        gameSwitch.setChecked(perGame);
        gameSwitch.setOnCheckedChangeListener((button, checked) -> editor.setPerGame(checked));
        com.limelight.ui.StepSlider.showValue(opacity, value -> String.format(Locale.getDefault(), "%d %%", value));
        opacity.addOnChangeListener((slider, value, fromUser) -> {
            if (fromUser && !settingOpacity) editor.setOpacity(Math.round(value));
        });
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

    /** The Add tile for {@code control}, or null when the layout shows it; for tests. */
    View tile(PadLayout.Control control) {
        for (PadEditor.Tile tile : tiles) {
            if (tile.control == control) return tile;
        }
        return null;
    }

    void onEditorSizeChanged(int w, int h, int oldw, int oldh) {
        // As wide as the design asks on a tablet, never wider than a phone in portrait leaves room for. A short surface
        // (a phone in landscape) gets a wider, lower panel without the hint, so it covers fewer controls.
        boolean shortSurface = h < SHORT_SURFACE_DP * density;
        int width = Math.round(Math.min((shortSurface ? SHORT_PANEL_WIDTH_DP : PANEL_WIDTH_DP) * density,
                w - 2 * PANEL_MARGIN_DP * density));
        // A narrow panel shows Reset as its icon so Cancel and Done stay on the row.
        boolean narrow = width < PANEL_WIDTH_DP * density;
        FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) panel.getLayoutParams();
        if (params.width != width || narrow != compactButtons || shortSurface != this.shortSurface) {
            params.width = Math.max(0, width);
            editor.post(() -> {
                compactButtons = narrow;
                this.shortSurface = shortSurface;
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
     * Where the preset and glyph chips and the game switch go, so the panel stays low on a short surface: beside the
     * Add tiles when every group of them fits there in one row, else in one row below them. On a tall surface the
     * chips sit above the switch, below the tiles.
     */
    private void arrangeLayout() {
        int margin = Math.round(12 * density);
        int content = ((FrameLayout.LayoutParams) panel.getLayoutParams()).width - panel.getPaddingStart() - panel.getPaddingEnd();
        boolean beside = false;
        if (shortSurface && content > 0 && addSection.getVisibility() == View.VISIBLE && groups != null) {
            int any = View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED);
            // The options as wide as their own words, whatever row they were in before.
            View chips = (View) glyphs.getParent();
            chips.measure(any, any);
            thisGame.measure(any, any);
            groups.measure(any, any);
            int side = Math.max(chips.getMeasuredWidth(), thisGame.getMeasuredWidth());
            beside = groups.getMeasuredWidth() <= content - side - margin;
        }
        layoutSection.setOrientation(beside ? LinearLayout.HORIZONTAL : LinearLayout.VERTICAL);
        LinearLayout.LayoutParams add = (LinearLayout.LayoutParams) addSection.getLayoutParams();
        add.width = beside ? 0 : LinearLayout.LayoutParams.MATCH_PARENT;
        add.weight = beside ? 1 : 0;
        add.setMarginEnd(beside ? margin : 0);
        addSection.setLayoutParams(add);
        boolean oneRow = shortSurface && !beside;
        options.setOrientation(oneRow ? LinearLayout.HORIZONTAL : LinearLayout.VERTICAL);
        options.setGravity(oneRow ? Gravity.CENTER_VERTICAL : Gravity.NO_GRAVITY);
        LinearLayout.LayoutParams side = (LinearLayout.LayoutParams) options.getLayoutParams();
        side.width = beside ? LinearLayout.LayoutParams.WRAP_CONTENT : LinearLayout.LayoutParams.MATCH_PARENT;
        options.setLayoutParams(side);
        // Beside the tiles the switch is as wide as its words, so its column is; in one row it takes what the chips
        // leave; below the chips it spans the panel.
        LinearLayout.LayoutParams game = (LinearLayout.LayoutParams) thisGame.getLayoutParams();
        game.width = beside ? LinearLayout.LayoutParams.WRAP_CONTENT : oneRow ? 0 : LinearLayout.LayoutParams.MATCH_PARENT;
        game.weight = oneRow ? 1 : 0;
        game.setMarginStart(oneRow ? Math.round(16 * density) : 0);
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
        if (selected == null || panel.getWidth() == 0 || panelPlaced) return;
        float[] box = editor.draft().bounds(selected, editor.getWidth(), editor.getHeight(), density, new float[4]);
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
        PadLayout.Spec spec = editor.reference();
        String name = selected == null ? context.getString(R.string.pad_edit_all) :
                name(context, selected, draft.glyph());
        if (selected != null && spec.hidden) name = context.getString(R.string.pad_edit_hidden, name);
        selection.setText(name);
        sizeText.setText(sizeLabel(spec));
        settingOpacity = true;
        opacity.setValue(Math.max(PadLayout.MIN_OPACITY, Math.min(PadLayout.MAX_OPACITY, Math.round(spec.opacity / 5f) * 5)));
        settingOpacity = false;
        // A picked control shows its own section; with none, the layout's: the Add tiles, preset, glyphs and game.
        boolean picked = selected != null;
        controlSection.setVisibility(picked ? View.VISIBLE : View.GONE);
        layoutSection.setVisibility(picked ? View.GONE : View.VISIBLE);
        boolean showing = picked && spec.hidden;
        hide.setIconResource(showing ? R.drawable.ic_visibility : R.drawable.ic_visibility_off);
        hide.setText(showing ? R.string.pad_edit_show : R.string.pad_edit_hide);
        label(reset, context.getString(R.string.pad_edit_reset));
        boolean pointerPad = selected == PadLayout.Control.POINTER_PAD;
        pointerMode.setVisibility(pointerPad ? View.VISIBLE : View.GONE);
        boolean camera = draft.pointerMode() == PadLayout.PointerMode.CAMERA;
        mouseCard.setSelected(!camera);
        cameraCard.setSelected(camera);
        // The pointer touchpad's line says what Camera does, so the mode is found; a short surface shows only that.
        hint.setText(pointerPad ? R.string.pad_edit_hint_camera : R.string.pad_edit_hint);
        hint.setVisibility(pointerPad || !shortSurface ? View.VISIBLE : View.GONE);
        // Split for a shown face cluster or D-pad; Join for one of the members that stand in for it.
        split.setVisibility(picked && PadLayout.members(selected).length > 0 && !spec.hidden ? View.VISIBLE : View.GONE);
        join.setVisibility(picked && selected.cluster() != null ? View.VISIBLE : View.GONE);
        glyphs.setText(glyphName(context, draft.glyph()));
        updateAdd();
        arrangeLayout();
    }

    /** Builds the Add tiles again when the controls the layout leaves out changed, and redraws them otherwise. */
    private void updateAdd() {
        StringBuilder shown = new StringBuilder();
        List<PadLayout.Control> out = new ArrayList<>();
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (!editor.addable(control)) continue;
            out.add(control);
            shown.append(control.name()).append(',');
        }
        if (shown.toString().equals(addShown)) {
            for (PadEditor.Tile tile : tiles) tile.refresh();
            return;
        }
        addShown = shown.toString();
        addSection.removeAllViews();
        tiles.clear();
        groups = null;
        addSection.setVisibility(out.isEmpty() ? View.GONE : View.VISIBLE);
        if (out.isEmpty()) return;
        addSection.addView(heading(R.string.pad_edit_add, R.style.TextAppearance_Nightjar_Label, R.color.night_on_surface));
        // Each group its name over its tiles; the groups side by side as far as the panel is wide.
        int gap = Math.round(PadEditor.TILE_GAP_DP * density);
        groups = new TileFlow(context, Math.round(PadEditor.GROUP_GAP_DP * density), gap);
        addSection.addView(groups, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        for (int group : new int[] {PadEditor.GAMEPAD, PadEditor.KEYBOARD_MOUSE, PadEditor.EXTRAS}) {
            TileFlow flow = null;
            for (PadLayout.Control control : out) {
                if (PadEditor.addGroup(control) != group) continue;
                if (flow == null) {
                    LinearLayout block = new LinearLayout(context);
                    block.setOrientation(LinearLayout.VERTICAL);
                    block.addView(heading(groupName(group), R.style.TextAppearance_Nightjar_Caption,
                            R.color.night_on_surface_variant));
                    flow = new TileFlow(context, gap, gap);
                    block.addView(flow);
                    groups.addView(block);
                }
                PadEditor.Tile tile = editor.newTile(control);
                tiles.add(tile);
                flow.addView(tile);
            }
        }
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

    /** A labelled button, or on a narrow panel its icon alone with the label as description and tooltip. */
    private void label(MaterialButton button, String text) {
        // The style's own padding and minimum width, kept so a panel that widens again restores them.
        if (button.getTag(R.id.pad_editor_hide) == null) {
            button.setTag(R.id.pad_editor_hide, new int[] {button.getPaddingStart(), button.getPaddingEnd(), button.getMinWidth()});
        }
        int[] wide = (int[]) button.getTag(R.id.pad_editor_hide);
        int touch = Math.round(PadLayout.MIN_TOUCH_DP * density);
        // Icon alone: a 48 dp square with the icon in its middle, so the row fits a phone.
        int side = compactButtons ? Math.max(0, (touch - button.getIconSize()) / 2) : 0;
        button.setText(compactButtons ? null : text);
        button.setContentDescription(compactButtons ? text : null);
        button.setIconPadding(compactButtons ? 0 : Math.round(8 * density));
        button.setIconGravity(compactButtons ? MaterialButton.ICON_GRAVITY_TEXT_START : MaterialButton.ICON_GRAVITY_START);
        button.setPaddingRelative(compactButtons ? side : wide[0], button.getPaddingTop(),
                compactButtons ? side : wide[1], button.getPaddingBottom());
        button.setMinWidth(compactButtons ? touch : wide[2]);
        button.setMinimumWidth(compactButtons ? touch : wide[2]);
        androidx.appcompat.widget.TooltipCompat.setTooltipText(button, compactButtons ? text : null);
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
