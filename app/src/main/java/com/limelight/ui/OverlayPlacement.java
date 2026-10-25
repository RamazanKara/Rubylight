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
import java.util.function.Supplier;

/**
 * Keeps the performance overlay (and other small stream overlays) in a free corner of the stream's
 * safe area: inside the camera cutout and fold pane, and clear of on-screen controls when they show.
 */
public final class OverlayPlacement {
    public enum Spot { TOP_START, TOP_END, TOP_CENTER, BOTTOM_START, BOTTOM_END }

    /** The corner the user wants the overlay in, as the screen shows it: left stays left in a right-to-left layout. */
    public enum Corner {
        TOP_LEFT("top_left"), TOP_RIGHT("top_right"), BOTTOM_LEFT("bottom_left"), BOTTOM_RIGHT("bottom_right");

        /** The value stored under {@code performance_overlay_position}. */
        public final String key;

        Corner(String key) {
            this.key = key;
        }

        /** The stored value's corner; a missing or unknown one is top left, where the overlay always was. */
        public static Corner parse(String key) {
            for (Corner corner : values()) {
                if (corner.key.equals(key)) return corner;
            }
            return TOP_LEFT;
        }

        Spot spot(boolean rtl) {
            boolean top = this == TOP_LEFT || this == TOP_RIGHT;
            boolean start = (this == TOP_LEFT || this == BOTTOM_LEFT) != rtl;
            return top ? (start ? Spot.TOP_START : Spot.TOP_END) : (start ? Spot.BOTTOM_START : Spot.BOTTOM_END);
        }
    }

    /** Rectangles, in the overlay parent's coordinates, that the overlay must not cover. */
    public interface Obstacles {
        void collect(List<Rect> out);
    }

    private static final Spot[] PREFERENCE = {Spot.TOP_START, Spot.TOP_END, Spot.TOP_CENTER,
            Spot.BOTTOM_START, Spot.BOTTOM_END};

    private OverlayPlacement() {
    }

    /** Top-start, where the overlay always was, unless something covers it there. */
    static Spot choose(Rect area, int width, int height, List<Rect> obstacles, boolean rtl) {
        return choose(area, width, height, obstacles, Spot.TOP_START, rtl);
    }

    /**
     * The preferred spot unless something covers it there; then the first spot that covers nothing, in the
     * order top-start, top-end, top centre, bottom-start, bottom-end. When every spot covers a control, the
     * one covering the least (the preferred spot on a tie).
     */
    static Spot choose(Rect area, int width, int height, List<Rect> obstacles, Spot preferred, boolean rtl) {
        Spot best = preferred;
        long bestCovered = Long.MAX_VALUE;
        for (int i = -1; i < PREFERENCE.length; i++) {
            Spot spot = i < 0 ? preferred : PREFERENCE[i];
            if (i >= 0 && spot == preferred) continue;
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

    /** Called after each placement with the spot taken, so the overlay can align its text to that side. */
    public interface OnPlaced {
        void placed(Spot spot);
    }

    /**
     * Places {@code overlay}, a direct child of the FrameLayout {@code parent}, after every layout of the
     * window: rotation, a fold, the cutout, the overlay's own size and on-screen controls moving or
     * showing all change where it goes. {@code corner} is read on each placement, so a new choice only
     * needs a layout; {@code onPlaced} may be null.
     */
    public static void attach(View overlay, ViewGroup parent, Obstacles obstacles, Supplier<Corner> corner, OnPlaced onPlaced) {
        int margin = Math.round(8 * parent.getResources().getDisplayMetrics().density);
        ViewTreeObserver.OnGlobalLayoutListener listener = () -> {
            Spot spot = place(overlay, parent, obstacles, margin, corner.get());
            if (spot != null && onPlaced != null) onPlaced.placed(spot);
        };
        parent.getViewTreeObserver().addOnGlobalLayoutListener(listener);
    }

    public static void attach(View overlay, ViewGroup parent, Obstacles obstacles) {
        attach(overlay, parent, obstacles, () -> Corner.TOP_LEFT, null);
    }

    /** Places the overlay now, top left unless something covers it; returns the spot it took. */
    public static Spot place(View overlay, ViewGroup parent, Obstacles obstacles, int margin) {
        return place(overlay, parent, obstacles, margin, Corner.TOP_LEFT);
    }

    /** Places the overlay now, in {@code corner} unless something covers it; returns the spot it took. */
    public static Spot place(View overlay, ViewGroup parent, Obstacles obstacles, int margin, Corner corner) {
        if (overlay.getVisibility() != View.VISIBLE || parent.getWidth() == 0 || overlay.getWidth() == 0) {
            return null;
        }
        WindowInsetsCompat insets = ViewCompat.getRootWindowInsets(parent);
        Insets cutout = insets == null ? Insets.NONE : insets.getInsets(WindowInsetsCompat.Type.displayCutout());
        Rect area = safeArea(parent, cutout, margin);
        List<Rect> covered = new ArrayList<>();
        if (obstacles != null) obstacles.collect(covered);
        boolean rtl = ViewCompat.getLayoutDirection(parent) == ViewCompat.LAYOUT_DIRECTION_RTL;
        Spot spot = choose(area, overlay.getWidth(), overlay.getHeight(), covered, corner.spot(rtl), rtl);
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
