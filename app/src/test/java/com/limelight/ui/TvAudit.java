package com.limelight.ui;

import android.content.res.Resources;
import android.graphics.Rect;
import android.util.TypedValue;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import com.limelight.R;

import java.util.ArrayList;
import java.util.List;

/**
 * What a TV screen must get right at ten feet: no text under the TV caption size, and nothing a viewer reads or focuses
 * in the overscan band a TV may cut off (5%: 48 x 27 dp at 960 x 540 dp). A view that runs off the screen edge is a
 * row scrolling past it, a peek by design, and only text that sits wholly on screen is held to the band.
 */
public final class TvAudit {
    /** The TV caption size: the smallest role in the TV type scale (values-television/styles.xml). */
    public static final float MIN_TEXT_SP = 14f;
    /** The 13 sp chip labels (CardChip, TvChip) sit on their own scrim or pill and are the one deliberate exception. */
    public static final float MIN_CHIP_SP = 13f;

    private TvAudit() {
    }

    /** Lays {@code screen} out at its display size, as the TV shows it, and returns its problems. */
    public static List<String> findings(android.app.Activity screen) {
        int width = screen.getResources().getDisplayMetrics().widthPixels;
        int height = screen.getResources().getDisplayMetrics().heightPixels;
        View root = screen.getWindow().getDecorView();
        root.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, width, height);
        return findings(root, width, height);
    }

    /**
     * A dialog's problems: the window centres it, so its own frame (as wide as it measures, no wider than the safe
     * area) stands for where it sits. Its text is held to the same sizes.
     */
    public static List<String> findings(android.app.Dialog dialog) {
        Resources resources = dialog.getContext().getResources();
        int width = resources.getDisplayMetrics().widthPixels;
        int height = resources.getDisplayMetrics().heightPixels;
        View root = dialog.getWindow().getDecorView();
        int safeWidth = width - 2 * resources.getDimensionPixelSize(R.dimen.shell_overscan_horizontal);
        int safeHeight = height - 2 * resources.getDimensionPixelSize(R.dimen.shell_overscan_vertical);
        root.measure(View.MeasureSpec.makeMeasureSpec(safeWidth, View.MeasureSpec.AT_MOST),
                View.MeasureSpec.makeMeasureSpec(safeHeight, View.MeasureSpec.AT_MOST));
        root.layout(0, 0, root.getMeasuredWidth(), root.getMeasuredHeight());
        Rect frame = new Rect(0, 0, root.getWidth(), root.getHeight());
        return audit(root, frame, frame);
    }

    /** Problems under {@code root}, as readable lines, for a screen {@code width} x {@code height} px. */
    public static List<String> findings(View root, int width, int height) {
        Resources resources = root.getResources();
        Rect safe = new Rect(resources.getDimensionPixelSize(R.dimen.shell_overscan_horizontal),
                resources.getDimensionPixelSize(R.dimen.shell_overscan_vertical),
                width - resources.getDimensionPixelSize(R.dimen.shell_overscan_horizontal),
                height - resources.getDimensionPixelSize(R.dimen.shell_overscan_vertical));
        return audit(root, new Rect(0, 0, width, height), safe);
    }

    private static List<String> audit(View root, Rect screen, Rect safe) {
        List<String> out = new ArrayList<>();
        int[] texts = new int[1];
        walk(root, screen, safe, out, texts);
        // A screen with no text on it was not laid out: an empty audit must not pass.
        if (texts[0] == 0) out.add("no text laid out on screen");
        return out;
    }

    private static void walk(View view, Rect screen, Rect safe, List<String> out, int[] texts) {
        if (view.getVisibility() != View.VISIBLE || view.getAlpha() == 0f) return;
        if (view instanceof ViewGroup && (view.canScrollHorizontally(1) || view.canScrollHorizontally(-1))) {
            // A row that scrolls sideways peeks past the keyline on purpose, and brings a card in when it takes focus.
            safe = new Rect(screen.left, safe.top, screen.right, safe.bottom);
        }
        if (view instanceof ViewGroup && (view.canScrollVertically(1) || view.canScrollVertically(-1))) {
            safe = new Rect(safe.left, screen.top, safe.right, screen.bottom);
        }
        Rect bounds = bounds(view);
        if (bounds.isEmpty() || !Rect.intersects(bounds, screen)) return;
        boolean onScreen = screen.contains(bounds);
        if (view instanceof TextView && ((TextView) view).getText().length() > 0) {
            TextView text = (TextView) view;
            texts[0]++;
            float sp = text.getTextSize() / scaledDensity(view.getResources());
            boolean chip = sp >= MIN_CHIP_SP - 0.05f && text.getBackground() != null;
            if (sp < MIN_TEXT_SP - 0.05f && !chip) {
                out.add(describe(view) + " text " + Math.round(sp * 10) / 10f + "sp");
            }
            if (onScreen && !safe.contains(bounds)) out.add(describe(view) + " text in the overscan band at " + bounds.toShortString());
        } else if (view.isFocusable() && onScreen && !fullScreen(bounds, screen) && !safe.contains(bounds)) {
            out.add(describe(view) + " focus stop in the overscan band at " + bounds.toShortString());
        }
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) walk(group.getChildAt(i), screen, safe, out, texts);
        }
    }

    /** A container as big as the screen (a scroller, a list) is not itself a stop a viewer reads. */
    private static boolean fullScreen(Rect bounds, Rect screen) {
        return bounds.width() >= screen.width() * 0.9f || bounds.height() >= screen.height() * 0.9f;
    }

    private static float scaledDensity(Resources resources) {
        return TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, 1f, resources.getDisplayMetrics());
    }

    private static Rect bounds(View view) {
        int[] at = new int[2];
        view.getLocationOnScreen(at);
        return new Rect(at[0], at[1], at[0] + view.getWidth(), at[1] + view.getHeight());
    }

    static String describe(View view) {
        String id = view.getId() == View.NO_ID ? "" : "#" + safeName(view);
        String text = view instanceof TextView ? " \"" + ((TextView) view).getText() + "\"" : "";
        if (text.length() > 40) text = text.substring(0, 38) + "...\"";
        return view.getClass().getSimpleName() + id + text;
    }

    private static String safeName(View view) {
        try {
            return view.getResources().getResourceEntryName(view.getId());
        } catch (Resources.NotFoundException e) {
            return Integer.toString(view.getId());
        }
    }
}
