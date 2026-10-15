package com.limelight.binding.input.touch;

import android.view.Choreographer;
import android.view.View;

import com.limelight.nvstream.NvConnection;
import com.limelight.ui.DesktopViewport;
import com.limelight.ui.LocalCursorView;
import com.limelight.ui.VideoScaling;

/**
 * Where relative finger movement goes: to the host as mouse moves, or, in Local cursor mode, to a
 * pointer drawn on this device whose absolute position is sent to the host. Also runs fling
 * momentum on the UI thread's frame callbacks, only while a glide is in progress.
 */
public final class TrackpadPointer {
    private final NvConnection conn;
    private final View streamView;
    private final LocalCursorView cursorView;
    private final boolean absoluteMouseMode;
    private final PointerAcceleration acceleration;
    private final TouchMomentum momentum;
    private DesktopViewport viewport;
    private boolean localCursor;
    private boolean cursorShown;
    private boolean hostCursorHidden;
    private int referenceWidth = 1, referenceHeight = 1;
    private float cursorX, cursorY;
    private boolean frameScheduled;

    private final Choreographer.FrameCallback momentumFrame = new Choreographer.FrameCallback() {
        @Override
        public void doFrame(long frameTimeNanos) {
            frameScheduled = false;
            boolean more = momentum.step(frameTimeNanos / 1_000_000L);
            int dx = momentum.stepX(), dy = momentum.stepY();
            if (dx != 0 || dy != 0) moveBy(dx, dy);
            if (more) scheduleFrame();
        }
    };

    public TrackpadPointer(NvConnection conn, View streamView, LocalCursorView cursorView, boolean absoluteMouseMode,
                           PointerAcceleration acceleration, TouchMomentum momentum) {
        this.conn = conn;
        this.streamView = streamView;
        this.cursorView = cursorView;
        this.absoluteMouseMode = absoluteMouseMode;
        this.acceleration = acceleration;
        this.momentum = momentum;
    }

    public void setViewport(DesktopViewport viewport) {
        this.viewport = viewport;
    }

    /** Null when acceleration is off. */
    public PointerAcceleration acceleration() {
        return acceleration;
    }

    public boolean hasMomentum() {
        return momentum != null;
    }

    public boolean isLocalCursor() {
        return localCursor;
    }

    public void setReferenceSize(int width, int height) {
        if (width <= 0 || height <= 0) return;
        if (width != referenceWidth || height != referenceHeight) {
            cursorX = cursorX / referenceWidth * width;
            cursorY = cursorY / referenceHeight * height;
            referenceWidth = width;
            referenceHeight = height;
        }
    }

    /** Turns Local cursor mode on or off; the pointer starts at the centre of the stream. */
    public void setLocalCursor(boolean enabled) {
        stopMomentum();
        if (enabled && !localCursor) {
            cursorX = referenceWidth / 2f;
            cursorY = referenceHeight / 2f;
        }
        localCursor = enabled;
        setCursorShown(enabled);
    }

    /** Hides the drawn pointer (for example in picture-in-picture) without leaving the mode. */
    public void setCursorShown(boolean shown) {
        cursorShown = shown && localCursor;
        if (cursorView == null) return;
        cursorView.setVisibility(cursorShown && !hostCursorHidden ? View.VISIBLE : View.GONE);
        if (cursorShown) syncCursor();
    }

    /** The host hid its cursor (host cursor shapes): draw nothing until it shows one again. */
    public void setHostCursorHidden(boolean hidden) {
        hostCursorHidden = hidden;
        setCursorShown(cursorShown);
    }

    /** Zoom of the desktop view, so the drawn pointer can follow the finger one to one. */
    public float zoomScale() {
        return viewport != null ? viewport.scale() : 1;
    }

    /** A new gesture's first finger landed. */
    public void touchDown() {
        stopMomentum();
        if (localCursor) {
            // The host pointer may have been moved by a mouse or the host itself since.
            sendCursorPosition();
        }
    }

    public void moveBy(int dx, int dy) {
        if (localCursor) {
            cursorX = Math.max(0, Math.min(referenceWidth - 1, cursorX + dx));
            cursorY = Math.max(0, Math.min(referenceHeight - 1, cursorY + dy));
            sendCursorPosition();
            syncCursor();
        } else if (absoluteMouseMode) {
            conn.sendMouseMoveAsMousePosition((short) dx, (short) dy, (short) referenceWidth, (short) referenceHeight);
        } else {
            conn.sendMouseMove((short) dx, (short) dy);
        }
    }

    public void track(int dx, int dy, long timeMs) {
        if (momentum != null) momentum.track(dx, dy, timeMs);
    }

    /** The finger that moved the pointer lifted at timeMs after a plain move. */
    public void release(long timeMs) {
        if (momentum != null && momentum.release(timeMs)) scheduleFrame();
    }

    public void stopMomentum() {
        if (momentum != null) momentum.stop();
        if (frameScheduled) {
            frameScheduled = false;
            Choreographer.getInstance().removeFrameCallback(momentumFrame);
        }
    }

    public boolean isGliding() {
        return momentum != null && momentum.isActive();
    }

    /** Places the drawn pointer over the stream pixel it points at, including desktop zoom and pan. */
    public void syncCursor() {
        if (!cursorShown || cursorView == null) return;
        float scale = 1, panX = 0, panY = 0;
        if (viewport != null) {
            scale = viewport.scale();
            panX = viewport.x();
            panY = viewport.y();
        }
        // The stream view is the video's rectangle in every scaling mode, so this holds for all of them.
        float x = VideoScaling.toScreen(cursorX, referenceWidth, streamView.getX(), streamView.getWidth(), scale, panX);
        float y = VideoScaling.toScreen(cursorY, referenceHeight, streamView.getY(), streamView.getHeight(), scale, panY);
        cursorView.moveTo(x, y);
    }

    float cursorX() {
        return cursorX;
    }

    float cursorY() {
        return cursorY;
    }

    private void sendCursorPosition() {
        conn.sendMousePosition((short) cursorX, (short) cursorY, (short) referenceWidth, (short) referenceHeight);
    }

    private void scheduleFrame() {
        if (frameScheduled) return;
        frameScheduled = true;
        Choreographer.getInstance().postFrameCallback(momentumFrame);
    }
}
