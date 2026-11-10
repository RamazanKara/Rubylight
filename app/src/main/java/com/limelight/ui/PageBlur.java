package com.limelight.ui;

import android.app.Activity;
import android.app.Dialog;
import android.graphics.RenderEffect;
import android.graphics.Shader;
import android.os.Build;
import android.provider.Settings;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;

import androidx.annotation.RequiresApi;

import java.util.Map;
import java.util.WeakHashMap;
import java.util.function.Consumer;

/**
 * The page behind a dialog or a sheet, blurred while that window is open (Nightjar #61). The dim stays as it was and is all
 * there is wherever the blur is not allowed.
 * <ul>
 *   <li>Never over a stream: a blur behind a window makes the system compose every stream frame on the GPU. What a stream's
 *   own menu and dialogs do is {@link StreamGlass}'s, not this class's.</li>
 *   <li>Otherwise by the rule of all live glass ({@link MenuBlurWindow#allowed}): Android 12 or later and a tier that affords
 *   live glass (so not on a low-memory device, in battery saver, when hot or in high contrast).</li>
 *   <li>Where the system's own switch is on, the system blurs behind the window ({@link Kind#WINDOW}). Where it is off
 *   although the tier affords live glass, the system has no cross-window blur at all (some makers ship Android 12+ without
 *   it), so the app blurs its own page under the window ({@link Kind#PAGE}): a render effect on the page's views, which
 *   needs nothing of the system's compositor. Only the user's own "no window blurs" developer switch keeps both off.
 *   The system's switch is followed while the window is open.</li>
 * </ul>
 */
final class PageBlur {
    /** How the page behind a window blurs, if at all. */
    enum Kind {
        /** The dim alone. */
        NONE,
        /** The system blurs behind the window ({@code FLAG_BLUR_BEHIND}). */
        WINDOW,
        /** The app blurs its own page under the window ({@code View.setRenderEffect}). */
        PAGE
    }

    /** For a test: forces whether the user turned window blurs off in the developer options. */
    static Boolean blursOffForTest;

    // How many open windows blur each page in the app: a sheet's dialog over a sheet must not end the sheet's blur.
    private static final Map<View, Integer> pageHolds = new WeakHashMap<>();

    private final Window window;
    private final Activity owner;
    private final int radius;
    // The system's own answer is read again when it changes, as on every show: one rule, one place.
    private final Consumer<Boolean> listener = on -> refresh();
    private boolean listening;
    private boolean shown;
    private boolean holdsPage;
    private Kind kind = Kind.NONE;

    /** The system's blur behind the window: the rule. */
    static boolean allowed(boolean overStream, int api, GlassTier tier, boolean systemSwitch) {
        return !overStream && MenuBlurWindow.allowed(api, tier, systemSwitch);
    }

    /**
     * The rule for both blurs. {@code userTurnedOff} is the developer option "Allow window-level blurs" turned off: the user
     * asked for no blur, which the app's own blur follows as well.
     */
    static Kind kind(boolean overStream, int api, GlassTier tier, boolean systemSwitch, boolean userTurnedOff) {
        if (allowed(overStream, api, tier, systemSwitch)) return Kind.WINDOW;
        if (overStream || userTurnedOff || !MenuBlurWindow.allowed(api, tier, true)) return Kind.NONE;
        return Kind.PAGE;
    }

    /**
     * Blurs the page behind {@code dialog} from now on, whenever the rule allows it. Null where it never can: over a stream,
     * before Android 12, or for a dialog no activity owns.
     */
    static PageBlur behind(Dialog dialog) {
        Window window = dialog.getWindow();
        Activity owner = StreamGlass.activityOf(dialog.getContext());
        if (window == null || owner == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return null;
        if (StreamGlass.overStream(owner)) return null;
        PageBlur blur = new PageBlur(window, owner);
        blur.refresh();
        blur.follow(window.getDecorView());
        return blur;
    }

    private PageBlur(Window window, Activity owner) {
        this.window = window;
        this.owner = owner;
        // The panel material's own radius: the page reads as behind the panel, at the depth its glass is drawn for.
        radius = Math.round(GlassSpec.of(owner, "panel", GlassTier.LIVE, false).blur);
    }

    /** Reads the tier and the system's switch again: a sheet shown a second time may find battery saver on. */
    void refresh() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return;
        kind = kind(false, Build.VERSION.SDK_INT, GlassTier.land(owner), MenuBlurWindow.systemSwitch(owner), userTurnedOff(owner));
        setWindow(kind == Kind.WINDOW);
        holdPage(shown && kind == Kind.PAGE);
    }

    /** Whether the window asks the system for the blur right now. */
    boolean on() {
        return (window.getAttributes().flags & WindowManager.LayoutParams.FLAG_BLUR_BEHIND) != 0;
    }

    /** Which blur the rule chose when last asked. */
    Kind kind() {
        return kind;
    }

    /** Whether the app blurs this activity's page itself right now, for any window. */
    static boolean pageBlurred(Activity activity) {
        Integer holds = pageHolds.get(activity.getWindow().getDecorView());
        return holds != null && holds > 0;
    }

    private static boolean userTurnedOff(Activity owner) {
        if (blursOffForTest != null) return blursOffForTest;
        try {
            // Settings.Global.DISABLE_WINDOW_BLURS, readable but not public.
            return Settings.Global.getInt(owner.getContentResolver(), "disable_window_blurs", 0) != 0;
        } catch (RuntimeException e) {
            return false;
        }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private void setWindow(boolean blurred) {
        WindowManager.LayoutParams params = window.getAttributes();
        int wanted = blurred ? radius : 0;
        if (on() == blurred && params.getBlurBehindRadius() == wanted) return;
        // The flag change hands the window its attributes, the radius among them.
        params.setBlurBehindRadius(wanted);
        if (blurred) window.addFlags(WindowManager.LayoutParams.FLAG_BLUR_BEHIND);
        else window.clearFlags(WindowManager.LayoutParams.FLAG_BLUR_BEHIND);
    }

    // The page is the activity's decor view: a dialog has its own window, so the effect never reaches the dialog itself.
    private void holdPage(boolean hold) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S || holdsPage == hold || radius <= 0) return;
        holdsPage = hold;
        View page = owner.getWindow().getDecorView();
        Integer before = pageHolds.get(page);
        int holds = (before == null ? 0 : before) + (hold ? 1 : -1);
        if (holds <= 0) {
            pageHolds.remove(page);
            page.setRenderEffect(null);
        } else {
            pageHolds.put(page, holds);
            // Clamped, so the screen's edges stay as bright as its middle.
            if (holds == 1) page.setRenderEffect(RenderEffect.createBlurEffect(radius, radius, Shader.TileMode.CLAMP));
        }
    }

    // The system's switch is heard only while the window is up: nothing of a closed dialog stays registered.
    private void follow(View decor) {
        decor.addOnAttachStateChangeListener(new View.OnAttachStateChangeListener() {
            @Override public void onViewAttachedToWindow(View view) {
                shown(true);
            }

            @Override public void onViewDetachedFromWindow(View view) {
                shown(false);
            }
        });
        if (decor.isAttachedToWindow()) shown(true);
    }

    private void shown(boolean on) {
        if (shown == on) return;
        shown = on;
        listen(on);
        if (on) refresh();
        else holdPage(false);
    }

    private void listen(boolean on) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S || listening == on) return;
        listening = on;
        WindowManager manager = owner.getWindowManager();
        if (on) manager.addCrossWindowBlurEnabledListener(listener);
        else manager.removeCrossWindowBlurEnabledListener(listener);
    }
}
