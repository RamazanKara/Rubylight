package com.limelight.ui;

import android.annotation.SuppressLint;
import android.content.Context;
import android.content.SharedPreferences;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.view.HapticFeedbackConstants;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewConfiguration;
import android.widget.FrameLayout;

import com.limelight.R;

/**
 * The stream's floating menu button: a round glass button that opens the stream menu with one tap and moves
 * when dragged, as the microphone button does. Off unless its setting is on. Where it sits is kept as a
 * fraction of the stream, in its own preferences file, so it survives rotations and new sessions.
 */
@SuppressLint("ViewConstructor") // Created by Game for the stream.
public final class MenuButton extends View {
    /** What the button asks of the stream. */
    public interface Host {
        void open();

        /** The bottom of a row of controls along the top middle, in the frame's pixels, to sit below; NaN for none. */
        float topRowBottom();
    }

    static final String PREFS = "menu_button";
    static final String KEY_X = "x", KEY_Y = "y";
    private static final float MARGIN_DP = 12;

    private final Host host;
    private final SharedPreferences store;
    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG), ink = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final float density, slop;
    private float downX, downY, startX, startY;
    private boolean dragging;

    public MenuButton(Context context, Host host) {
        super(context);
        this.host = host;
        store = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        density = context.getResources().getDisplayMetrics().density;
        slop = ViewConfiguration.get(context).getScaledTouchSlop();
        ink.setStyle(Paint.Style.STROKE);
        ink.setStrokeCap(Paint.Cap.ROUND);
        setFocusable(false);
        setHapticFeedbackEnabled(true);
        setContentDescription(context.getString(R.string.stream_menu_settings));
        setVisibility(GONE);
    }

    /** The frame layout parameters for the button: a fixed size at the frame's top left, moved by translation. */
    public FrameLayout.LayoutParams layoutParams() {
        int size = Math.round(MicButton.SIZE_DP * density);
        return new FrameLayout.LayoutParams(size, size, android.view.Gravity.TOP | android.view.Gravity.START);
    }

    /** Shows or hides the button: its setting, and the overlays not hidden (picture in picture). */
    public void setShown(boolean shown) {
        int visibility = shown ? VISIBLE : GONE;
        if (visibility == getVisibility()) return;
        if (!shown) {
            setPressed(false);
            dragging = false;
        }
        setVisibility(visibility);
    }

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        View parent = (View) getParent();
        parent.addOnLayoutChangeListener(relayout);
        parent.post(this::place);
    }

    @Override
    protected void onDetachedFromWindow() {
        ((View) getParent()).removeOnLayoutChangeListener(relayout);
        super.onDetachedFromWindow();
    }

    private final OnLayoutChangeListener relayout = (v, l, t, r, b, ol, ot, or, ob) -> {
        if (r - l != or - ol || b - t != ob - ot) place();
    };

    /** Puts the button where it was dragged, or at its default place when it never was; the controls below may have changed. */
    public void place() {
        View parent = (View) getParent();
        if (parent == null || parent.getWidth() == 0 || dragging) return;
        float[] center = Placement.center(store.getFloat(KEY_X, Float.NaN), store.getFloat(KEY_Y, Float.NaN),
                parent.getWidth(), parent.getHeight(), MicButton.SIZE_DP * density, MARGIN_DP * density, host.topRowBottom());
        float half = MicButton.SIZE_DP * density / 2;
        setTranslationX(center[0] - half);
        setTranslationY(center[1] - half);
    }

    @Override
    protected void onDraw(Canvas canvas) {
        float size = Math.min(getWidth(), getHeight()), r = size / 2;
        float stroke = 1.5f * density;
        fill.setColor(isPressed() ? 0xB3000000 : 0x8C000000);
        canvas.drawCircle(r, r, r - stroke / 2, fill);
        ink.setStrokeWidth(stroke);
        ink.setColor(0x66FFFFFF);
        canvas.drawCircle(r, r, r - stroke / 2, ink);
        // Three lines, the menu's sign.
        ink.setStrokeWidth(2 * density);
        ink.setColor(0xF2FFFFFF);
        float half = size * .18f, gap = size * .13f;
        for (int line = -1; line <= 1; line++) canvas.drawLine(r - half, r + line * gap, r + half, r + line * gap, ink);
    }

    @Override
    protected void drawableStateChanged() {
        super.drawableStateChanged();
        invalidate();
    }

    @SuppressLint("ClickableViewAccessibility") // A tap is performClick; a drag only moves the button.
    @Override
    public boolean onTouchEvent(MotionEvent event) {
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                downX = event.getRawX();
                downY = event.getRawY();
                startX = getTranslationX();
                startY = getTranslationY();
                dragging = false;
                setPressed(true);
                return true;
            case MotionEvent.ACTION_MOVE: {
                float dx = event.getRawX() - downX, dy = event.getRawY() - downY;
                if (!dragging && Math.hypot(dx, dy) > slop) dragging = true;
                if (dragging) {
                    View parent = (View) getParent();
                    setTranslationX(MicButton.Placement.clamp(startX + dx, 0, parent.getWidth() - getWidth()));
                    setTranslationY(MicButton.Placement.clamp(startY + dy, 0, parent.getHeight() - getHeight()));
                }
                return true;
            }
            case MotionEvent.ACTION_UP:
                setPressed(false);
                if (dragging) {
                    View parent = (View) getParent();
                    float half = getWidth() / 2f;
                    store.edit().putFloat(KEY_X, (getTranslationX() + half) / Math.max(1, parent.getWidth()))
                            .putFloat(KEY_Y, (getTranslationY() + half) / Math.max(1, parent.getHeight())).apply();
                    // Over the stream's SurfaceView a moved button was left undrawn until the next layout (Retroid
                    // Pocket); redraw the frame and lay it out, as showing the button does.
                    parent.invalidate();
                    requestLayout();
                } else {
                    performClick();
                }
                dragging = false;
                return true;
            case MotionEvent.ACTION_CANCEL:
                setPressed(false);
                boolean moved = dragging;
                dragging = false;
                if (moved) place();
                return true;
            default:
                return true;
        }
    }

    @Override
    public boolean performClick() {
        super.performClick();
        performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP);
        host.open();
        return true;
    }

    /** Where the button sits, as plain math so it is tested without a device. */
    public static final class Placement {
        private Placement() { }

        /**
         * The button's center in pixels. A stored fraction pair wins, kept so the whole button stays on the
         * surface; otherwise beside the microphone button's default place, at the top middle.
         */
        public static float[] center(float fx, float fy, float width, float height, float size, float margin, float rowBottom) {
            if (Float.isNaN(fx) || Float.isNaN(fy)) {
                fx = (width / 2 + size + margin) / Math.max(1, width);
                fy = ((Float.isNaN(rowBottom) ? 0 : rowBottom) + margin + size / 2) / Math.max(1, height);
            }
            return MicButton.Placement.center(fx, fy, width, height, size, margin, rowBottom);
        }
    }
}
