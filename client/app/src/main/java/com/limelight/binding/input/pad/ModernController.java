package com.limelight.binding.input.pad;

import android.content.Context;
import android.content.SharedPreferences;
import android.graphics.Rect;
import android.os.Handler;
import android.os.Looper;
import android.view.Gravity;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.LayoutGamepad;
import com.limelight.preferences.PreferenceConfiguration;

import java.util.EnumMap;
import java.util.List;
import java.util.Map;

/**
 * The modern on-screen controller: analog sticks that float or stay put, a D-pad, the face cluster,
 * shoulders, triggers, stick clicks and View, Menu and Home. Controls are separate views in the
 * stream's frame so touches between them reach the stream. Its input is the on-screen gamepad, the
 * same report the classic controller sends, and its layout is kept for all games or for one game.
 */
public final class ModernController {
    static final String PREFS = "osc_modern";
    static final String GLOBAL_KEY = "layout";
    /** The classic controller's resends: some hosts drop a report that follows another closely. */
    private static final long[] RESEND_MS = {25, 50, 75};

    private final Context context;
    private final FrameLayout frame;
    private final PreferenceConfiguration prefs;
    private final String gameKey;
    private final SharedPreferences store;
    private final LayoutGamepad gamepad;
    private final PadPainter painter;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Map<PadLayout.Control, PadControlView> views = new EnumMap<>(PadLayout.Control.class);
    private final Runnable resend;
    private final View.OnLayoutChangeListener resize;
    private PadLayout layout;
    private boolean perGame, shown = true;
    private PadEditor editor;
    private final float[] box = new float[4];

    /**
     * @param gameKey {@link com.limelight.binding.input.TouchLayout#profileKey} of the game, or null when
     *                the stream has none; then only the layout for all games exists.
     */
    public ModernController(Context context, FrameLayout frame, HeldInputState.Sink sink,
                            PreferenceConfiguration prefs, String gameKey) {
        this.context = context;
        this.frame = frame;
        this.prefs = prefs;
        this.gameKey = gameKey == null || gameKey.isEmpty() ? null : GLOBAL_KEY + ":" + gameKey;
        this.store = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        this.gamepad = new LayoutGamepad(sink);
        this.painter = new PadPainter(context);
        resend = gamepad::resend;
        resize = (v, l, t, r, b, ol, ot, or, ob) -> {
            if (r - l != or - ol || b - t != ob - ot) place();
        };
        load();
    }

    private void load() {
        String game = gameKey == null ? null : store.getString(gameKey, null);
        perGame = game != null;
        layout = PadLayout.parse(perGame ? game : store.getString(GLOBAL_KEY, null));
    }

    /** Whether the settings keep this control off whatever the layout says. */
    boolean forcedHidden(PadLayout.Control control) {
        if (prefs.onlyL3R3) return control != PadLayout.Control.L3 && control != PadLayout.Control.R3;
        return control == PadLayout.Control.HOME && !prefs.showGuideButton;
    }

    boolean shows(PadLayout layout, PadLayout.Control control) {
        return !forcedHidden(control) && !layout.spec(control).hidden;
    }

    /** Builds the controls again from the saved layout and the settings. */
    public void refreshLayout() {
        releaseInput();
        removeElements();
        load();
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (forcedHidden(control)) continue;
            PadControlView view = new PadControlView(context, control, painter, gamepad, this::changed);
            view.configureStick(prefs.oscFloatingSticks, prefs.oscStickDeadzone / 100f);
            view.setHaptics(prefs.vibrateOsc);
            views.put(control, view);
            frame.addView(view, new FrameLayout.LayoutParams(0, 0, Gravity.TOP | Gravity.START));
        }
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
            if (width == 0 || height == 0) continue;
            layout.bounds(control, width, height, density, box);
            FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) view.getLayoutParams();
            params.width = Math.round(box[2]);
            params.height = Math.round(box[3]);
            params.leftMargin = Math.round(box[0]);
            params.topMargin = Math.round(box[1]);
            view.setLayoutParams(params);
            view.setRestingAlpha(prefs.oscOpacity / 100f * layout.spec(control).opacity / 100f);
        }
    }

    private void changed() {
        handler.removeCallbacks(resend);
        for (long delay : RESEND_MS) handler.postDelayed(resend, delay);
    }

    public void show() {
        shown = true;
        place();
    }

    public void hide() {
        shown = false;
        releaseInput();
        place();
    }

    public void releaseInput() {
        for (PadControlView view : views.values()) view.release();
        handler.removeCallbacks(resend);
        gamepad.releaseAll();
    }

    public void removeElements() {
        frame.removeOnLayoutChangeListener(resize);
        for (PadControlView view : views.values()) frame.removeView(view);
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
    }

    /**
     * The bottom of the top row (View, Home, Menu) in the frame's pixels, so the microphone button can sit
     * below it; NaN when none of them shows.
     */
    public float topRowBottom() {
        int width = frame.getWidth(), height = frame.getHeight();
        if (!shown || editor != null || width == 0 || height == 0) return Float.NaN;
        float density = context.getResources().getDisplayMetrics().density, bottom = Float.NaN;
        for (PadLayout.Control control : new PadLayout.Control[] {PadLayout.Control.SELECT, PadLayout.Control.HOME,
                PadLayout.Control.START}) {
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

    /** Opens the layout editor over the stream; input pauses until it closes. */
    public void startEditing() {
        if (editor != null) return;
        releaseInput();
        editor = new PadEditor(context, this, painter, layout.copy(), perGame, gameKey != null);
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
        if (save) save(closing.draft(), closing.perGame());
        place();
    }

    void save(PadLayout draft, boolean forThisGame) {
        layout.copyFrom(draft);
        SharedPreferences.Editor edit = store.edit();
        String text = draft.serialize();
        if (forThisGame && gameKey != null) {
            edit.putString(gameKey, text);
        } else {
            // Saving for all games drops this game's own layout, so the shared one shows here too.
            edit.putString(GLOBAL_KEY, text);
            if (gameKey != null) edit.remove(gameKey);
        }
        edit.apply();
        perGame = forThisGame && gameKey != null;
    }

    /** Forgets every saved modern layout, for all games and for each game. */
    public static void clearLayouts(Context context) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().clear().apply();
    }

    /** The layout for all games, for the editor's "only this game" switch going off. */
    PadLayout globalLayout() {
        return PadLayout.parse(store.getString(GLOBAL_KEY, null));
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
}
