package com.limelight.preferences;

import android.graphics.Canvas;
import android.graphics.RectF;

import com.limelight.ui.VideoScaling;

/**
 * Video size's preview picture: a 16:9 screen (neutral) with a wider picture in it (amber, with a circle that shows
 * its shape). Fit shows all of it with bars above and below, Fill covers the screen and loses the sides (ghosted
 * outside the screen), Stretch covers it and squeezes the circle, and 1:1 pixels shows a smaller picture with a
 * border all round. Illustrative only.
 */
final class PreviewScaling {
    static final float SCREEN = 16f / 9f, PICTURE = 21f / 9f;
    /** A stream smaller than the screen, one stream pixel per screen pixel. */
    static final float NATIVE_WIDTH = 0.62f;
    private static final float CIRCLE = 0.3f;

    private PreviewScaling() { }

    /** The picture's width as a share of the screen's, for a VideoScaling mode. */
    static float width(int mode) {
        switch (mode) {
            case VideoScaling.FILL:
                return PICTURE / SCREEN;
            case VideoScaling.NATIVE:
                return NATIVE_WIDTH;
            case VideoScaling.SAFE_FIT:
                return 0.84f;
            default:
                return 1f;
        }
    }

    /** The picture's height as a share of the screen's. */
    static float height(int mode) {
        switch (mode) {
            case VideoScaling.FILL:
            case VideoScaling.STRETCH:
                return 1f;
            case VideoScaling.NATIVE:
                return NATIVE_WIDTH * SCREEN / PICTURE;
            case VideoScaling.SAFE_FIT:
                return 0.84f * SCREEN / PICTURE;
            default:
                return SCREEN / PICTURE;
        }
    }

    /** How much wider than tall a circle in the picture is drawn: 1 while the picture keeps its shape. */
    static float roundness(int mode) {
        return width(mode) * SCREEN / (height(mode) * PICTURE);
    }

    static void draw(Canvas canvas, RectF box, int from, int to, float t, PreviewInks inks) {
        // Room at the sides for what Fill cuts off.
        float screenHeight = Math.min(box.height() * 0.86f, box.width() / PICTURE);
        float screenWidth = screenHeight * SCREEN;
        float cx = box.centerX(), cy = box.centerY();
        float left = cx - screenWidth / 2f, top = cy - screenHeight / 2f, right = left + screenWidth, bottom = top + screenHeight;
        float width = PreviewInks.lerp(width(from), width(to), t) * screenWidth;
        float height = PreviewInks.lerp(height(from), height(to), t) * screenHeight;
        float radius = 3f * inks.dp;
        if (!inks.linesOnly) canvas.drawRoundRect(left, top, right, bottom, radius, radius, inks.fill(inks.neutral, 0.14f));
        if (width > screenWidth + inks.dp || height > screenHeight + inks.dp) {
            canvas.drawRect(cx - width / 2f, cy - height / 2f, cx + width / 2f, cy + height / 2f, inks.line(inks.accent, 0.4f, 1f));
        }
        canvas.save();
        canvas.clipRect(left, top, right, bottom);
        if (!inks.linesOnly) {
            canvas.drawRect(cx - width / 2f, cy - height / 2f, cx + width / 2f, cy + height / 2f, inks.fill(inks.accent, 0.24f));
        }
        canvas.drawRect(cx - width / 2f, cy - height / 2f, cx + width / 2f, cy + height / 2f, inks.line(inks.accent, 1f, 1.5f));
        float rx = CIRCLE * width / PICTURE, ry = CIRCLE * height;
        canvas.drawOval(cx - rx, cy - ry, cx + rx, cy + ry, inks.line(inks.accent, 1f, 1.5f));
        canvas.restore();
        canvas.drawRoundRect(left, top, right, bottom, radius, radius, inks.line(inks.neutral, 0.9f, 1.5f));
    }
}
