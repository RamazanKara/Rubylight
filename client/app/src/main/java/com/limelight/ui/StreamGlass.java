package com.limelight.ui;

import android.app.Activity;
import android.app.Dialog;
import android.content.Context;
import android.content.ContextWrapper;
import android.graphics.Rect;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.InsetDrawable;
import android.util.TypedValue;
import android.view.View;
import android.view.Window;

import com.limelight.R;

/**
 * The one glass of everything over a stream: night in every theme, lit rim and sheen, and as see-through as what is behind
 * it lets the text stay readable.
 * <ul>
 *   <li>A modal surface (the stream menu, a dialog) stands on a system blur of the stream where the device's tier affords
 *   one ({@link MenuBlurWindow}), with a see-through fill, and on a near-opaque fill where it does not.</li>
 *   <li>What stays up while the game plays (the statistics, the keyboards) never has a blur under it: a blur would make
 *   the system compose every stream frame on the GPU. It is the HUD recipe, see-through, and its text stands on its own
 *   keys or carries a shadow.</li>
 * </ul>
 */
public final class StreamGlass {
    private StreamGlass() {
    }

    /** Whether this context draws over a stream: its theme says so. */
    public static boolean overStream(Context context) {
        TypedValue value = new TypedValue();
        return context.getTheme().resolveAttribute(R.attr.nightjarOverStream, value, true)
                && value.type == TypedValue.TYPE_INT_BOOLEAN && value.data != 0;
    }

    /** The HUD's glass behind the statistics, the menu hint and the stream's notices: all corners round. */
    public static void dressHud(View... views) {
        for (View view : views) {
            if (view == null) continue;
            float r = view.getResources().getDimension(R.dimen.stream_overlay_radius);
            view.setBackground(GlassPanelDrawable.hud(view.getContext(), new float[] {r, r, r, r, r, r, r, r}));
        }
    }

    /**
     * Shows a dialog over the stream with the stream menu's glass: over a blur of the stream, only behind the panel and only
     * while it is open, where this device can afford it, and near-opaque where it cannot. Other dialogs just show.
     */
    public static void show(Dialog dialog) {
        Window window = dialog.getWindow();
        GlassPanelDrawable glass = window != null ? glassOf(window.getDecorView().getBackground()) : null;
        Activity owner = activityOf(dialog.getContext());
        if (glass != null && owner != null && !dialog.isShowing()) {
            int radius = Math.round(GlassSpec.menuWindow(dialog.getContext()).blur);
            // Shown first, so it stands under the dialog: it blurs the stream, not the panel.
            MenuBlurWindow blur = MenuBlurWindow.openIfAllowed(owner, radius, glass::setOverBlur);
            glass.setOverBlur(blur != null);
            if (blur != null) follow(window.getDecorView(), window.getDecorView().getBackground(), blur);
        }
        dialog.show();
    }

    private static void follow(View decor, Drawable background, MenuBlurWindow blur) {
        int[] at = new int[2];
        Rect insets = new Rect();
        View.OnLayoutChangeListener place = (view, l, t, r, b, ol, ot, or, ob) -> {
            decor.getLocationOnScreen(at);
            // The panel is the window less the background's insets; its bounds are set only when it draws.
            background.getPadding(insets);
            blur.cover(at[0] + insets.left, at[1] + insets.top,
                    decor.getWidth() - insets.left - insets.right, decor.getHeight() - insets.top - insets.bottom);
        };
        decor.addOnLayoutChangeListener(place);
        decor.addOnAttachStateChangeListener(new View.OnAttachStateChangeListener() {
            @Override public void onViewAttachedToWindow(View view) {
            }

            @Override public void onViewDetachedFromWindow(View view) {
                blur.close();
                decor.removeOnLayoutChangeListener(place);
                decor.removeOnAttachStateChangeListener(this);
            }
        });
    }

    static GlassPanelDrawable glassOf(Drawable background) {
        while (background instanceof InsetDrawable) background = ((InsetDrawable) background).getDrawable();
        return background instanceof GlassPanelDrawable && ((GlassPanelDrawable) background).overStream()
                ? (GlassPanelDrawable) background : null;
    }

    static Activity activityOf(Context context) {
        while (context instanceof ContextWrapper) {
            if (context instanceof Activity) return (Activity) context;
            context = ((ContextWrapper) context).getBaseContext();
        }
        return null;
    }
}
