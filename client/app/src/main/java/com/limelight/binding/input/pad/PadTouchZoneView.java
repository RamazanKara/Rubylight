package com.limelight.binding.input.pad;

import android.annotation.SuppressLint;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.view.View;

import com.limelight.binding.input.ControllerHandler;

/**
 * Play-mode view for a {@link PadTouchZone}: claims touches edge-to-edge under strict
 * {@code activePointerId} isolation, requests unbuffered dispatch, replays historical move
 * samples, stays invisible and allocation/redraw-free when {@code opacity == 0}, renders a
 * minimal {@link PadSkin70} floating ring otherwise, and auto-cancels on hide or detach.
 */
@SuppressLint("ViewConstructor")
final class PadTouchZoneView extends View {
    private static final int DECAY_TICK_MS = 16;

    final PadTouchZone zone;
    final PadTouchZoneHandler handler;
    private final PadSkin70 skin;
    private final float density;
    private final RectF rect = new RectF();
    private ModernController.PointerSink pointerSink;
    private float baseOpacity = 1f;
    private float effectiveOpacity;
    private int activePointerId = MotionEvent.INVALID_POINTER_ID;
    private long decayTickNs;
    private Runnable pendingTapUp;

    private final Runnable decayTick = new Runnable() {
        @Override
        public void run() {
            long now = SystemClock.elapsedRealtimeNanos();
            long elapsed = Math.max(1_000_000L, now - decayTickNs);
            decayTickNs = now;
            if (handler.tickDecay(elapsed)) {
                postDelayed(this, DECAY_TICK_MS);
            }
        }
    };

    PadTouchZoneView(Context context, PadTouchZone zone, PadTouchZoneHandler handler, PadSkin70 skin) {
        super(context);
        this.zone = zone;
        this.handler = handler;
        this.skin = skin;
        this.density = context.getResources().getDisplayMetrics().density;
        setFocusable(false);
        setClickable(false);
        setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO);
    }

    void setPointerSink(ModernController.PointerSink sink) {
        this.pointerSink = sink;
    }

    void setBaseOpacity(float baseOpacity) {
        this.baseOpacity = Math.max(0f, Math.min(1f, baseOpacity));
        float next = this.baseOpacity * (zone.opacity / 100f);
        boolean changed = Float.compare(next, effectiveOpacity) != 0;
        effectiveOpacity = next;
        if (changed) invalidate();
    }

    void onTap(int mouseButton) {
        flushPendingTapUp();
        ModernController.PointerSink sink = pointerSink;
        if (sink == null) return;
        sink.button(mouseButton, true);
        Runnable release = new Runnable() {
            @Override
            public void run() {
                if (pendingTapUp != this) return;
                pendingTapUp = null;
                sink.button(mouseButton, false);
            }
        };
        pendingTapUp = release;
        postDelayed(release, ControllerHandler.MINIMUM_BUTTON_DOWN_TIME_MS);
    }

    @Override
    public boolean hasOverlappingRendering() {
        return false;
    }

    @SuppressLint("ClickableViewAccessibility")
    @Override
    public boolean onTouchEvent(MotionEvent event) {
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN: {
                requestUnbufferedDispatch(event);
                if (activePointerId == MotionEvent.INVALID_POINTER_ID) {
                    int index = 0;
                    int id = event.getPointerId(index);
                    activePointerId = id;
                    removeCallbacks(decayTick);
                    float lx = event.getX(index);
                    float ly = event.getY(index);
                    float travel = PadTouchZoneHandler.defaultTravelRadius(getWidth(), getHeight(), density);
                    handler.down(id, lx, ly, getLeft() + lx, getTop() + ly,
                            getWidth(), getHeight(), travel, event.getEventTime());
                    redraw();
                }
                return true;
            }
            case MotionEvent.ACTION_POINTER_DOWN: {
                if (activePointerId == MotionEvent.INVALID_POINTER_ID) {
                    requestUnbufferedDispatch(event);
                    int index = event.getActionIndex();
                    int id = event.getPointerId(index);
                    activePointerId = id;
                    removeCallbacks(decayTick);
                    float lx = event.getX(index);
                    float ly = event.getY(index);
                    float travel = PadTouchZoneHandler.defaultTravelRadius(getWidth(), getHeight(), density);
                    handler.down(id, lx, ly, getLeft() + lx, getTop() + ly,
                            getWidth(), getHeight(), travel, event.getEventTime());
                    redraw();
                }
                return true;
            }
            case MotionEvent.ACTION_MOVE: {
                if (activePointerId == MotionEvent.INVALID_POINTER_ID) return false;
                int index = event.findPointerIndex(activePointerId);
                if (index < 0) return true;
                int history = event.getHistorySize();
                for (int h = 0; h < history; h++) {
                    float hx = event.getHistoricalX(index, h);
                    float hy = event.getHistoricalY(index, h);
                    handler.move(activePointerId, hx, hy, getLeft() + hx, getTop() + hy,
                            PadControlView.eventTimeNs(event, h), density);
                }
                float lx = event.getX(index);
                float ly = event.getY(index);
                handler.move(activePointerId, lx, ly, getLeft() + lx, getTop() + ly,
                        PadControlView.eventTimeNs(event, -1), density);
                redraw();
                return true;
            }
            case MotionEvent.ACTION_POINTER_UP: {
                int id = event.getPointerId(event.getActionIndex());
                if (id == activePointerId) {
                    activePointerId = MotionEvent.INVALID_POINTER_ID;
                    handler.up(id, event.getEventTime());
                    startDecayIfNeeded();
                    redraw();
                }
                return true;
            }
            case MotionEvent.ACTION_UP: {
                int id = event.getPointerId(event.getActionIndex());
                if (id == activePointerId) {
                    activePointerId = MotionEvent.INVALID_POINTER_ID;
                    handler.up(id, event.getEventTime());
                    startDecayIfNeeded();
                    redraw();
                } else if (activePointerId != MotionEvent.INVALID_POINTER_ID) {
                    releaseInput();
                }
                return true;
            }
            case MotionEvent.ACTION_CANCEL:
                releaseInput();
                return true;
            default:
                return false;
        }
    }

    @Override
    protected void onDraw(Canvas canvas) {
        if (effectiveOpacity <= 0f) return;
        float w = getWidth();
        float h = getHeight();
        if (w <= 0f || h <= 0f) return;
        skin.setOpacity(effectiveOpacity);
        try {
            float inset = 2f * density;
            rect.set(inset, inset, w - inset, h - inset);
            float radius = 12f * density;
            Paint border = skin.line(PadSkin70.alpha(skin.rim, 0.35f), density);
            canvas.drawRoundRect(rect, radius, radius, border);
            if (!handler.isActive()) return;
            if (zone.kind.isMove()) {
                float travel = PadTouchZoneHandler.defaultTravelRadius(w, h, density);
                int ringColor = handler.isSprinting()
                        ? skin.pressedRim : PadSkin70.alpha(skin.rim, 0.65f);
                canvas.drawCircle(handler.originX(), handler.originY(), travel,
                        skin.line(ringColor, 1.5f * density));
                float knobR = Math.max(10f * density, travel * 0.28f);
                int knobColor = handler.isSprinting() ? skin.pressedRim : skin.rim;
                canvas.drawCircle(handler.touchX(), handler.touchY(), knobR,
                        skin.line(knobColor, 2f * density));
            } else {
                float dotR = 14f * density;
                canvas.drawCircle(handler.touchX(), handler.touchY(), dotR,
                        skin.line(PadSkin70.alpha(skin.pressedRim, 0.7f), 1.5f * density));
            }
        } finally {
            skin.setOpacity(1f);
        }
    }

    @Override
    public void setVisibility(int visibility) {
        super.setVisibility(visibility);
        if (visibility != VISIBLE) releaseInput();
    }

    @Override
    protected void onVisibilityChanged(View changedView, int visibility) {
        super.onVisibilityChanged(changedView, visibility);
        if (visibility != VISIBLE) releaseInput();
    }

    @Override
    protected void onDetachedFromWindow() {
        super.onDetachedFromWindow();
        releaseInput();
    }

    void releaseInput() {
        activePointerId = MotionEvent.INVALID_POINTER_ID;
        removeCallbacks(decayTick);
        flushPendingTapUp();
        handler.release();
        redraw();
    }

    private void startDecayIfNeeded() {
        removeCallbacks(decayTick);
        if (handler.isDecayActive()) {
            decayTickNs = SystemClock.elapsedRealtimeNanos();
            postDelayed(decayTick, DECAY_TICK_MS);
        }
    }

    private void flushPendingTapUp() {
        Runnable pending = pendingTapUp;
        if (pending != null) {
            removeCallbacks(pending);
            pending.run();
        }
    }

    private void redraw() {
        if (effectiveOpacity > 0f) invalidate();
    }
}
