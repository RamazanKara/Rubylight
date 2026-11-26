package com.limelight.ui;

/** Free zoom/pan transform across container borders and edges, with a matching inverse for absolute input. */
public final class DesktopViewport {
    public static final float MIN_SCALE = 0.5f;
    public static final float MAX_SCALE = 4.0f;
    private static final float MIN_VISIBLE_FRACTION = 0.2f;

    private float width = 1, height = 1;
    private float containerWidth = 1, containerHeight = 1;
    private float offsetX, offsetY;
    private float scale = 1, x, y;

    public float scale() { return scale; }
    public float x() { return x; }
    public float y() { return y; }
    public boolean isTransformed() { return scale != 1f || x != 0f || y != 0f; }

    public void resize(float width, float height) {
        setBounds(width, height, width, height, 0, 0);
    }

    public void setBounds(float width, float height, float containerWidth, float containerHeight,
                          float offsetX, float offsetY) {
        if (!(width > 0 && height > 0 && containerWidth > 0 && containerHeight > 0) ||
                !Float.isFinite(width) || !Float.isFinite(height) ||
                !Float.isFinite(containerWidth) || !Float.isFinite(containerHeight) ||
                !Float.isFinite(offsetX) || !Float.isFinite(offsetY)) {
            return;
        }
        x *= width / this.width;
        y *= height / this.height;
        this.width = width;
        this.height = height;
        this.containerWidth = containerWidth;
        this.containerHeight = containerHeight;
        this.offsetX = offsetX;
        this.offsetY = offsetY;
        clamp();
    }

    public void reset() { scale = 1; x = y = 0; }

    public void zoom(float factor, float focusX, float focusY) {
        zoom(factor, focusX, focusY, MIN_SCALE, MAX_SCALE);
    }

    public void zoom(float factor, float focusX, float focusY, float minScale, float maxScale) {
        if (!Float.isFinite(factor) || factor <= 0 || !Float.isFinite(focusX) || !Float.isFinite(focusY) ||
                !Float.isFinite(minScale) || !Float.isFinite(maxScale) || minScale <= 0 || minScale > maxScale) {
            return;
        }
        float low = Math.max(MIN_SCALE, minScale);
        float high = Math.min(MAX_SCALE, maxScale);
        if (low > high) return;
        float next = Math.max(low, Math.min(high, scale * factor));
        float ratio = next / scale;
        x = focusX - (focusX - x) * ratio;
        y = focusY - (focusY - y) * ratio;
        scale = next;
        clamp();
    }

    public void pan(float dx, float dy) {
        if (!Float.isFinite(dx) || !Float.isFinite(dy)) return;
        x += dx;
        y += dy;
        clamp();
    }

    public float inputX(float screenX) { return Math.max(0, Math.min(width, (screenX - x) / scale)); }
    public float inputY(float screenY) { return Math.max(0, Math.min(height, (screenY - y) / scale)); }

    private void clamp() {
        float scaledW = width * scale;
        float scaledH = height * scale;
        float keepX = Math.min(scaledW, containerWidth) * MIN_VISIBLE_FRACTION;
        float keepY = Math.min(scaledH, containerHeight) * MIN_VISIBLE_FRACTION;
        float minX = -offsetX + keepX - scaledW;
        float maxX = containerWidth - offsetX - keepX;
        float minY = -offsetY + keepY - scaledH;
        float maxY = containerHeight - offsetY - keepY;
        x = Math.max(minX, Math.min(maxX, x));
        y = Math.max(minY, Math.min(maxY, y));
    }
}
