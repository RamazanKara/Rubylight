package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.graphics.Typeface;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.view.View;

import com.limelight.R;
import com.limelight.binding.input.keyboard.PcKey;
import com.limelight.binding.input.keyboard.PcKeyboardController;
import com.limelight.binding.input.keyboard.PcKeyboardLayout;
import com.limelight.binding.input.keyboard.PcModifier;
import com.limelight.binding.input.keyboard.StickyModifiers;

import java.util.List;

/**
 * Draws the PC keyboard and turns touches into presses on its {@link PcKeyboardController}. Keys are
 * laid out from the layout's unit positions, so every size and aspect works. Nothing here allocates
 * while drawing or touching, apart from the boxed pointer id the engine uses as an owner.
 */
public final class PcKeyboardView extends View {
    private static final int KEY = 0xFF2B2F36, KEY_MODIFIER = 0xFF22262C, KEY_PRESSED = 0xFF5B6574;
    private static final int ARMED = 0xFF3F7FD6, LOCKED = 0xFFE8A33D, FOCUS = 0xFFFFFFFF;
    private static final int LABEL = 0xFFF2F4F7, LABEL_SECONDARY = 0xFF9AA3AF;

    private final PcKeyboardController controller;
    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint stroke = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint text = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final RectF rect = new RectF();
    private final Runnable ticker = this::tick;
    private final float gap;
    private float unitX, unitY;
    private boolean ticking;
    private int heightLimit = Integer.MAX_VALUE;

    public PcKeyboardView(Context context, PcKeyboardController controller) {
        super(context);
        this.controller = controller;
        float density = context.getResources().getDisplayMetrics().density;
        gap = Math.max(1.5f, 2 * density);
        stroke.setStyle(Paint.Style.STROKE);
        text.setTextAlign(Paint.Align.CENTER);
        text.setTypeface(Typeface.DEFAULT_BOLD);
        setFocusable(false);
        setContentDescription(context.getString(R.string.pc_keyboard_title));
    }

    /** Redraw, and keep the repeat and navigation timer running while something needs it. */
    public void changed() {
        invalidate();
        if (controller.needsTick() && !ticking) {
            ticking = true;
            postOnAnimation(ticker);
        }
    }

    private void tick() {
        ticking = false;
        controller.tick(SystemClock.uptimeMillis());
        invalidate();
        if (controller.needsTick() && isAttachedToWindow()) {
            ticking = true;
            postOnAnimation(ticker);
        }
    }

    @Override
    protected void onDetachedFromWindow() {
        removeCallbacks(ticker);
        ticking = false;
        super.onDetachedFromWindow();
    }

    /** The most height the keys may take; the overlay sets it from the window and the size setting. */
    void setHeightLimit(int limit) {
        heightLimit = limit;
    }

    @Override
    protected void onMeasure(int widthSpec, int heightSpec) {
        int width = MeasureSpec.getSize(widthSpec);
        PcKeyboardLayout layout = controller.layout();
        // Keys about as tall as they are wide at most; a short window squeezes them instead of scrolling.
        int natural = Math.round(width / layout.widthUnits() * layout.rowCount() * 1.1f);
        setMeasuredDimension(width, Math.max(1, Math.min(natural, heightLimit)));
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        PcKeyboardLayout layout = controller.layout();
        unitX = w / layout.widthUnits();
        unitY = h / (float) layout.rowCount();
    }

    private void bounds(PcKey key, int row, RectF out) {
        out.set(key.x * unitX + gap / 2, row * unitY + gap / 2,
                (key.x + key.width) * unitX - gap / 2, (row + 1) * unitY - gap / 2);
    }

    @Override
    protected void onDraw(Canvas canvas) {
        PcKeyboardLayout layout = controller.layout();
        unitX = getWidth() / layout.widthUnits();
        unitY = getHeight() / (float) layout.rowCount();
        float radius = Math.min(unitX, unitY) * 0.14f;
        float baseSize = Math.min(unitY * 0.42f, unitX * 0.5f);
        stroke.setStrokeWidth(Math.max(2f, unitY * 0.07f));
        boolean shift = controller.engine().modifiers().isActive(PcModifier.SHIFT);
        boolean altGr = controller.engine().modifiers().isActive(PcModifier.ALTGR);
        PcKey focused = controller.isFocusVisible() ? controller.focus().focused() : null;

        List<List<PcKey>> rows = layout.rows();
        for (int row = 0; row < rows.size(); row++) {
            List<PcKey> keys = rows.get(row);
            for (int i = 0; i < keys.size(); i++) {
                PcKey key = keys.get(i);
                bounds(key, row, rect);
                StickyModifiers.State state = key.modifier == null ? StickyModifiers.State.OFF
                        : controller.engine().modifiers().state(key.modifier);
                boolean pressed = controller.engine().isKeyHeld(key);
                fill.setColor(pressed ? KEY_PRESSED : state == StickyModifiers.State.LOCKED ? LOCKED
                        : state == StickyModifiers.State.ONCE ? ARMED : key.modifier != null ? KEY_MODIFIER : KEY);
                canvas.drawRoundRect(rect, radius, radius, fill);
                if (key == focused) {
                    stroke.setColor(FOCUS);
                    canvas.drawRoundRect(rect, radius, radius, stroke);
                }
                drawLabel(canvas, key, shift, altGr, baseSize, state == StickyModifiers.State.LOCKED);
            }
        }
    }

    private void drawLabel(Canvas canvas, PcKey key, boolean shift, boolean altGr, float baseSize, boolean onLight) {
        String label = key.labelFor(shift, altGr);
        float size = baseSize;
        float room = rect.width() - gap * 2;
        text.setTextSize(size);
        float measured = text.measureText(label);
        if (measured > room) {
            size *= room / measured;
            text.setTextSize(size);
        }
        text.setColor(onLight ? 0xFF1A1D22 : LABEL);
        float x = rect.centerX();
        float y = rect.centerY() - (text.ascent() + text.descent()) / 2;
        // A key with a second legend shows it small in the corner: the shifted character on the main layout.
        if (key.shiftLabel != null && !shift && key.shiftLabel.length() <= 2) {
            float small = Math.min(size * 0.6f, baseSize * 0.6f);
            text.setTextSize(small);
            text.setColor(LABEL_SECONDARY);
            canvas.drawText(key.shiftLabel, rect.left + rect.width() * 0.26f, rect.top + rect.height() * 0.34f, text);
            text.setTextSize(size);
            text.setColor(LABEL);
            y = rect.centerY() + size * 0.5f;
        }
        canvas.drawText(label, x, y, text);
    }

    private PcKey keyAt(float x, float y) {
        PcKeyboardLayout layout = controller.layout();
        int row = (int) (y / unitY);
        if (row < 0 || row >= layout.rowCount()) return null;
        List<PcKey> keys = layout.rows().get(row);
        float units = x / unitX;
        PcKey nearest = null;
        float nearestDistance = Float.MAX_VALUE;
        for (int i = 0; i < keys.size(); i++) {
            PcKey key = keys.get(i);
            if (units >= key.x && units < key.x + key.width) return key;
            // The gap between two keys goes to the closer one, so a finger between them still types.
            float distance = Math.min(Math.abs(units - key.x), Math.abs(units - (key.x + key.width)));
            if (distance < nearestDistance) {
                nearestDistance = distance;
                nearest = key;
            }
        }
        return nearestDistance < 0.35f ? nearest : null;
    }

    @Override
    public boolean onTouchEvent(MotionEvent event) {
        int action = event.getActionMasked();
        switch (action) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN: {
                int index = event.getActionIndex();
                PcKey key = keyAt(event.getX(index), event.getY(index));
                if (key != null) {
                    controller.touchDown(key, event.getPointerId(index), event.getEventTime());
                    Haptics.perform(this, Haptics.Kind.KEY);
                    changed();
                }
                return true;
            }
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP:
                controller.touchUp(event.getPointerId(event.getActionIndex()));
                changed();
                return true;
            case MotionEvent.ACTION_CANCEL:
                for (int i = 0; i < event.getPointerCount(); i++) controller.touchUp(event.getPointerId(i));
                changed();
                return true;
            default:
                return true;
        }
    }
}
