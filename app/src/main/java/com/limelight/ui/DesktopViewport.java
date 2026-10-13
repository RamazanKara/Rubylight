package com.limelight.ui;

/** Bounded zoom/pan transform, in view pixels, with a matching inverse for absolute input. */
public final class DesktopViewport {
    private float width = 1, height = 1, scale = 1, x, y;
    public float scale() { return scale; }
    public float x() { return x; }
    public float y() { return y; }
    public void resize(float width, float height) {
        if (!(width > 0 && height > 0)) return;
        x *= width / this.width; y *= height / this.height;
        this.width = width; this.height = height; clamp();
    }
    public void reset() { scale = 1; x = y = 0; }
    public void zoom(float factor, float focusX, float focusY) {
        if (!Float.isFinite(factor) || factor <= 0 || !Float.isFinite(focusX) || !Float.isFinite(focusY)) return;
        float next = Math.max(1, Math.min(4, scale * factor));
        float ratio = next / scale;
        x = focusX - (focusX - x) * ratio;
        y = focusY - (focusY - y) * ratio;
        scale = next; clamp();
    }
    public void pan(float dx, float dy) {
        if (!Float.isFinite(dx) || !Float.isFinite(dy)) return;
        x += dx; y += dy; clamp();
    }
    public float inputX(float screenX) { return Math.max(0, Math.min(width, (screenX - x) / scale)); }
    public float inputY(float screenY) { return Math.max(0, Math.min(height, (screenY - y) / scale)); }
    private void clamp() {
        x = Math.max(width * (1 - scale), Math.min(0, x));
        y = Math.max(height * (1 - scale), Math.min(0, y));
    }
}
