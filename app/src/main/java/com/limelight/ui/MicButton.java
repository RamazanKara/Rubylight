package com.limelight.ui;

import android.annotation.SuppressLint;
import android.content.Context;
import android.content.SharedPreferences;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.drawable.Drawable;
import android.view.HapticFeedbackConstants;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewConfiguration;
import android.widget.FrameLayout;

import androidx.appcompat.content.res.AppCompatResources;
import androidx.core.content.ContextCompat;

import com.limelight.R;

/**
 * The stream's microphone button: a round glass button that mutes and unmutes the microphone with
 * one tap, and moves when dragged. Live shows the microphone with an amber rim; muted shows it
 * crossed out on red, so the state never rests on colour alone. Where it sits is kept as a fraction
 * of the stream, in its own preferences file, so it survives rotations and new sessions.
 */
@SuppressLint("ViewConstructor") // Created by Game for the stream.
public final class MicButton extends View {
    /** What the button asks of the stream. */
    public interface Host {
        /** Whether the host takes the microphone this session, so muting means something. */
        boolean offered();

        boolean muted();

        void setMuted(boolean muted);

        /** The bottom of a row of controls along the top middle, in the frame's pixels, to sit below; NaN for none. */
        float topRowBottom();
    }

    static final String PREFS = "mic_button";
    static final String KEY_X = "x", KEY_Y = "y";
    public static final float SIZE_DP = 48;
    private static final float MARGIN_DP = 12;
    /** How often the button checks the session: the host can offer the microphone late, a controller can mute it. */
    private static final long POLL_MS = 1000;

    private final Host host;
    private final SharedPreferences store;
    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG), rim = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Drawable live, muted;
    private final int accent, mutedFill, mutedIcon, liveIcon;
    private final float density, slop;
    private float downX, downY, startX, startY;
    private boolean dragging, shownMuted, suppressed;
    private final Runnable poll = new Runnable() {
        @Override
        public void run() {
            refresh();
            postDelayed(this, POLL_MS);
        }
    };

    public MicButton(Context context, Host host) {
        super(context);
        this.host = host;
        store = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        density = context.getResources().getDisplayMetrics().density;
        slop = ViewConfiguration.get(context).getScaledTouchSlop();
        accent = ContextCompat.getColor(context, R.color.night_primary);
        mutedFill = ContextCompat.getColor(context, R.color.night_error_container);
        mutedIcon = ContextCompat.getColor(context, R.color.night_on_error_container);
        liveIcon = 0xF2FFFFFF;
        live = AppCompatResources.getDrawable(context, R.drawable.ic_mic).mutate();
        muted = AppCompatResources.getDrawable(context, R.drawable.ic_mic_off).mutate();
        live.setTint(liveIcon);
        muted.setTint(mutedIcon);
        rim.setStyle(Paint.Style.STROKE);
        setFocusable(false);
        setHapticFeedbackEnabled(true);
        shownMuted = host.muted();
        setVisibility(GONE);
        describe();
    }

    /** The frame layout parameters for the button: a fixed size at the frame's top left, moved by translation. */
    public FrameLayout.LayoutParams layoutParams() {
        int size = Math.round(SIZE_DP * density);
        return new FrameLayout.LayoutParams(size, size, android.view.Gravity.TOP | android.view.Gravity.START);
    }

    /** Hides the button whatever the session says: picture-in-picture, no input permission, the setting off. */
    public void setSuppressed(boolean suppressed) {
        this.suppressed = suppressed;
        refresh();
    }

    /** Shows the session again: whether the host offers the microphone, and the mute state the menu or a controller changed. */
    public void refresh() {
        int visibility = !suppressed && host.offered() ? VISIBLE : GONE;
        if (visibility != getVisibility()) {
            if (visibility == GONE) {
                setPressed(false);
                dragging = false;
            }
            setVisibility(visibility);
        }
        boolean now = host.muted();
        if (now == shownMuted) return;
        shownMuted = now;
        describe();
        invalidate();
    }


    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        View parent = (View) getParent();
        parent.addOnLayoutChangeListener(relayout);
        parent.post(this::place);
        removeCallbacks(poll);
        post(poll);
    }

    @Override
    protected void onDetachedFromWindow() {
        removeCallbacks(poll);
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
                parent.getWidth(), parent.getHeight(), SIZE_DP * density, MARGIN_DP * density, host.topRowBottom());
        float half = SIZE_DP * density / 2;
        setTranslationX(center[0] - half);
        setTranslationY(center[1] - half);
    }

    private void describe() {
        setContentDescription(getContext().getString(shownMuted ? R.string.mic_button_unmute : R.string.mic_button_mute));
    }

    @Override
    protected void onDraw(Canvas canvas) {
        float size = Math.min(getWidth(), getHeight()), r = size / 2;
        float stroke = (shownMuted ? 1.5f : 2f) * density;
        fill.setColor(shownMuted ? (mutedFill & 0x00FFFFFF) | 0xE6000000 : 0x8C000000);
        canvas.drawCircle(r, r, r - stroke / 2, fill);
        rim.setStrokeWidth(stroke);
        rim.setColor(shownMuted ? 0x66FFFFFF : accent);
        canvas.drawCircle(r, r, r - stroke / 2, rim);
        Drawable icon = shownMuted ? muted : live;
        int inset = Math.round(size * .25f);
        icon.setBounds(inset, inset, Math.round(size) - inset, Math.round(size) - inset);
        icon.draw(canvas);
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
                    setTranslationX(Placement.clamp(startX + dx, 0, parent.getWidth() - getWidth()));
                    setTranslationY(Placement.clamp(startY + dy, 0, parent.getHeight() - getHeight()));
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
        host.setMuted(!host.muted());
        performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP);
        refresh();
        return true;
    }

    /** Where the button sits, as plain math so it is tested without a device. */
    public static final class Placement {
        private Placement() { }

        /**
         * The button's center in pixels. A stored fraction pair wins, kept so the whole button stays on
         * the surface; otherwise top center, {@code margin} below the top edge or below {@code rowBottom}.
         */
        public static float[] center(float fx, float fy, float width, float height, float size, float margin, float rowBottom) {
            float half = size / 2, x, y;
            if (!Float.isNaN(fx) && !Float.isNaN(fy)) {
                x = fx * width;
                y = fy * height;
            } else {
                x = width / 2;
                y = (Float.isNaN(rowBottom) ? 0 : rowBottom) + margin + half;
            }
            return new float[] {clamp(x, half, Math.max(half, width - half)), clamp(y, half, Math.max(half, height - half))};
        }

        static float clamp(float value, float min, float max) {
            return Math.max(min, Math.min(max, value));
        }
    }
}
