package com.limelight.binding.input.pad;

import android.content.Context;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.graphics.Rect;
import android.os.Handler;
import android.os.Looper;
import android.view.Gravity;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.R;
import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.LayoutGamepad;
import com.limelight.binding.input.touch.PointerAcceleration;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.AdaptiveLayout;

import java.util.ArrayList;
import java.util.Collections;
import java.util.EnumMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * The modern on-screen controller: analog sticks that float or stay put, a D-pad, the face cluster,
 * shoulders, triggers, stick clicks and View, Menu and Home, a WASD stick and a pointer touchpad the layout
 * editor can add, the face buttons and the D-pad's directions as separate controls when the editor splits them,
 * and Xbox, PlayStation or Nintendo glyphs. Controls are separate views in the stream's frame
 * so touches between them reach the stream. Its input is the on-screen gamepad, the same report the classic
 * controller sends (the WASD stick sends keys, the pointer touchpad mouse input), and its layout is kept per
 * orientation for all games, for one game or for one physical controller, which brings its own layout while it is
 * connected ({@link PadControllerLayouts}). With the Steam Controller controls setting it adds that
 * controller's trackpads, back buttons and grip touch — a tap on a trackpad clicks the pad — sent as
 * the classic controller sends them.
 */
public final class ModernController {
    static final String PREFS = "osc_modern";
    static final String GLOBAL_KEY = PadLayoutKeys.ROOT;
    /** The classic controller's resends: some hosts drop a report that follows another closely. */
    private static final long[] RESEND_MS = {25, 50, 75};

    private final Context context;
    private final FrameLayout frame;
    private final PreferenceConfiguration prefs;
    private final String gameKey;
    private final SharedPreferences store;
    private final LayoutGamepad gamepad;
    private final PadHeldInputs heldInputs;
    /** The WASD stick's keys, Windows virtual-key codes through the sink's {@code key}. */
    private final WasdKeys.Keys keys;
    private final PadPainter painter;
    private final PadCustomOverlay overlay;
    private final AdaptiveLayout.SafeInsetsListener safeInsetsListener = this::setSafeInsets;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Map<PadLayout.Control, PadControlView> views = new EnumMap<>(PadLayout.Control.class);
    private final Runnable resend;
    private final View.OnLayoutChangeListener resize;
    private PadLayout layout;
    private boolean perGame, shown = true, landscape;
    private PadEditor editor;
    private final float[] box = new float[4];
    private TouchSink touchSink;
    private PointerSink pointerSink;
    /** The connected external controllers, {model, name} pairs in the order they connected. */
    private List<String[]> controllers = Collections.emptyList();
    /** The controller whose own layout shows, or null while the layout for this game or all games shows. */
    private String controllerModel;
    /** The controller the open editor's Only with switch saves for, or null when none is connected. */
    private String editTarget;
    private String editTargetName;

    /** Where the Steam Controller trackpads' touches go: touchpad 0 of the on-screen controls' pad. */
    public interface TouchSink {
        void touch(byte eventType, int pointerId, float x, float y, float pressure);
    }

    /** Where the pointer touchpad's mouse output goes; without one it sends nothing. */
    public interface PointerSink {
        /** Relative pointer movement in pixels. */
        void move(int dx, int dy);
        /** A mouse button from {@link com.limelight.nvstream.input.MouseButtonPacket}. */
        void button(int button, boolean down);
        /** Scroll in high-resolution units, 120 to a notch; either may be zero. */
        void scroll(int vertical, int horizontal);
    }

    /**
     * @param gameKey {@link com.limelight.binding.input.TouchLayout#profileKey} of the game, or null when
     *                the stream has none; then only the layout for all games exists.
     */
    public ModernController(Context context, FrameLayout frame, HeldInputState.Sink sink,
                            PreferenceConfiguration prefs, String gameKey) {
        this.context = context;
        this.frame = frame;
        this.prefs = prefs;
        this.gameKey = gameKey == null || gameKey.isEmpty() ? null : gameKey;
        this.store = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        this.gamepad = new LayoutGamepad(sink);
        this.heldInputs = new PadHeldInputs(sink);
        this.keys = heldInputs.asWasdKeys();
        this.painter = new PadPainter(context);
        this.overlay = new PadCustomOverlay(context, frame, painter.skin, heldInputs, gamepad, this::changed);
        this.landscape = landscape();
        resend = gamepad::resend;
        resize = (v, l, t, r, b, ol, ot, or, ob) -> {
            if (r - l != or - ol || b - t != ob - ot) place();
        };
        ClassicLayoutMigration.carryOverOnce(context, store, prefs.flipFaceButtons);
        load();
    }

    private boolean landscape() {
        return context.getResources().getConfiguration().orientation == Configuration.ORIENTATION_LANDSCAPE;
    }

    private void load() {
        // A connected controller's own layout first: the player saved it for that controller, in any game.
        controllerModel = chooseController();
        if (controllerModel != null) {
            perGame = false;
            layout = PadLayout.parse(PadControllerLayouts.read(store, landscape, controllerModel));
            painter.setGlyph(layout.glyph());
            painter.setFacesSwapped(layout.facesSwapped());
            return;
        }
        // This game's own layout first, in this version or the older one, then the layout shared by all games. A
        // game's own choice always wins, so a shared layout saved later never shadows it.
        String text = store.getString(PadLayoutKeys.key(landscape, gameKey), null);
        perGame = text != null && gameKey != null;
        if (text == null && gameKey != null) {
            text = store.getString(PadLayoutKeys.legacyKey(gameKey), null);
            perGame = text != null;
        }
        if (text == null) text = store.getString(PadLayoutKeys.key(landscape, null), null);
        if (text == null) text = store.getString(PadLayoutKeys.legacyKey(null), null);
        layout = PadLayout.parse(text);
        painter.setGlyph(layout.glyph());
        painter.setFacesSwapped(layout.facesSwapped());
    }

    private String chooseController() {
        return chooseController(store, controllers);
    }

    private static String chooseController(SharedPreferences store, List<String[]> connected) {
        List<String> models = new ArrayList<>(connected.size());
        for (String[] pair : connected) models.add(pair[0]);
        return PadControllerLayouts.choose(models, model -> PadControllerLayouts.has(store, model));
    }

    /**
     * Whether one of the connected controllers brings a layout of its own: the player saved it to use touch with that
     * controller, so a connected controller does not hide the controls then. False without any such layout.
     */
    public static boolean controllerBringsLayout(Context context, List<String[]> connected) {
        if (connected == null || connected.isEmpty()) return false;
        return chooseController(context.getSharedPreferences(PREFS, Context.MODE_PRIVATE), connected) != null;
    }

    /**
     * The connected controllers changed: shows the layout saved for the last one to connect that has one, or the
     * layout for this game or all games when none has, as before. An open editor keeps its draft; Done saves where
     * its switches say and the layout to show is chosen then.
     */
    public void setControllers(List<String[]> connected) {
        controllers = connected == null ? Collections.emptyList() : new ArrayList<>(connected);
        if (editor != null || Objects.equals(controllerModel, chooseController())) return;
        releaseInput();
        load();
        overlay.sync(layout, shown, editor != null, prefs.oscOpacity / 100f);
        applyAppearance();
        place();
        if (onLayoutChanged != null) onLayoutChanged.run();
    }

    /** The name a connected controller model shows under, or the model itself. */
    private String controllerName(String model) {
        for (String[] pair : controllers) if (pair[0].equals(model)) return pair[1];
        return model;
    }

    /** Sends the Steam Controller trackpads' touches here; without a sink they send nothing. */
    public void setTouchSink(TouchSink sink) {
        touchSink = sink;
    }

    private void touch(byte eventType, int pointerId, float x, float y, float pressure) {
        if (touchSink != null) touchSink.touch(eventType, pointerId, x, y, pressure);
    }

    /** Sends the pointer touchpad's mouse output here; without a sink it sends nothing. */
    public void setPointerSink(PointerSink sink) {
        pointerSink = sink;
        for (PadControlView view : views.values()) view.setPointerSink(sink);
        overlay.setPointerSink(sink);
    }

    /** Repositions custom buttons and edge-to-edge touch zones when Safe Fit or cutout padding flips. */
    public void setSafeInsets(int left, int top, int right, int bottom) {
        if (overlay.setSafeInsets(left, top, right, bottom)) {
            place();
        }
    }

    /** Whether the settings keep this control off whatever the layout says. */
    boolean forcedHidden(PadLayout.Control control) {
        // As on the classic controller, the Steam Controller controls follow their own setting alone.
        if (control.steam) return !prefs.oscSteamControls;
        if (prefs.onlyL3R3) return control != PadLayout.Control.L3 && control != PadLayout.Control.R3;
        return (control == PadLayout.Control.HOME && !prefs.showGuideButton) ||
                (control == PadLayout.Control.TOUCHPAD && !prefs.showTouchpadButton);
    }

    boolean shows(PadLayout layout, PadLayout.Control control) {
        return !forcedHidden(control) && layout.shown(control);
    }

    /** Builds the controls again from the saved layout and the settings. */
    public void refreshLayout() {
        releaseInput();
        removeElements();
        load();
        painter.setGlyph(layout.glyph());
        painter.setFacesSwapped(layout.facesSwapped());
        // The acceleration curve and the Trackpad speed reach the pointer touchpad's motion model, here and in
        // applyAppearance; the speed sets how far Camera mode turns, PointerMove applies it to the pointer.
        String curve = prefs.trackpadAcceleration ? prefs.trackpadAccelerationCurve : PointerAcceleration.LINEAR;
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (forcedHidden(control)) continue;
            PadControlView view = new PadControlView(context, control, painter, gamepad, this::changed, keys);
            view.configureStick(prefs.oscFloatingSticks, prefs.oscStickDeadzone / 100f);
            view.setHaptics(prefs.vibrateOsc);
            view.setStickTouch(prefs.oscSteamControls);
            view.setTouchSink(this::touch);
            view.setPointerSink(pointerSink);
            view.setAreas(this::nearestArea);
            if (control == PadLayout.Control.POINTER_PAD) {
                view.configurePointer(layout.pointerMode() == PadLayout.PointerMode.CAMERA, curve, prefs.trackpadSpeed);
            }
            views.put(control, view);
        }
        // Each control's glow under every control, so one never covers a neighbour's glyph; then the controls.
        for (PadControlView view : views.values()) {
            frame.addView(view.haloLayer(), new FrameLayout.LayoutParams(0, 0, Gravity.TOP | Gravity.START));
        }
        for (PadControlView view : views.values()) {
            frame.addView(view, new FrameLayout.LayoutParams(0, 0, Gravity.TOP | Gravity.START));
        }
        // A split cluster's members slide into each other: a finger that lands on one holds whichever it is over.
        for (PadLayout.Control cluster : new PadLayout.Control[] {PadLayout.Control.FACE, PadLayout.Control.DPAD}) {
            PadLayout.Control[] members = PadLayout.members(cluster);
            PadControlView[] group = new PadControlView[members.length];
            for (int i = 0; i < members.length; i++) group[i] = views.get(members[i]);
            new PadControlView.Group(group);
        }
        // The sticks' knobs above every control, so a knob pushed past its base covers a neighbour.
        for (PadControlView view : views.values()) {
            View knob = view.knobLayer();
            if (knob != null) frame.addView(knob, new FrameLayout.LayoutParams(0, 0, Gravity.TOP | Gravity.START));
        }
        overlay.setHaptics(prefs.vibrateOsc);
        overlay.setPointerSink(pointerSink);
        overlay.sync(layout, shown, editor != null, prefs.oscOpacity / 100f);
        frame.setTag(R.id.pad_safe_insets_observer, safeInsetsListener);
        frame.addOnLayoutChangeListener(resize);
        place();
    }

    /** Puts every control where the layout says on the frame's current size. */
    void place() {
        int width = frame.getWidth(), height = frame.getHeight();
        float density = context.getResources().getDisplayMetrics().density;
        for (Map.Entry<PadLayout.Control, PadControlView> entry : views.entrySet()) {
            PadLayout.Control control = entry.getKey();
            PadControlView view = entry.getValue();
            boolean visible = shown && editor == null && shows(layout, control);
            if (!visible) view.release();
            view.setVisibility(visible ? View.VISIBLE : View.GONE);
            float reach = layout.spec(control).reach;
            View area = view.reachLayer();
            boolean reaching = visible && reach > PadLayout.MIN_REACH;
            // A touch area past the box (#172) joins the frame only once the layout grows one, under the controls'
            // glows and so under every control; at the default the frame holds what it always held.
            int under = frame.indexOfChild(view.haloLayer());
            if (reaching && area.getParent() == null && under >= 0) {
                frame.addView(area, under, new FrameLayout.LayoutParams(0, 0, Gravity.TOP | Gravity.START));
            }
            area.setVisibility(reaching ? View.VISIBLE : View.GONE);
            if (width == 0 || height == 0) continue;
            layout.bounds(control, width, height, density, box);
            FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) view.getLayoutParams();
            params.width = Math.round(box[2]);
            params.height = Math.round(box[3]);
            params.leftMargin = Math.round(box[0]);
            params.topMargin = Math.round(box[1]);
            view.setLayoutParams(params);
            // Its glow's view: the box grown by the glow's reach on every side.
            around(view.haloLayer(), params, view.haloReach());
            // Its touch area: the box grown to the reach, centred on it.
            if (area.getVisibility() == View.VISIBLE) {
                FrameLayout.LayoutParams grown = (FrameLayout.LayoutParams) area.getLayoutParams();
                grown.width = Math.round(params.width * reach);
                grown.height = Math.round(params.height * reach);
                grown.leftMargin = params.leftMargin - (grown.width - params.width) / 2;
                grown.topMargin = params.topMargin - (grown.height - params.height) / 2;
                area.setLayoutParams(grown);
            }
            View knob = view.knobLayer();
            if (knob != null) {
                // The control's box grown by the knob's overhang on every side; the control keeps the box.
                around(knob, params, view.knobOverhang(params.width, params.height));
            }
            view.setRestingAlpha(prefs.oscOpacity / 100f * layout.spec(control).opacity / 100f);
        }
        overlay.place(shown, editor != null, prefs.oscOpacity / 100f);
    }

    /**
     * The control a finger at the frame's point ({@code x}, {@code y}) works when it lands in {@code owner}'s touch
     * area (#172): the shown control with the nearest box among those whose area holds the point, {@code owner} on a
     * tie, so overlapping areas split between their controls instead of the topmost taking all.
     */
    private PadControlView nearestArea(PadControlView owner, float x, float y) {
        PadControlView best = owner;
        float bestDistance = distance(owner, x, y);
        for (PadControlView view : views.values()) {
            View area = view.reachLayer();
            if (view == owner || view.getVisibility() != View.VISIBLE || area.getVisibility() != View.VISIBLE
                    || area.getParent() == null || x < area.getLeft() || x >= area.getRight() || y < area.getTop()
                    || y >= area.getBottom()) continue;
            float distance = distance(view, x, y);
            if (distance < bestDistance) {
                best = view;
                bestDistance = distance;
            }
        }
        return best;
    }

    /** The squared distance from the frame's point to a view's box, 0 inside it. */
    private static float distance(View box, float x, float y) {
        float dx = Math.max(0, Math.max(box.getLeft() - x, x - box.getRight()));
        float dy = Math.max(0, Math.max(box.getTop() - y, y - box.getBottom()));
        return dx * dx + dy * dy;
    }

    /** Lays {@code layer} out as the box of {@code control} grown by {@code reach} pixels on every side. */
    private static void around(View layer, FrameLayout.LayoutParams control, int reach) {
        FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) layer.getLayoutParams();
        params.width = control.width + 2 * reach;
        params.height = control.height + 2 * reach;
        params.leftMargin = control.leftMargin - reach;
        params.topMargin = control.topMargin - reach;
        layer.setLayoutParams(params);
    }

    private void changed() {
        handler.removeCallbacks(resend);
        for (long delay : RESEND_MS) handler.postDelayed(resend, delay);
    }

    public void show() {
        shown = true;
        place();
        // What sits below the top row (the microphone button) follows the controls back, as when they hide.
        if (onLayoutChanged != null) onLayoutChanged.run();
    }

    public void hide() {
        shown = false;
        releaseInput();
        place();
    }

    /**
     * The window turned: loads this orientation's own layout. An editor's unsaved draft is left alone, since its
     * Done writes to the orientation the stream is in when it closes.
     */
    public void onOrientationChanged() {
        boolean now = landscape();
        if (now == landscape) return;
        landscape = now;
        // An open editor keeps its draft; Done saves it to the orientation the stream is in when it closes.
        if (editor != null) return;
        releaseInput();
        load();
        overlay.sync(layout, shown, editor != null, prefs.oscOpacity / 100f);
        applyAppearance();
        place();
        if (onLayoutChanged != null) onLayoutChanged.run();
    }

    /** Redraws the controls after the glyphs or the pointer touchpad's mode changed. */
    private void applyAppearance() {
        painter.setGlyph(layout.glyph());
        painter.setFacesSwapped(layout.facesSwapped());
        String curve = prefs.trackpadAcceleration ? prefs.trackpadAccelerationCurve : PointerAcceleration.LINEAR;
        for (Map.Entry<PadLayout.Control, PadControlView> entry : views.entrySet()) {
            PadControlView view = entry.getValue();
            if (entry.getKey() == PadLayout.Control.POINTER_PAD) {
                view.configurePointer(layout.pointerMode() == PadLayout.PointerMode.CAMERA, curve, prefs.trackpadSpeed);
            }
            view.invalidate();
        }
    }

    public void releaseInput() {
        for (PadControlView view : views.values()) view.release();
        overlay.releaseInput();
        handler.removeCallbacks(resend);
        gamepad.releaseAll();
        heldInputs.releaseAll();
    }

    public void removeElements() {
        frame.removeOnLayoutChangeListener(resize);
        if (frame.getTag(R.id.pad_safe_insets_observer) == safeInsetsListener) {
            frame.setTag(R.id.pad_safe_insets_observer, null);
        }
        overlay.removeViews();
        for (PadControlView view : views.values()) {
            frame.removeView(view);
            frame.removeView(view.haloLayer());
            frame.removeView(view.reachLayer());
            if (view.knobLayer() != null) frame.removeView(view.knobLayer());
        }
        views.clear();
    }

    /** Removes the controls and any open editor for good. */
    public void destroy() {
        if (editor != null) finishEditing(false);
        releaseInput();
        removeElements();
    }

    /** The visible controls, in the frame's coordinates, so overlays keep clear of them. */
    public void collectVisibleBounds(List<Rect> out) {
        for (PadControlView view : views.values()) {
            com.limelight.ui.OverlayPlacement.addVisible(view, frame, out);
        }
        overlay.collectVisibleBounds(out);
    }

    /**
     * The bottom of the top row (View, Home, Menu, and the Touchpad below them) in the frame's pixels, so the microphone button can sit
     * below it; NaN when none of them shows.
     */
    public float topRowBottom() {
        int width = frame.getWidth(), height = frame.getHeight();
        if (!shown || editor != null || width == 0 || height == 0) return Float.NaN;
        float density = context.getResources().getDisplayMetrics().density, bottom = Float.NaN;
        for (PadLayout.Control control : new PadLayout.Control[] {PadLayout.Control.SELECT, PadLayout.Control.HOME,
                PadLayout.Control.START, PadLayout.Control.TOUCHPAD}) {
            if (!shows(layout, control)) continue;
            layout.bounds(control, width, height, density, box);
            // Only the middle of the top edge counts: a row moved elsewhere leaves the button its own place.
            if (box[1] > height / 3f || Math.abs(box[0] + box[2] / 2 - width / 2f) > width / 4f) continue;
            bottom = Float.isNaN(bottom) ? box[1] + box[3] : Math.max(bottom, box[1] + box[3]);
        }
        return bottom;
    }

    public boolean isEditing() {
        return editor != null;
    }

    /** Whether the controls are on screen, unless the editor covers them. */
    public boolean isShown() {
        return shown;
    }

    /** Opens the layout editor over the stream; input pauses until it closes. */
    public void startEditing() {
        if (editor != null) return;
        releaseInput();
        // Only with names the controller whose layout shows, else the controller that connected last.
        editTarget = controllerModel != null ? controllerModel
                : controllers.isEmpty() ? null : controllers.get(controllers.size() - 1)[0];
        editTargetName = editTarget == null ? null : controllerName(editTarget);
        editor = new PadEditor(context, this, painter, layout.copy(), perGame, gameKey != null,
                editTargetName, controllerModel != null);
        place();
        frame.addView(editor, new FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT));
    }

    /** Closes the editor, keeping its layout when {@code save} is true. */
    public void finishEditing(boolean save) {
        if (editor == null) return;
        PadEditor closing = editor;
        editor = null;
        frame.removeView(closing);
        if (save) save(closing.draft(), closing.perGame(), closing.forController() ? editTarget : null);
        else load(); // Cancel after a rotation shows the new orientation's saved layout, not the old one.
        // Cancel also restores the glyphs and pointer mode the editor previewed.
        overlay.sync(layout, shown, false, prefs.oscOpacity / 100f);
        applyAppearance();
        place();
        if (onLayoutChanged != null) onLayoutChanged.run();
    }

    /**
     * Removes the layout kept for the controller whose layout shows (#154) and closes the editor without saving: the
     * layouts for this game and all games stay as they were, and the controls show the one that applies now.
     */
    void forgetControllerLayout() {
        if (controllerModel == null) return;
        SharedPreferences.Editor edit = store.edit();
        PadControllerLayouts.remove(edit, controllerModel);
        edit.apply();
        controllerModel = null;
        finishEditing(false);
    }

    private Runnable onLayoutChanged;

    /** Runs when the editor closes, so what sits below the top row (the microphone button) can move with it. */
    public void setOnLayoutChanged(Runnable listener) {
        onLayoutChanged = listener;
    }

    void save(PadLayout draft, boolean forThisGame) {
        save(draft, forThisGame, null);
    }

    /**
     * Keeps {@code draft} for {@code forController}, a controller model, when it is set; else for this game or all
     * games. Saving a controller's layout for a game or all games instead takes that controller's own layout away.
     */
    void save(PadLayout draft, boolean forThisGame, String forController) {
        layout.copyFrom(draft);
        SharedPreferences.Editor edit = store.edit();
        String text = draft.serialize();
        if (forController != null) {
            String name = editTargetName != null && Objects.equals(forController, editTarget)
                    ? editTargetName : controllerName(forController);
            PadControllerLayouts.save(edit, landscape, forController, name, text);
            edit.apply();
            editTargetName = null;
            if (!Objects.equals(forController, chooseController())) {
                load();
            } else {
                perGame = false;
                controllerModel = forController;
            }
            applyAppearance();
            return;
        }
        editTargetName = null;
        if (controllerModel != null) {
            PadControllerLayouts.remove(edit, controllerModel);
            controllerModel = null;
        }
        if (forThisGame && gameKey != null) {
            edit.putString(PadLayoutKeys.key(landscape, gameKey), text);
        } else {
            // Saving for all games drops this game's own layout in this orientation so the shared one shows
            // here. The other orientation keeps its own, so saving in portrait never deletes a landscape one.
            edit.putString(PadLayoutKeys.key(landscape, null), text);
            if (gameKey != null) {
                edit.remove(PadLayoutKeys.key(landscape, gameKey));
                // The old single per-game layout is both orientations' fallback: hand it to the other orientation
                // before removing it, so a downgrade-sized layout is not lost there.
                String legacy = store.getString(PadLayoutKeys.legacyKey(gameKey), null);
                String other = PadLayoutKeys.key(!landscape, gameKey);
                if (legacy != null && !store.contains(other)) edit.putString(other, legacy);
                edit.remove(PadLayoutKeys.legacyKey(gameKey));
            }
        }
        edit.apply();
        perGame = forThisGame && gameKey != null;
        // Another connected controller with its own layout still brings it.
        if (chooseController() != null) load();
        overlay.sync(layout, shown, editor != null, prefs.oscOpacity / 100f);
        applyAppearance();
    }

    /** Forgets every saved modern layout, for all games and for each game; Reset means default, not the old classic layout. */
    public static void clearLayouts(Context context) {
        // The carry-over stays done, and the retired style stays read, so a reset means the defaults.
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().clear()
                .putBoolean(ClassicLayoutMigration.MIGRATED, true)
                .putBoolean(ClassicLayoutMigration.STYLE_RETIRED, true).apply();
    }

    /**
     * Where the player left the layout editor's panel in this orientation, its top-left corner in pixels; null when
     * it was never moved. The editor keeps it on the surface, so a place from a larger screen or a fold still shows.
     */
    int[] panelPosition() {
        String value = store.getString(PadLayoutKeys.panelKey(landscape), null);
        if (value == null) return null;
        String[] parts = value.split(",");
        if (parts.length != 2) return null;
        try {
            return new int[] {Integer.parseInt(parts[0].trim()), Integer.parseInt(parts[1].trim())};
        } catch (NumberFormatException e) {
            return null;
        }
    }

    /** Remembers where the editor's panel was left, for this orientation. */
    void savePanelPosition(int left, int top) {
        store.edit().putString(PadLayoutKeys.panelKey(landscape), left + "," + top).apply();
    }

    PadLayout layout() {
        return layout;
    }

    PreferenceConfiguration prefs() {
        return prefs;
    }

    PadControlView view(PadLayout.Control control) {
        return views.get(control);
    }

    FrameLayout frame() {
        return frame;
    }

    PadCustomOverlay customOverlay() {
        return overlay;
    }

    PadHeldInputs heldInputs() {
        return heldInputs;
    }
}
