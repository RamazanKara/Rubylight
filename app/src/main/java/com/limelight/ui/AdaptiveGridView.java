package com.limelight.ui;

import android.animation.Animator;
import android.animation.AnimatorListenerAdapter;
import android.animation.ValueAnimator;
import android.content.Context;
import android.content.res.Configuration;
import android.os.SystemClock;
import android.util.AttributeSet;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewTreeObserver;
import android.view.animation.AnimationUtils;
import android.widget.GridView;

import com.limelight.R;

/**
 * A grid that picks its column count from the space it is measured with.
 *
 * <p>The count used to be set from a layout listener. GridView.setNumColumns() removes every child
 * and requests a layout, and a layout requested from inside a layout pass is dropped, so a rotation
 * that changed the count left the library blank until something else relaid it out (for example a
 * search query). Measuring is the one place where changing the count is followed by a layout of the
 * new children.
 */
public class AdaptiveGridView extends GridView {
    /** Below this height, cards switch to their compact artwork so a whole row stays visible. */
    private static final int SHORT_VIEWPORT_DP = 400;

    /** Room kept around the selected card for its focus scale and outline, in dp. */
    private static final int FOCUS_MARGIN_DP = 8;
    /** A TV poster's focus scale when it is focused (tv_poster_focus, tv_poster_focus_small). Its glow is inside its own padding. */
    private static final float POSTER_SCALE = 1.08f;
    private static final float POSTER_SCALE_SMALL = 1.10f;

    private boolean policySet;
    private boolean revealPending;
    private final Runnable revealSelection = this::revealSelection;
    private boolean computers;
    private boolean smallIcons;
    /** Tells a held key from a press, so a long run across the grid does not queue a focus tween per card. */
    private final FastTravel travel = new FastTravel();

    /** A row change glides: the content jumps as the list lays it out, then every card eases from its old place. */
    private static final long GLIDE_MS = 180;
    /** How long after a key its scrolling counts as the key's: past it a scroll is a touch fling, which never glides. */
    private static final long GLIDE_WINDOW_MS = 250;
    private static final int NO_OFFSET = Integer.MIN_VALUE;
    private int contentY = NO_OFFSET;
    private long glideUntil;
    private float glideOffset;
    private ValueAnimator glide;
    private int glideCount;

    /** The first cards of a screen rise in, staggered; a key or a touch ends it. */
    private static final int ENTER_CARDS = 10;
    private static final long ENTER_MS = 220;
    private static final long ENTER_STAGGER_MS = 18;
    private static final float ENTER_RISE_DP = 24;
    private java.util.List<View> entering;
    private final Runnable enterDone = () -> entering = null;

    public AdaptiveGridView(Context context) {
        super(context);
    }

    public AdaptiveGridView(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    public AdaptiveGridView(Context context, AttributeSet attrs, int defStyleAttr) {
        super(context, attrs, defStyleAttr);
    }

    /** Columns follow {@link AdaptiveLayout#gridColumns} for PC cards or app cards. */
    public void setColumnPolicy(boolean computers, boolean smallIcons) {
        if (policySet && this.computers == computers && this.smallIcons == smallIcons) return;
        policySet = true;
        this.computers = computers;
        this.smallIcons = smallIcons;
        requestLayout();
    }

    /**
     * D-pad and keyboard focus stays on the grid, which moves a selection over its cards. A card's
     * overflow button cannot take focus without breaking taps on the card, so the grid steps onto it,
     * but only where the end direction (right in left-to-right layouts) has no further card: at the
     * end of a row or on the last card. Elsewhere that direction moves card to card in one press;
     * a long press of the select button, Menu and Shift+F10 open any card's options directly. On a marked
     * button, Enter or the A button opens it and the opposite direction returns to the card.
     */
    private View markedAction;
    private View endTarget;

    /** The view the end direction moves to from the last card, such as the button that adds an item. */
    public void setEndFocusTarget(View target) {
        endTarget = target;
    }

    @Override
    public boolean onKeyDown(int keyCode, KeyEvent event) {
        if (event.hasNoModifiers()) {
            if (markedAction != null && isBackKey(keyCode)) return true;
            View action = selectedAction();
            int towardAction = getLayoutDirection() == LAYOUT_DIRECTION_RTL ? KeyEvent.KEYCODE_DPAD_LEFT : KeyEvent.KEYCODE_DPAD_RIGHT;
            int towardCard = towardAction == KeyEvent.KEYCODE_DPAD_RIGHT ? KeyEvent.KEYCODE_DPAD_LEFT : KeyEvent.KEYCODE_DPAD_RIGHT;
            if (markedAction == null && action != null && keyCode == towardAction && atRowEnd()) {
                markAction(action);
                return true;
            }
            // Past the last card (and its overflow button) the end direction reaches the screen's own button,
            // which floats outside the grid and has no other way in from the cards.
            if (endTarget != null && endTarget.getVisibility() == VISIBLE && keyCode == towardAction &&
                    getSelectedItemPosition() == getCount() - 1 && (action == null || markedAction != null)) {
                markAction(null);
                endTarget.requestFocus();
                return true;
            }
            if (markedAction != null && markedAction == action) {
                if (keyCode == towardCard) {
                    markAction(null);
                    return true;
                }
                if (isActivateKey(keyCode)) return true;
            }
        }
        markAction(null);
        endEnter();
        revealPending = movesSelection(keyCode);
        if (revealPending) {
            long now = SystemClock.uptimeMillis();
            travel.onMove(event.getRepeatCount(), now);
            contentY = contentY();
            glideUntil = now + GLIDE_WINDOW_MS;
        }
        boolean handled = super.onKeyDown(keyCode, event);
        // The list may already have scrolled inside the key, or does so in the layout that follows.
        if (revealPending) noteScroll();
        return handled;
    }

    /** True while a key is held or presses follow each other quickly: screens skip their tweens then. */
    public boolean isFastTravel() {
        return travel.isActive(SystemClock.uptimeMillis());
    }

    /** Where the first row's top is in the whole list, so two readings differ by exactly what the list scrolled. */
    private int contentY() {
        if (getChildCount() == 0 || getNumColumns() <= 0) return NO_OFFSET;
        View first = getChildAt(0);
        int row = getFirstVisiblePosition() / getNumColumns();
        return row * (first.getHeight() + getVerticalSpacing()) - first.getTop();
    }

    /** Takes the list's scroll since the last reading and, when a key caused it, starts the glide over it. */
    private void noteScroll() {
        int before = contentY;
        contentY = contentY();
        if (before == NO_OFFSET || contentY == NO_OFFSET || contentY == before) return;
        if (SystemClock.uptimeMillis() > glideUntil) return;
        int delta = contentY - before;
        if (computers || !isTelevision() || Math.abs(delta) > getHeight() || travel.isActive(SystemClock.uptimeMillis())
                || Motion.reduced(getContext())) {
            endGlide();
            return;
        }
        // The cards moved up by delta, so each starts that far below where it now is.
        glideOffset += delta;
        applyGlide();
        if (glide != null) {
            ValueAnimator previous = glide;
            glide = null;
            previous.cancel();
        }
        ValueAnimator animator = ValueAnimator.ofFloat(glideOffset, 0f);
        animator.setDuration(GLIDE_MS);
        animator.setInterpolator(AnimationUtils.loadInterpolator(getContext(), R.interpolator.tv_std));
        animator.addUpdateListener(animation -> {
            glideOffset = (float) animation.getAnimatedValue();
            applyGlide();
        });
        animator.addListener(new AnimatorListenerAdapter() {
            @Override public void onAnimationEnd(Animator animation) {
                if (glide == animation) {
                    glide = null;
                    glideOffset = 0;
                    applyGlide();
                }
            }
        });
        glide = animator;
        glideCount++;
        animator.start();
    }

    /** How many glides this grid has started; tests read it because a paused test looper does not tick animators. */
    public int glideCount() {
        return glideCount;
    }

    private void applyGlide() {
        for (int i = 0; i < getChildCount(); i++) getChildAt(i).setTranslationY(glideOffset);
    }

    private void endGlide() {
        ValueAnimator running = glide;
        glide = null;
        if (running != null) running.cancel();
        if (glideOffset != 0) {
            glideOffset = 0;
            applyGlide();
        }
    }

    /**
     * Brings a screen's first cards in: up to ten of the visible ones rise 24 dp and fade in, a diagonal wave from the
     * first. Meant for the first time a screen fills, once its list has been handed to the grid. Nothing with Reduce motion.
     */
    public void playEnter() {
        if (computers || !isTelevision() || Motion.reduced(getContext())) return;
        getViewTreeObserver().addOnPreDrawListener(new ViewTreeObserver.OnPreDrawListener() {
            @Override public boolean onPreDraw() {
                getViewTreeObserver().removeOnPreDrawListener(this);
                startEnter();
                return true;
            }
        });
    }

    private void startEnter() {
        endEnter();
        int count = Math.min(getChildCount(), ENTER_CARDS);
        if (count == 0 || !isAttachedToWindow()) return;
        int columns = Math.max(1, getNumColumns());
        float rise = ENTER_RISE_DP * getResources().getDisplayMetrics().density;
        android.view.animation.Interpolator settle = AnimationUtils.loadInterpolator(getContext(), R.interpolator.tv_emph_dec);
        java.util.List<View> cards = new java.util.ArrayList<>(count);
        long longest = 0;
        for (int i = 0; i < count; i++) {
            View card = getChildAt(i);
            long delay = ENTER_STAGGER_MS * (i / columns + i % columns);
            card.setAlpha(0f);
            card.setTranslationY(rise);
            card.animate().alpha(1f).translationY(0f).setStartDelay(delay).setDuration(ENTER_MS).setInterpolator(settle);
            cards.add(card);
            longest = Math.max(longest, delay);
        }
        entering = cards;
        postDelayed(enterDone, longest + ENTER_MS + 50);
    }

    /** Puts the rising cards where they are going, at once. */
    private void endEnter() {
        java.util.List<View> cards = entering;
        if (cards == null) return;
        entering = null;
        removeCallbacks(enterDone);
        for (View card : cards) {
            card.animate().cancel();
            card.setAlpha(1f);
            card.setTranslationY(glideOffset);
        }
    }

    @Override
    protected void onDetachedFromWindow() {
        endEnter();
        endGlide();
        super.onDetachedFromWindow();
    }

    /**
     * After a key moved the selection, the card is shown whole with room for its focus scale and outline.
     * The list's own scrolling only keeps the card inside the grid's padding, which a card that is taller than
     * that padding, or one that scales up, can still spill out of.
     */
    private void revealSelection() {
        revealPending = false;
        View card = getSelectedView();
        if (card == null || !isAttachedToWindow()) return;
        int margin = focusMarginPx(card);
        int top = card.getTop() - margin;
        int bottom = card.getBottom() + margin;
        int dy = 0;
        if (bottom - top > getHeight() || top < 0) dy = top;
        else if (bottom > getHeight()) dy = bottom - getHeight();
        if (dy != 0) {
            scrollListBy(dy);
            noteScroll();
        }
    }

    /**
     * Room around the selected card. A TV poster grows by its focus scale around its centre, and that growth is
     * what can spill out of the grid, so the room is derived from the card's height; its glow is inside its own padding.
     */
    private int focusMarginPx(View card) {
        int base = Math.round(FOCUS_MARGIN_DP * getResources().getDisplayMetrics().density);
        if (computers || !isTelevision()) return base;
        float scale = smallIcons ? POSTER_SCALE_SMALL : POSTER_SCALE;
        return Math.max(base, (int) Math.ceil((scale - 1f) * card.getHeight() / 2f));
    }

    private boolean isTelevision() {
        return (getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK)
                == Configuration.UI_MODE_TYPE_TELEVISION;
    }

    @Override
    public boolean onKeyUp(int keyCode, KeyEvent event) {
        if (markedAction != null && event.hasNoModifiers() && isBackKey(keyCode)) {
            if (!event.isCanceled()) markAction(null);
            return true;
        }
        if (markedAction != null && isActivateKey(keyCode) && markedAction == selectedAction()) {
            if (!event.isCanceled()) markedAction.performClick();
            return true;
        }
        return super.onKeyUp(keyCode, event);
    }

    private boolean secondaryGesture;

    /**
     * A mouse's right button opens the item's options, as a long press does, and the rest of that
     * click is swallowed so it does not also open the item. A context-click listener on the cards
     * would make them clickable and take touches away from the grid.
     */
    @Override
    public boolean dispatchTouchEvent(android.view.MotionEvent event) {
        int action = event.getActionMasked();
        if (action == android.view.MotionEvent.ACTION_DOWN) {
            endEnter();
            secondaryGesture = false;
            OnItemLongClickListener listener = getOnItemLongClickListener();
            int position = pointToPosition((int) event.getX(), (int) event.getY());
            if (listener != null && position != INVALID_POSITION && event.isFromSource(android.view.InputDevice.SOURCE_MOUSE)
                    && event.isButtonPressed(android.view.MotionEvent.BUTTON_SECONDARY)) {
                secondaryGesture = true;
                listener.onItemLongClick(this, getChildAt(position - getFirstVisiblePosition()), position,
                        getItemIdAtPosition(position));
                return true;
            }
        } else if (secondaryGesture) {
            if (action == android.view.MotionEvent.ACTION_UP || action == android.view.MotionEvent.ACTION_CANCEL) {
                secondaryGesture = false;
            }
            return true;
        }
        return super.dispatchTouchEvent(event);
    }

    @Override
    protected void onFocusChanged(boolean gainFocus, int direction, android.graphics.Rect previouslyFocusedRect) {
        super.onFocusChanged(gainFocus, direction, previouslyFocusedRect);
        if (!gainFocus) markAction(null);
    }

    @Override
    protected void layoutChildren() {
        View action = markedAction;
        super.layoutChildren();
        // A rebuilt or recycled card no longer holds the marked button.
        if (action != null && action != selectedAction()) markAction(null);
        if (travel.isActive(SystemClock.uptimeMillis())) {
            // A held key puts every card straight into the state it is in: the new one focused, the old one at rest.
            for (int i = 0; i < getChildCount(); i++) FastTravel.settle(getChildAt(i));
        }
        if (glideOffset != 0) applyGlide();
        if (revealPending) {
            noteScroll();
            removeCallbacks(revealSelection);
            post(revealSelection);
        } else {
            contentY = contentY();
        }
    }

    /** The overflow button of the selected card, if it shows one. */
    View selectedAction() {
        View card = getSelectedView();
        View action = card == null ? null : card.findViewById(R.id.grid_more);
        return action != null && action.getVisibility() == VISIBLE && action.isEnabled() ? action : null;
    }

    /** No card follows the selected one in its row, so the end direction has nowhere else to go. */
    private boolean atRowEnd() {
        int position = getSelectedItemPosition();
        int columns = Math.max(1, getNumColumns());
        return position == INVALID_POSITION || position % columns == columns - 1 || position == getCount() - 1;
    }

    boolean isActionMarked() {
        return markedAction != null;
    }

    private void markAction(View action) {
        if (markedAction == action) return;
        // Hover is the one state a card does not pass down to its children, so it can mark the button
        // alone; the button's background draws the same ring as a focused icon button.
        if (markedAction != null) markedAction.setHovered(false);
        markedAction = action;
        if (action != null) action.setHovered(true);
    }

    /** The keys that move a grid's selection, which may leave the new card partly out of view. */
    private static boolean movesSelection(int keyCode) {
        switch (keyCode) {
            case KeyEvent.KEYCODE_DPAD_UP:
            case KeyEvent.KEYCODE_DPAD_DOWN:
            case KeyEvent.KEYCODE_DPAD_LEFT:
            case KeyEvent.KEYCODE_DPAD_RIGHT:
            case KeyEvent.KEYCODE_PAGE_UP:
            case KeyEvent.KEYCODE_PAGE_DOWN:
            case KeyEvent.KEYCODE_MOVE_HOME:
            case KeyEvent.KEYCODE_MOVE_END:
            case KeyEvent.KEYCODE_TAB:
                return true;
            default:
                return false;
        }
    }

    private static boolean isActivateKey(int keyCode) {
        return keyCode == KeyEvent.KEYCODE_DPAD_CENTER || keyCode == KeyEvent.KEYCODE_ENTER ||
                keyCode == KeyEvent.KEYCODE_NUMPAD_ENTER || keyCode == KeyEvent.KEYCODE_SPACE ||
                keyCode == KeyEvent.KEYCODE_BUTTON_A;
    }

    private static boolean isBackKey(int keyCode) {
        return keyCode == KeyEvent.KEYCODE_BACK || keyCode == KeyEvent.KEYCODE_BUTTON_B || keyCode == KeyEvent.KEYCODE_ESCAPE;
    }

    /** True when the grid is too short for full-height artwork, as on a phone in landscape. */
    public boolean hasShortViewport() {
        int height = getMeasuredHeight();
        return height > 0 && height / getResources().getDisplayMetrics().density < SHORT_VIEWPORT_DP;
    }

    @Override
    protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        if (policySet && MeasureSpec.getMode(widthMeasureSpec) != MeasureSpec.UNSPECIFIED) {
            float density = getResources().getDisplayMetrics().density;
            Configuration configuration = getResources().getConfiguration();
            float fontScale = Math.max(1, configuration.fontScale);
            boolean television = (configuration.uiMode & Configuration.UI_MODE_TYPE_MASK)
                    == Configuration.UI_MODE_TYPE_TELEVISION;
            int width = Math.max(0, MeasureSpec.getSize(widthMeasureSpec) - getPaddingLeft() - getPaddingRight());
            int columns = AdaptiveLayout.gridColumns(width / density / fontScale,
                    MeasureSpec.getSize(heightMeasureSpec) / density, computers, smallIcons, television);
            // A no-op when unchanged; otherwise the children are rebuilt by the layout that follows.
            setNumColumns(columns);
        }
        super.onMeasure(widthMeasureSpec, heightMeasureSpec);
    }
}
