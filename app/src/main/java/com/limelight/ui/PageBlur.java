package com.limelight.ui;

import android.app.Activity;
import android.app.Dialog;
import android.os.Build;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;

import androidx.annotation.RequiresApi;

import java.util.function.Consumer;

/**
 * The page behind a dialog or a sheet, blurred by the system while that window is open (Nightjar #61). The dim stays as it
 * was and is all there is wherever the blur is not allowed.
 * <ul>
 *   <li>Never over a stream: a blur behind a window makes the system compose every stream frame on the GPU. What a stream's
 *   own menu and dialogs do is {@link StreamGlass}'s, not this class's.</li>
 *   <li>Otherwise by the rule of all live glass ({@link MenuBlurWindow#allowed}): Android 12 or later, a tier that affords
 *   live glass (so not on a low-memory device, in battery saver, when hot or in high contrast), and the system's own switch
 *   on. That switch is followed while the window is open.</li>
 * </ul>
 */
final class PageBlur {
    private final Window window;
    private final Activity owner;
    private final int radius;
    // The system's own answer is read again when it changes, as on every show: one rule, one place.
    private final Consumer<Boolean> listener = on -> refresh();
    private boolean listening;

    /** The rule. */
    static boolean allowed(boolean overStream, int api, GlassTier tier, boolean systemSwitch) {
        return !overStream && MenuBlurWindow.allowed(api, tier, systemSwitch);
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
        set(allowed(false, Build.VERSION.SDK_INT, GlassTier.land(owner), MenuBlurWindow.systemSwitch(owner)));
    }

    /** Whether the window asks for the blur right now. */
    boolean on() {
        return (window.getAttributes().flags & WindowManager.LayoutParams.FLAG_BLUR_BEHIND) != 0;
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private void set(boolean blurred) {
        WindowManager.LayoutParams params = window.getAttributes();
        int wanted = blurred ? radius : 0;
        if (on() == blurred && params.getBlurBehindRadius() == wanted) return;
        // The flag change hands the window its attributes, the radius among them.
        params.setBlurBehindRadius(wanted);
        if (blurred) window.addFlags(WindowManager.LayoutParams.FLAG_BLUR_BEHIND);
        else window.clearFlags(WindowManager.LayoutParams.FLAG_BLUR_BEHIND);
    }

    // The system's switch is heard only while the window is up: nothing of a closed dialog stays registered.
    private void follow(View decor) {
        decor.addOnAttachStateChangeListener(new View.OnAttachStateChangeListener() {
            @Override public void onViewAttachedToWindow(View view) {
                listen(true);
            }

            @Override public void onViewDetachedFromWindow(View view) {
                listen(false);
            }
        });
        if (decor.isAttachedToWindow()) listen(true);
    }

    private void listen(boolean on) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S || listening == on) return;
        listening = on;
        WindowManager manager = owner.getWindowManager();
        if (on) manager.addCrossWindowBlurEnabledListener(listener);
        else manager.removeCrossWindowBlurEnabledListener(listener);
    }
}
