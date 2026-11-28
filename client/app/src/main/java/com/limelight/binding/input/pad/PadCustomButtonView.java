package com.limelight.binding.input.pad;

import android.annotation.SuppressLint;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.RectF;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.view.View;
import android.view.animation.AnimationUtils;

import com.limelight.ui.Haptics;

/**
 * Play-mode view for a {@link PadCustomButton}: draws with {@link PadSkin70}, owns a
 * {@link HaloLayer} for unclipped press glow, displays a toggle-latch indicator, enforces strict
 * {@code activePointerId} isolation, routes Hold/Toggle key/chord/mouse/scroll bindings via
 * {@link PadHeldInputs}, forwards frame-space drag-aim with historical samples, and releases
 * unconditionally on cancel, hide, detach, or editing.
 */
@SuppressLint("ViewConstructor")
final class PadCustomButtonView extends View {
    private static final int DECAY_TICK_MS = 16;

    final PadCustomButton button;
    private final PadSkin70 skin;
    private final PadHeldInputs heldInputs;
    private final PadTouchZoneHandler.AimForwarder aimForwarder;
    private final int aimToken;
    private final float density;
    private final HaloLayer haloLayer;
    private final RectF shapeRect = new RectF();

    private ModernController.PointerSink pointerSink;
    private boolean hapticsEnabled = true;
    private float restingAlpha = 1f;
    private int activePointerId = MotionEvent.INVALID_POINTER_ID;
    private boolean pressed;
    private boolean latched;
    private boolean aimingActive;
    private float pressLevel;
    private long releasedAtMs;
    private long decayTickNs;

    private final Runnable decayTick = new Runnable() {
        @Override
        public void run() {
            long now = SystemClock.elapsedRealtimeNanos();
            long elapsed = Math.max(1_000_000L, now - decayTickNs);
            decayTickNs = now;
            if (aimForwarder != null && aimForwarder.tickDecay(elapsed)) {
                postDelayed(this, DECAY_TICK_MS);
            }
        }
    };

    private final Runnable easeTick = new Runnable() {
        @Override
        public void run() {
            if (pressed || latched || restingAlpha <= 0f) return;
            long now = AnimationUtils.currentAnimationTimeMillis();
            pressLevel = PadPainter.easeBack(now - releasedAtMs);
            applyAlphaAndRedraw();
            if (pressLevel > 0f) {
                postOnAnimation(this);
            }
        }
    };

    @SuppressLint("ViewConstructor")
    static final class HaloLayer extends View {
        private final PadCustomButtonView owner;

        HaloLayer(Context context, PadCustomButtonView owner) {
            super(context);
            this.owner = owner;
            setFocusable(false);
            setClickable(false);
            setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO);
        }

        @Override
        protected void onDraw(Canvas canvas) {
            if (owner.restingAlpha <= 0f || owner.pressLevel <= 0f) return;
            owner.drawHalo(canvas, (getWidth() - owner.getWidth()) / 2f,
                    (getHeight() - owner.getHeight()) / 2f, owner.getWidth(), owner.getHeight());
        }

        @Override
        public boolean hasOverlappingRendering() {
            return false;
        }

        @SuppressLint("ClickableViewAccessibility")
        @Override
        public boolean onTouchEvent(MotionEvent event) {
            return false;
        }
    }

    PadCustomButtonView(Context context, PadCustomButton button, PadSkin70 skin,
                        PadHeldInputs heldInputs, PadTouchZoneHandler.AimForwarder aimForwarder,
                        int aimToken) {
        super(context);
        this.button = button;
        this.skin = skin;
        this.heldInputs = heldInputs;
        this.aimForwarder = aimForwarder;
        this.aimToken = aimToken;
        this.density = context.getResources().getDisplayMetrics().density;
        this.haloLayer = new HaloLayer(context, this);
        setFocusable(false);
        setHapticFeedbackEnabled(true);
    }

    View haloLayer() {
        return haloLayer;
    }

    int haloReach() {
        return (int) Math.ceil(skin.glowReach());
    }

    void setPointerSink(ModernController.PointerSink sink) {
        this.pointerSink = sink;
    }

    void setHaptics(boolean enabled) {
        this.hapticsEnabled = enabled;
    }

    void setRestingAlpha(float alpha) {
        float clamped = Math.max(0f, Math.min(1f, alpha));
        boolean wasVisible = restingAlpha > 0f;
        restingAlpha = clamped;
        applyAlphaAndRedraw();
        if (wasVisible != (clamped > 0f)) {
            invalidate();
            haloLayer.invalidate();
        }
    }

    boolean isLatched() {
        return latched;
    }

    boolean isPressedState() {
        return pressed;
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
                    handleDown(event, 0);
                }
                return true;
            }
            case MotionEvent.ACTION_POINTER_DOWN: {
                if (activePointerId == MotionEvent.INVALID_POINTER_ID) {
                    requestUnbufferedDispatch(event);
                    handleDown(event, event.getActionIndex());
                }
                return true;
            }
            case MotionEvent.ACTION_MOVE: {
                if (activePointerId == MotionEvent.INVALID_POINTER_ID) return false;
                int index = event.findPointerIndex(activePointerId);
                if (index < 0) return true;
                if (button.dragAim && aimingActive && aimForwarder != null) {
                    int history = event.getHistorySize();
                    for (int h = 0; h < history; h++) {
                        float hx = getLeft() + event.getHistoricalX(index, h);
                        float hy = getTop() + event.getHistoricalY(index, h);
                        aimForwarder.move(aimToken, hx, hy, PadControlView.eventTimeNs(event, h), density);
                    }
                    float fx = getLeft() + event.getX(index);
                    float fy = getTop() + event.getY(index);
                    aimForwarder.move(aimToken, fx, fy, PadControlView.eventTimeNs(event, -1), density);
                }
                return true;
            }
            case MotionEvent.ACTION_POINTER_UP: {
                int id = event.getPointerId(event.getActionIndex());
                if (id == activePointerId) {
                    handleUp();
                }
                return true;
            }
            case MotionEvent.ACTION_UP: {
                int id = event.getPointerId(event.getActionIndex());
                if (id == activePointerId) {
                    handleUp();
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

    private void handleDown(MotionEvent event, int index) {
        activePointerId = event.getPointerId(index);
        removeCallbacks(easeTick);
        removeCallbacks(decayTick);
        if (button.bindingKind == PadCustomButton.BindingKind.MOUSE_SCROLL) {
            pressed = true;
            pressLevel = 1f;
            applyBinding(true);
        } else if (button.toggle) {
            latched = !latched;
            pressLevel = latched ? 1f : 0f;
            applyBinding(latched);
        } else {
            pressed = true;
            pressLevel = 1f;
            applyBinding(true);
        }
        if (button.dragAim && aimForwarder != null) {
            aimingActive = aimForwarder.begin(aimToken, getLeft() + event.getX(index),
                    getTop() + event.getY(index));
        }
        if (hapticsEnabled) Haptics.perform(this, Haptics.Kind.CLICK);
        applyAlphaAndRedraw();
    }

    private void handleUp() {
        activePointerId = MotionEvent.INVALID_POINTER_ID;
        if (aimingActive && aimForwarder != null) {
            boolean decaying = aimForwarder.end(aimToken);
            aimingActive = false;
            if (decaying) {
                decayTickNs = SystemClock.elapsedRealtimeNanos();
                removeCallbacks(decayTick);
                postDelayed(decayTick, DECAY_TICK_MS);
            }
        }
        if (button.bindingKind == PadCustomButton.BindingKind.MOUSE_SCROLL) {
            pressed = false;
            startEase();
            return;
        }
        if (!button.toggle && pressed) {
            pressed = false;
            applyBinding(false);
            startEase();
        } else {
            applyAlphaAndRedraw();
        }
    }

    private void startEase() {
        if (restingAlpha <= 0f) {
            pressLevel = 0f;
            applyAlphaAndRedraw();
            return;
        }
        releasedAtMs = AnimationUtils.currentAnimationTimeMillis();
        removeCallbacks(easeTick);
        postOnAnimation(easeTick);
        applyAlphaAndRedraw();
    }

    private void applyBinding(boolean down) {
        switch (button.bindingKind) {
            case KEY:
                if (heldInputs != null) heldInputs.key(button.keyAt(0), down);
                break;
            case KEY_CHORD:
                if (heldInputs != null) {
                    if (down) {
                        for (int i = 0; i < button.keyCount(); i++) heldInputs.key(button.keyAt(i), true);
                    } else {
                        for (int i = button.keyCount() - 1; i >= 0; i--) heldInputs.key(button.keyAt(i), false);
                    }
                }
                break;
            case MOUSE_BUTTON:
                if (heldInputs != null) heldInputs.mouse(button.mouseButton, down);
                break;
            case MOUSE_SCROLL:
                if (down && pointerSink != null) {
                    pointerSink.scroll(button.scrollDirection * 120, 0);
                }
                break;
            default:
                break;
        }
    }

    void releaseInput() {
        activePointerId = MotionEvent.INVALID_POINTER_ID;
        removeCallbacks(easeTick);
        removeCallbacks(decayTick);
        if (aimForwarder != null && (aimingActive || aimForwarder.isDecaying())) {
            aimForwarder.cancel(aimToken);
            aimingActive = false;
        }
        if (pressed || latched) {
            boolean wasScroll = button.bindingKind == PadCustomButton.BindingKind.MOUSE_SCROLL;
            pressed = false;
            latched = false;
            if (!wasScroll) applyBinding(false);
        }
        pressLevel = 0f;
        applyAlphaAndRedraw();
    }

    @Override
    public void setVisibility(int visibility) {
        super.setVisibility(visibility);
        haloLayer.setVisibility(visibility);
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

    private void applyAlphaAndRedraw() {
        boolean active = pressed || latched;
        float level = active ? 1f : pressLevel;
        float alpha = PadControlView.alpha(restingAlpha, level);
        if (Float.compare(getAlpha(), alpha) != 0) setAlpha(alpha);
        if (Float.compare(haloLayer.getAlpha(), alpha) != 0) haloLayer.setAlpha(alpha);
        if (restingAlpha > 0f) {
            invalidate();
            haloLayer.invalidate();
        }
    }

    @Override
    protected void onDraw(Canvas canvas) {
        if (restingAlpha <= 0f) return;
        drawButton(canvas, skin, button, 0f, 0f, getWidth(), getHeight(), pressLevel, latched);
    }

    private void drawHalo(Canvas canvas, float left, float top, float width, float height) {
        float edge = PadPainter.EDGE_DP * density;
        if (button.shape == PadCustomButton.Shape.ROUND) {
            skin.glowDisc(canvas, left + width / 2f, top + height / 2f,
                    width / 2f - edge, height / 2f - edge, pressLevel);
        } else {
            shapeRect.set(left + edge, top + edge, left + width - edge, top + height - edge);
            float radius = button.shape == PadCustomButton.Shape.PILL
                    ? Math.min(shapeRect.width(), shapeRect.height()) / 2f
                    : Math.min(shapeRect.width(), shapeRect.height()) * 0.22f;
            skin.glowSlab(canvas, shapeRect, radius, pressLevel);
        }
    }

    /** Shared {@link PadSkin70} renderer used by both live play and {@link PadEditorSurface}. */
    static void drawButton(Canvas canvas, PadSkin70 skin, PadCustomButton button,
                           float left, float top, float width, float height,
                           float pressLevel, boolean latched) {
        if (skin.hidden() || width <= 0f || height <= 0f) return;
        float edge = PadPainter.EDGE_DP * skin.density;
        float cx = left + width / 2f;
        float cy = top + height / 2f;
        float minSide = Math.min(width, height);
        float rx = width / 2f - edge;
        float ry = height / 2f - edge;
        if (button.shape == PadCustomButton.Shape.ROUND) {
            skin.disc(canvas, cx, cy, rx, ry, pressLevel);
        } else {
            skin.inner.set(left + edge, top + edge, left + width - edge, top + height - edge);
            float radius = button.shape == PadCustomButton.Shape.PILL
                    ? Math.min(skin.inner.width(), skin.inner.height()) / 2f
                    : Math.min(skin.inner.width(), skin.inner.height()) * 0.22f;
            skin.slab(canvas, skin.inner, radius, pressLevel);
        }
        String text = button.effectiveLabel();
        float sunk = skin.sink(pressLevel);
        float innerMin = Math.max(8f * skin.density, minSide - 2f * edge - 2f * sunk);
        float innerWidth = Math.max(8f * skin.density, width - 2f * edge - 2f * sunk);
        float baseTextSize = innerMin * (text.length() <= 2 ? 0.40f : 0.32f);
        float measured = Math.max(1f, skin.textWidth(text, baseTextSize));
        float fit = Math.min(1f, (innerWidth * 0.78f) / measured);
        float textCy = button.toggle ? cy - innerMin * 0.08f : cy;
        skin.text(canvas, text, cx, textCy, Math.max(9f * skin.density, baseTextSize * fit));
        if (button.toggle) {
            float dotR = Math.max(2.5f * skin.density, innerMin * 0.065f);
            float dotY = top + height - edge - sunk - Math.max(6f * skin.density, innerMin * 0.16f);
            int color = latched ? skin.pressedRim : PadSkin70.alpha(skin.rim, 0.55f);
            canvas.drawCircle(cx, dotY, dotR, skin.fill(color));
        }
    }
}
