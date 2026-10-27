package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.os.Handler;
import android.os.Looper;
import android.text.TextPaint;
import android.text.TextUtils;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewConfiguration;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;
import com.limelight.binding.input.DesktopControlsConfig;

/** A self-contained relative pad. No connection, stream touch state, timers for dragging, or momentum. */
public final class DesktopTrackpad extends View {
    public interface Sink {
        void move(float dx, float dy);
        void scroll(float vertical, float horizontal);
        void mouseButton(int button, boolean down);
    }

    private final Sink sink;
    private final DesktopControlsConfig config;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final TextPaint hintPaint = new TextPaint(Paint.ANTI_ALIAS_FLAG);
    private final RectF bounds = new RectF();
    private final String[] hintLines;
    private String[] fittedLines = new String[0];
    private final float density;
    private final float slop;
    private final boolean[] pulsed = new boolean[6];
    private final Runnable[] releases = new Runnable[6];
    private boolean active, ignored, moved, wasTwoFinger;
    private float lastX, lastY, distance;
    private long downTime;

    public DesktopTrackpad(Context context, DesktopControlsConfig configuration, Sink sink) {
        super(context);
        this.config = configuration.copy();
        this.sink = sink;
        density = getResources().getDisplayMetrics().density;
        slop = ViewConfiguration.get(context).getScaledTouchSlop();
        String[] hints = context.getString(config.tapToClick ? R.string.desktop_controls_pad_hint :
                R.string.desktop_controls_pad_no_tap).split("\n");
        hintLines = !config.tapToClick && !config.mouseButtons ? new String[] {hints[0]} : hints;
        hintPaint.setTextAlign(Paint.Align.CENTER);
        setMinimumHeight(Math.round(80 * density));
        setClickable(true);
        setFocusable(false);
        setContentDescription(config.tapToClick ? context.getString(R.string.desktop_controls_pad_accessibility) :
                context.getString(R.string.desktop_controls_trackpad) + ". " + android.text.TextUtils.join(". ", hintLines));
        for (int i = 1; i < releases.length; i++) {
            final int button = i;
            releases[i] = () -> {
                if (pulsed[button]) {
                    pulsed[button] = false;
                    sink.mouseButton(button, false);
                }
            };
        }
    }

    @Override protected void onSizeChanged(int width, int height, int oldWidth, int oldHeight) {
        super.onSizeChanged(width, height, oldWidth, oldHeight);
        bounds.set(density, density, width - density, height - density);
        hintPaint.setTextSize(12 * getResources().getDisplayMetrics().scaledDensity);
        fittedLines = new String[hintLines.length];
        for (int i = 0; i < hintLines.length; i++) {
            fittedLines[i] = TextUtils.ellipsize(hintLines[i], hintPaint,
                    Math.max(0, width - 24 * density), TextUtils.TruncateAt.END).toString();
        }
    }

    @Override protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        int outline = MaterialColors.getColor(this, com.google.android.material.R.attr.colorOutlineVariant);
        int accent = MaterialColors.getColor(this, androidx.appcompat.R.attr.colorPrimary);
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(MaterialColors.getColor(this, com.google.android.material.R.attr.colorSurfaceContainerLow));
        canvas.drawRoundRect(bounds, 14 * density, 14 * density, paint);
        paint.setStyle(Paint.Style.STROKE);
        paint.setStrokeWidth(density);
        paint.setColor(active ? accent : outline);
        canvas.drawRoundRect(bounds, 14 * density, 14 * density, paint);
        paint.setStyle(Paint.Style.FILL);
        hintPaint.setColor(MaterialColors.getColor(this, com.google.android.material.R.attr.colorOnSurfaceVariant));
        float lineHeight = hintPaint.getFontSpacing();
        float baseline = getHeight() / 2f - (fittedLines.length - 1) * lineHeight / 2f -
                (hintPaint.ascent() + hintPaint.descent()) / 2f;
        for (String line : fittedLines) {
            canvas.drawText(line, getWidth() / 2f, baseline, hintPaint);
            baseline += lineHeight;
        }
    }

    @Override public boolean onTouchEvent(MotionEvent event) {
        if (!isEnabled()) return false;
        int action = event.getActionMasked();
        if (action == MotionEvent.ACTION_DOWN) {
            requestUnbufferedDispatch(event);
            releaseInputs();
            active = true;
            ignored = moved = wasTwoFinger = false;
            distance = 0;
            downTime = event.getEventTime();
            lastX = event.getX();
            lastY = event.getY();
            if (getParent() != null) getParent().requestDisallowInterceptTouchEvent(true);
            invalidate();
            return true;
        }
        if (action == MotionEvent.ACTION_CANCEL) {
            releaseInputs();
            return true;
        }
        if (!active || ignored) {
            if (action == MotionEvent.ACTION_UP) releaseInputs();
            return true;
        }
        if (action == MotionEvent.ACTION_POINTER_DOWN) {
            if (event.getPointerCount() > 2) {
                ignored = true;
                moved = true;
            } else {
                wasTwoFinger = true;
                lastX = centroid(event, true, -1);
                lastY = centroid(event, false, -1);
            }
            return true;
        }
        if (action == MotionEvent.ACTION_POINTER_UP) {
            if ((event.getFlags() & MotionEvent.FLAG_CANCELED) != 0) {
                releaseInputs();
            } else {
                lastX = centroid(event, true, event.getActionIndex());
                lastY = centroid(event, false, event.getActionIndex());
            }
            return true;
        }
        if (action == MotionEvent.ACTION_MOVE) {
            float x = centroid(event, true, -1), y = centroid(event, false, -1);
            float dx = x - lastX, dy = y - lastY;
            distance += Math.hypot(dx, dy);
            moved |= distance > slop;
            if (event.getPointerCount() == 2) {
                if (moved) {
                    float direction = config.naturalScroll ? 1 : -1;
                    // 40 dp of travel is one 120-unit high-resolution wheel notch.
                    // Wheel-up and wheel-right have opposite content-motion signs.
                    sink.scroll(direction * dy * 3 / density, -direction * dx * 3 / density);
                }
            } else if (!wasTwoFinger) {
                float scale = config.pointerSpeedPercent / 100f;
                if (dx != 0 || dy != 0) sink.move(dx * scale, dy * scale);
            }
            // After a two-finger gesture, the remaining finger cannot move/click accidentally.
            lastX = x;
            lastY = y;
            return true;
        }
        if (action == MotionEvent.ACTION_UP) {
            boolean click = config.tapToClick && !moved && event.getEventTime() - downTime <= 300 &&
                    (event.getFlags() & MotionEvent.FLAG_CANCELED) == 0;
            active = false;
            if (getParent() != null) getParent().requestDisallowInterceptTouchEvent(false);
            if (click) {
                if (wasTwoFinger) pulse(3); // Moonlight: left=1, middle=2, right=3.
                else performClick();
            }
            invalidate();
            return true;
        }
        return true;
    }

    private static float centroid(MotionEvent event, boolean x, int skip) {
        float total = 0;
        int count = 0;
        for (int i = 0; i < event.getPointerCount(); i++) {
            if (i == skip) continue;
            total += x ? event.getX(i) : event.getY(i);
            count++;
        }
        return count == 0 ? 0 : total / count;
    }

    @Override public boolean performClick() {
        super.performClick();
        // Accessibility click is an explicit click, even when gesture tap-to-click is disabled.
        if (isEnabled()) pulse(1);
        return true;
    }

    private void pulse(int button) {
        handler.removeCallbacks(releases[button]);
        releases[button].run();
        pulsed[button] = true;
        sink.mouseButton(button, true);
        handler.postDelayed(releases[button], 100);
    }

    public void releaseInputs() {
        active = ignored = moved = wasTwoFinger = false;
        for (int i = 1; i < releases.length; i++) {
            handler.removeCallbacks(releases[i]);
            releases[i].run();
        }
        if (getParent() != null) getParent().requestDisallowInterceptTouchEvent(false);
        invalidate();
    }

    @Override public void onWindowFocusChanged(boolean hasWindowFocus) {
        super.onWindowFocusChanged(hasWindowFocus);
        if (!hasWindowFocus) releaseInputs();
    }

    @Override protected void onDetachedFromWindow() {
        releaseInputs();
        super.onDetachedFromWindow();
    }
}
