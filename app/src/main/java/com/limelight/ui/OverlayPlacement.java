package com.limelight.ui;

import android.graphics.Rect;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.ViewTreeObserver;
import android.widget.FrameLayout;

import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;

import java.util.ArrayList;
import java.util.List;

/**
 * Keeps the performance overlay (and other small stream overlays) in a free corner of the stream's
 * safe area: inside the camera cutout and fold pane, and clear of on-screen controls when they show.
 */
public final class OverlayPlacement {
    public enum Spot { TOP_START, TOP_END, TOP_CENTER, BOTTOM_START, BOTTOM_END }

    /** Rectangles, in the overlay parent's coordinates, that the overlay must not cover. */
    public interface Obstacles {
        void collect(List<Rect> out);
    }

    private static final Spot[] PREFERENCE = {Spot.TOP_START, Spot.TOP_END, Spot.TOP_CENTER,
            Spot.BOTTOM_START, Spot.BOTTOM_END};

    private OverlayPlacement() {
    }

    /**
     * Top-start, where the overlay always was, unless something covers it there; then the first spot
     * that covers nothing, in the order top-end, top centre, bottom-start, bottom-end. When every spot
     * covers a control, the one covering the least.
     */
    static Spot choose(Rect area, int width, int height, List<Rect> obstacles, boolean rtl) {
        Spot best = Spot.TOP_START;
        long bestCovered = Long.MAX_VALUE;
        for (Spot spot : PREFERENCE) {
            Rect bounds = bounds(spot, area, width, height, rtl);
            long covered = 0;
            for (Rect obstacle : obstacles) {
                Rect overlap = new Rect();
                if (overlap.setIntersect(bounds, obstacle)) {
                    covered += (long) overlap.width() * overlap.height();
                }
            }
            if (covered == 0) return spot;
            if (covered < bestCovered) {
                best = spot;
                bestCovered = covered;
            }
        }
        return best;
    }

    static Rect bounds(Spot spot, Rect area, int width, int height, boolean rtl) {
        boolean left = (spot == Spot.TOP_START || spot == Spot.BOTTOM_START) != rtl;
        int x = spot == Spot.TOP_CENTER ? area.centerX() - width / 2 : left ? area.left : area.right - width;
        int y = spot == Spot.BOTTOM_START || spot == Spot.BOTTOM_END ? area.bottom - height : area.top;
        return new Rect(x, y, x + width, y + height);
    }

    /**
     * The parent's padded area less any camera cutout the padding leaves uncovered: a full-panel
     * stream has no padding there, so the overlay keeps its own distance from the cutout.
     */
    static Rect safeArea(View parent, Insets cutout, int margin) {
        return new Rect(Math.max(parent.getPaddingLeft(), cutout.left) + margin,
                Math.max(parent.getPaddingTop(), cutout.top) + margin,
                parent.getWidth() - Math.max(parent.getPaddingRight(), cutout.right) - margin,
                parent.getHeight() - Math.max(parent.getPaddingBottom(), cutout.bottom) - margin);
    }

    /**
     * Places {@code overlay}, a direct child of the FrameLayout {@code parent}, after every layout of the
     * window: rotation, a fold, the cutout, the overlay's own size and on-screen controls moving or
     * showing all change where it goes.
     */
    public static void attach(View overlay, ViewGroup parent, Obstacles obstacles) {
        int margin = Math.round(8 * parent.getResources().getDisplayMetrics().density);
        ViewTreeObserver.OnGlobalLayoutListener listener = () -> place(overlay, parent, obstacles, margin);
        parent.getViewTreeObserver().addOnGlobalLayoutListener(listener);
    }

    /** Places the overlay now; returns the spot it took. */
    public static Spot place(View overlay, ViewGroup parent, Obstacles obstacles, int margin) {
        if (overlay.getVisibility() != View.VISIBLE || parent.getWidth() == 0 || overlay.getWidth() == 0) {
            return null;
        }
        WindowInsetsCompat insets = ViewCompat.getRootWindowInsets(parent);
        Insets cutout = insets == null ? Insets.NONE : insets.getInsets(WindowInsetsCompat.Type.displayCutout());
        Rect area = safeArea(parent, cutout, margin);
        List<Rect> covered = new ArrayList<>();
        if (obstacles != null) obstacles.collect(covered);
        boolean rtl = ViewCompat.getLayoutDirection(parent) == ViewCompat.LAYOUT_DIRECTION_RTL;
        Spot spot = choose(area, overlay.getWidth(), overlay.getHeight(), covered, rtl);
        FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) overlay.getLayoutParams();
        int gravity = (spot == Spot.BOTTOM_START || spot == Spot.BOTTOM_END ? Gravity.BOTTOM : Gravity.TOP) |
                (spot == Spot.TOP_CENTER ? Gravity.CENTER_HORIZONTAL :
                        spot == Spot.TOP_START || spot == Spot.BOTTOM_START ? Gravity.START : Gravity.END);
        // FrameLayout already offsets children by its padding; the margins add the rest of the safe area.
        int left = area.left - parent.getPaddingLeft(), right = parent.getWidth() - parent.getPaddingRight() - area.right;
        int top = area.top - parent.getPaddingTop(), bottom = parent.getHeight() - parent.getPaddingBottom() - area.bottom;
        int start = rtl ? right : left, end = rtl ? left : right;
        if (spot == Spot.TOP_CENTER) start = end = 0;
        if (params.gravity != gravity || params.getMarginStart() != start || params.getMarginEnd() != end ||
                params.topMargin != top || params.bottomMargin != bottom) {
            params.gravity = gravity;
            params.setMarginStart(start);
            params.setMarginEnd(end);
            params.leftMargin = rtl ? end : start;
            params.rightMargin = rtl ? start : end;
            params.topMargin = top;
            params.bottomMargin = bottom;
            overlay.setLayoutParams(params);
        }
        return spot;
    }

    /** Adds the bounds of every visible child view, in {@code parent}'s coordinates. */
    public static void addVisible(View view, View parent, List<Rect> out) {
        if (view == null || view.getVisibility() != View.VISIBLE || view.getWidth() == 0) return;
        Rect rect = new Rect(0, 0, view.getWidth(), view.getHeight());
        if (parent instanceof ViewGroup && view.getParent() != null) {
            try {
                ((ViewGroup) parent).offsetDescendantRectToMyCoords(view, rect);
            } catch (IllegalArgumentException notADescendant) {
                return;
            }
        }
        out.add(rect);
    }
}
