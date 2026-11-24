package com.limelight.ui;

import android.content.Context;
import android.view.Gravity;
import android.widget.LinearLayout;

import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.widget.NestedScrollView;

import com.google.android.material.button.MaterialButton;

/**
 * Custom layout and scroll views backing {@link ActionSheet}: the adaptive toggle grid row, floating
 * panel frame, capsule footer bar, and navigation-sound scroll view.
 */
final class ActionSheetLayoutViews {
    private ActionSheetLayoutViews() { }

    /**
     * Equal toggles side by side while every word of their labels fits its share of the width. When they
     * do not, as four toggles on a phone held upright, they form two columns of tiles; with a large font
     * or a very narrow sheet they stack, one full-width row each with the icon at the start, so no label
     * breaks mid-word.
     */
    static class ToggleRowBase extends LinearLayout {
        private final int gap;
        private final int tileHeight;
        private int columns;

        ToggleRowBase(Context context, int gap) {
            this(context, gap, gap * 9);
        }

        ToggleRowBase(Context context, int gap, int tileHeight) {
            super(context);
            this.gap = gap;
            this.tileHeight = tileHeight;
            // Equal buttons in a straight line, whatever the size of their icons or text.
            setBaselineAligned(false);
        }

        /** How many toggles share a line: all of them, two, or one. */
        int columns() {
            return columns;
        }

        @Override
        protected void onMeasure(int widthSpec, int heightSpec) {
            int count = getChildCount();
            int width = MeasureSpec.getSize(widthSpec) - getPaddingLeft() - getPaddingRight();
            boolean bounded = MeasureSpec.getMode(widthSpec) != MeasureSpec.UNSPECIFIED;
            if (count == 0 || !bounded || wordsFit((width - gap * (count - 1)) / count)) columns = Math.max(count, 1);
            else if (count > 2 && wordsFit((width - gap) / 2)) columns = 2;
            else columns = 1;
            boolean stacked = columns == 1 && count > 1;
            setOrientation(stacked ? VERTICAL : HORIZONTAL);
            for (int i = 0; i < count; i++) {
                MaterialButton button = (MaterialButton) getChildAt(i);
                LayoutParams params = (LayoutParams) button.getLayoutParams();
                params.width = stacked ? LayoutParams.MATCH_PARENT : 0;
                params.height = stacked ? LayoutParams.WRAP_CONTENT : LayoutParams.MATCH_PARENT;
                params.weight = stacked ? 0 : 1;
                params.setMarginStart(stacked || i % columns == 0 ? 0 : gap);
                params.resolveLayoutDirection(getLayoutDirection());
                params.topMargin = i >= columns ? gap : 0;
                int iconGravity = stacked ? MaterialButton.ICON_GRAVITY_TEXT_START : MaterialButton.ICON_GRAVITY_TOP;
                if (button.getIconGravity() != iconGravity) button.setIconGravity(iconGravity);
                // A stacked toggle has its icon beside the label, so it needs only a list row's height.
                int minHeight = stacked ? gap * 6 : tileHeight;
                if (button.getMinHeight() != minHeight) button.setMinHeight(minHeight);
            }
            if (columns == count || stacked) {
                super.onMeasure(widthSpec, heightSpec);
                return;
            }
            // Tiles: equal columns, and each line as tall as its tallest tile so the grid stays even.
            int cell = (width - gap * (columns - 1)) / columns;
            int height = getPaddingTop() + getPaddingBottom();
            for (int line = 0; line * columns < count; line++) {
                int tallest = 0;
                for (int i = line * columns; i < Math.min(count, (line + 1) * columns); i++) {
                    android.view.View child = getChildAt(i);
                    child.measure(MeasureSpec.makeMeasureSpec(cell, MeasureSpec.EXACTLY),
                            MeasureSpec.makeMeasureSpec(0, MeasureSpec.UNSPECIFIED));
                    tallest = Math.max(tallest, child.getMeasuredHeight());
                }
                for (int i = line * columns; i < Math.min(count, (line + 1) * columns); i++) {
                    getChildAt(i).measure(MeasureSpec.makeMeasureSpec(cell, MeasureSpec.EXACTLY),
                            MeasureSpec.makeMeasureSpec(tallest, MeasureSpec.EXACTLY));
                }
                height += tallest + (line > 0 ? gap : 0);
            }
            setMeasuredDimension(MeasureSpec.getSize(widthSpec), resolveSize(height, heightSpec));
        }

        @Override
        protected void onLayout(boolean changed, int left, int top, int right, int bottom) {
            int count = getChildCount();
            if (columns == count || columns == 1) {
                super.onLayout(changed, left, top, right, bottom);
                return;
            }
            boolean rtl = getLayoutDirection() == LAYOUT_DIRECTION_RTL;
            int y = getPaddingTop();
            for (int line = 0; line * columns < count; line++) {
                int tallest = 0;
                for (int i = line * columns; i < Math.min(count, (line + 1) * columns); i++) {
                    android.view.View child = getChildAt(i);
                    int column = i % columns;
                    int x = column * (child.getMeasuredWidth() + gap);
                    int childLeft = rtl ? right - left - getPaddingRight() - x - child.getMeasuredWidth()
                            : getPaddingLeft() + x;
                    child.layout(childLeft, y, childLeft + child.getMeasuredWidth(), y + child.getMeasuredHeight());
                    tallest = Math.max(tallest, child.getMeasuredHeight());
                }
                y += tallest + gap;
            }
        }

        private boolean wordsFit(int buttonWidth) {
            for (int i = 0; i < getChildCount(); i++) {
                MaterialButton button = (MaterialButton) getChildAt(i);
                int room = buttonWidth - button.getPaddingLeft() - button.getPaddingRight();
                for (String word : button.getText().toString().split("\\s+")) {
                    if (button.getPaint().measureText(word) > room) return false;
                }
            }
            return true;
        }
    }

    /**
     * A panel's window: the panel docked to the end edge in landscape and centred in portrait, the stream around it,
     * and a tap beside the panel closing it. The panel is the design's 452 dp where there is room; in landscape it takes at
     * most two thirds of the width but never less than 360 dp.
     */
    static class PanelFrameBase extends android.widget.FrameLayout {
        static final int WIDTH_DP = 452, MIN_WIDTH_DP = 360;
        static final float LANDSCAPE_SHARE = 0.66f;
        private final android.view.View panel;
        private final int margin;
        private Runnable onOutsideTap;
        private boolean pressedOutside;
        private android.view.View anchor;

        PanelFrameBase(Context context, android.view.View panel, int margin) {
            super(context);
            this.panel = panel;
            this.margin = margin;
            addView(panel, new LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT));
            // Over an immersive stream only a display cutout is left to keep clear of; elsewhere the bars too.
            ViewCompat.setOnApplyWindowInsetsListener(this, (view, insets) -> {
                androidx.core.graphics.Insets bars = insets.getInsets(
                        WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout());
                view.setPadding(bars.left, bars.top, bars.right, bars.bottom);
                return insets;
            });
        }

        void setOnOutsideTap(Runnable onOutsideTap) {
            this.onOutsideTap = onOutsideTap;
        }

        void setAnchor(android.view.View anchor) {
            this.anchor = anchor;
        }

        /**
         * Under an anchor: {@link #MIN_WIDTH_DP} wide, 8 dp below it, its end on the anchor's end (start in RTL), kept
         * {@code edge} inside the window. Positions are on screen; the result is the panel's left and top in this frame.
         */
        static int[] underAnchor(int[] anchorAt, int anchorWidth, int anchorHeight, int[] frameAt, int frameWidth,
                                 int panelWidth, int edge, float density, boolean rtl) {
            int start = rtl ? anchorAt[0] - frameAt[0] : anchorAt[0] - frameAt[0] + anchorWidth - panelWidth;
            int left = Math.max(edge, Math.min(start, frameWidth - panelWidth - edge));
            return new int[] {left, anchorAt[1] - frameAt[1] + anchorHeight + Math.round(8 * density)};
        }

        /** The panel's width in pixels in a window {@code width} × {@code height} pixels big. */
        static int panelWidth(int width, int height, int margin, float density) {
            int wide = Math.round(WIDTH_DP * density);
            int target = width > height
                    ? Math.max(Math.round(MIN_WIDTH_DP * density), Math.min(wide, Math.round(width * LANDSCAPE_SHARE)))
                    : wide;
            return Math.max(0, Math.min(target, width - 2 * margin));
        }

        @Override
        protected void onMeasure(int widthSpec, int heightSpec) {
            int width = MeasureSpec.getSize(widthSpec) - getPaddingLeft() - getPaddingRight();
            int height = MeasureSpec.getSize(heightSpec) - getPaddingTop() - getPaddingBottom();
            LayoutParams params = (LayoutParams) panel.getLayoutParams();
            float density = getResources().getDisplayMetrics().density;
            if (anchor != null && anchor.isAttachedToWindow()) {
                params.width = Math.max(0, Math.min(Math.round(MIN_WIDTH_DP * density), width - 2 * margin));
                params.gravity = Gravity.TOP | Gravity.LEFT;
                int[] anchorAt = new int[2];
                int[] frameAt = new int[2];
                anchor.getLocationOnScreen(anchorAt);
                getLocationOnScreen(frameAt);
                frameAt[0] += getPaddingLeft();
                frameAt[1] += getPaddingTop();
                int[] at = underAnchor(anchorAt, anchor.getWidth(), anchor.getHeight(), frameAt, width, params.width, margin,
                        density, getLayoutDirection() == LAYOUT_DIRECTION_RTL);
                params.setMargins(at[0], at[1], 0, margin);
            } else {
                params.width = panelWidth(width, height, margin, density);
                params.gravity = width > height ? Gravity.END | Gravity.CENTER_VERTICAL : Gravity.CENTER;
                params.setMargins(margin, margin, margin, margin);
            }
            super.onMeasure(widthSpec, heightSpec);
        }

        private boolean outside(android.view.MotionEvent event) {
            return event.getX() < panel.getLeft() || event.getX() >= panel.getRight()
                    || event.getY() < panel.getTop() || event.getY() >= panel.getBottom();
        }

        // Touches the panel's rows leave fall through to here: beside the panel they close it, on it they do nothing.
        @Override
        public boolean onTouchEvent(android.view.MotionEvent event) {
            switch (event.getActionMasked()) {
                case android.view.MotionEvent.ACTION_DOWN:
                    pressedOutside = outside(event);
                    break;
                case android.view.MotionEvent.ACTION_UP:
                    if (pressedOutside && outside(event)) performClick();
                    pressedOutside = false;
                    break;
                case android.view.MotionEvent.ACTION_CANCEL:
                    pressedOutside = false;
                    break;
                default:
                    break;
            }
            return true;
        }

        @Override
        public boolean performClick() {
            super.performClick();
            if (onOutsideTap != null) onOutsideTap.run();
            return true;
        }
    }

    /**
     * A panel's footer: the ways out side by side in capsules of equal width. When a label would not fit its share, the last one,
     * the red one, takes a line of its own.
     */
    static class FooterBarBase extends android.view.ViewGroup {
        private final int gap;
        private boolean split;

        FooterBarBase(Context context, int gap) {
            super(context);
            this.gap = gap;
        }

        /** True while the last capsule has its own line. */
        boolean split() {
            return split;
        }

        @Override
        protected void onMeasure(int widthSpec, int heightSpec) {
            int width = MeasureSpec.getSize(widthSpec);
            int room = width - getPaddingLeft() - getPaddingRight();
            int count = getChildCount();
            if (count == 0) {
                split = false;
                setMeasuredDimension(width, 0);
                return;
            }
            int cell = (room - gap * (count - 1)) / count;
            int widest = 0;
            for (int i = 0; i < count; i++) {
                android.view.View child = getChildAt(i);
                child.measure(MeasureSpec.makeMeasureSpec(0, MeasureSpec.UNSPECIFIED), MeasureSpec.makeMeasureSpec(0, MeasureSpec.UNSPECIFIED));
                widest = Math.max(widest, child.getMeasuredWidth());
            }
            split = count > 1 && widest > cell;
            int height = getPaddingTop() + getPaddingBottom();
            if (split) {
                height += measureLine(0, count - 1, (room - gap * (count - 2)) / (count - 1)) + gap;
                height += measureLine(count - 1, count, room);
            } else {
                height += measureLine(0, count, cell);
            }
            setMeasuredDimension(width, height);
        }

        // Children [from, to) on one line, each as tall as the tallest; answers the line's height.
        private int measureLine(int from, int to, int cell) {
            int tallest = 0;
            for (int i = from; i < to; i++) {
                android.view.View child = getChildAt(i);
                child.measure(MeasureSpec.makeMeasureSpec(cell, MeasureSpec.EXACTLY),
                        MeasureSpec.makeMeasureSpec(0, MeasureSpec.UNSPECIFIED));
                tallest = Math.max(tallest, child.getMeasuredHeight());
            }
            for (int i = from; i < to; i++) {
                android.view.View child = getChildAt(i);
                child.measure(MeasureSpec.makeMeasureSpec(child.getMeasuredWidth(), MeasureSpec.EXACTLY),
                        MeasureSpec.makeMeasureSpec(tallest, MeasureSpec.EXACTLY));
            }
            return tallest;
        }

        @Override
        protected void onLayout(boolean changed, int left, int top, int right, int bottom) {
            boolean rtl = getLayoutDirection() == LAYOUT_DIRECTION_RTL;
            int width = right - left;
            int y = getPaddingTop();
            int x = 0;
            int count = getChildCount();
            for (int i = 0; i < count; i++) {
                android.view.View child = getChildAt(i);
                if (split && i == count - 1) {
                    y += getChildAt(0).getMeasuredHeight() + gap;
                    x = 0;
                }
                int childLeft = rtl ? width - getPaddingRight() - x - child.getMeasuredWidth() : getPaddingLeft() + x;
                child.layout(childLeft, y, childLeft + child.getMeasuredWidth(), y + child.getMeasuredHeight());
                x += child.getMeasuredWidth() + gap;
            }
        }
    }

    /**
     * A sheet's scroll that leaves a focused row alone while it is wholly in view. The platform's scroll also keeps a focused
     * row clear of the fading edge, so focus reaching the stream menu's Bitrate slid the tiles under the head for a few
     * pixels' fade although everything fitted.
     */
    static class StillScrollBase extends NestedScrollView {
        StillScrollBase(Context context) {
            super(context);
        }

        @Override
        protected int computeScrollDeltaToGetChildRectOnScreen(android.graphics.Rect rect) {
            int top = getScrollY();
            if (rect.top >= top && rect.bottom <= top + getHeight()) return 0;
            return super.computeScrollDeltaToGetChildRectOnScreen(rect);
        }

        /**
         * Up and Down inside a sheet that scrolls are this scroll's own move: it takes the key before the window does, so
         * the window's navigation sound, which Left and Right still get, never played for them (nightjar#7). A move that
         * lands focus on another row plays it here; a press that only scrolls stays silent, as the window's would, also
         * when it scrolls the focused row away and the scroll itself takes the focus.
         */
        @Override
        public boolean arrowScroll(int direction) {
            android.view.View before = findFocus();
            boolean handled = super.arrowScroll(direction);
            android.view.View after = findFocus();
            if (handled && after != null && after != before && inside(after)) {
                after.playSoundEffect(android.view.SoundEffectConstants.getContantForFocusDirection(direction));
            }
            return handled;
        }

        /** Whether {@code view} is one of this scroll's rows or within one, not the scroll itself. */
        private boolean inside(android.view.View view) {
            for (android.view.ViewParent parent = view.getParent(); parent instanceof android.view.View; parent = parent.getParent()) {
                if (parent == this) return true;
            }
            return false;
        }
    }
}
