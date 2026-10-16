package com.limelight.binding.video;

// Sizes for a stream that follows the phone's own resolution while it runs: a foldable opening
// or closing, or a split-screen or desktop window being resized. A Rubylight 2.2 host changes
// size on control message 0x5532 (protocol/core/src/control.rs) and answers with an IDR.
public final class StreamResizePolicy {
    // What the host accepts in 0x5532; it rejects anything else.
    public static final int MIN_DIMENSION = 256;
    public static final int MAX_DIMENSION = 8192;
    public static final int MIN_FPS_MILLIHZ = 10_000;
    public static final int MAX_FPS_MILLIHZ = 500_000;

    public static final int ORIENTATION_OF_AREA = 0;
    public static final int ORIENTATION_LANDSCAPE = 1;
    public static final int ORIENTATION_PORTRAIT = 2;

    private StreamResizePolicy() {}

    // The native stream size for an area of areaWidth x areaHeight window pixels, where each
    // window pixel covers pixelScale panel pixels. The result is turned to the requested
    // orientation, shrunk with its aspect ratio kept until the decoder takes it, and rounded down
    // to even sizes and the decoder's alignment. Null when no size the host accepts fits.
    public static int[] nativeSize(int areaWidth, int areaHeight, float pixelScale, int orientation,
                                   int maxWidth, int maxHeight, int widthAlignment, int heightAlignment) {
        if (areaWidth <= 0 || areaHeight <= 0 || !(pixelScale > 0) || Float.isInfinite(pixelScale)) {
            return null;
        }
        long width = Math.round(areaWidth * (double) pixelScale);
        long height = Math.round(areaHeight * (double) pixelScale);
        if ((orientation == ORIENTATION_LANDSCAPE && width < height) ||
                (orientation == ORIENTATION_PORTRAIT && width > height)) {
            long swap = width;
            width = height;
            height = swap;
        }
        int limitWidth = Math.min(maxWidth, MAX_DIMENSION);
        int limitHeight = Math.min(maxHeight, MAX_DIMENSION);
        if (limitWidth <= 0 || limitHeight <= 0) {
            return null;
        }
        if (width > limitWidth || height > limitHeight) {
            double factor = Math.min((double) limitWidth / width, (double) limitHeight / height);
            width = (long) Math.floor(width * factor);
            height = (long) Math.floor(height * factor);
        }
        width = alignDown(width, widthAlignment);
        height = alignDown(height, heightAlignment);
        if (width < MIN_DIMENSION || height < MIN_DIMENSION) {
            return null;
        }
        return new int[] {(int) width, (int) height};
    }

    // Even, and a multiple of the decoder's alignment.
    static long alignDown(long value, int alignment) {
        long step = 2;
        if (alignment > 1) {
            step = alignment % 2 == 0 ? alignment : 2L * alignment;
        }
        return value - value % step;
    }

    // The stream frame rate, in millihertz, that follows a panel refreshing at up to panelHz.
    public static int fpsMillihz(float panelHz) {
        int fps = DisplayFrameRatePolicy.streamFrameRate(panelHz) * 1000;
        return Math.max(MIN_FPS_MILLIHZ, Math.min(MAX_FPS_MILLIHZ, fps));
    }

    // The frame rate to ask for: unchanged unless it follows the display and the panel's top
    // refresh rate has moved since the stream's rate was set for streamPanelHz.
    public static int targetFpsMillihz(boolean followsDisplay, int currentMillihz, float streamPanelHz, float panelHz) {
        if (!followsDisplay || !(panelHz > 0) || Math.abs(panelHz - streamPanelHz) < 0.5f) {
            return currentMillihz;
        }
        return fpsMillihz(panelHz);
    }

    // The visible picture of a decoder output format; crop values are inclusive, negative when
    // the decoder did not report them.
    public static int[] visibleSize(int width, int height, int cropLeft, int cropRight, int cropTop, int cropBottom) {
        if (cropLeft >= 0 && cropRight >= cropLeft) {
            width = cropRight - cropLeft + 1;
        }
        if (cropTop >= 0 && cropBottom >= cropTop) {
            height = cropBottom - cropTop + 1;
        }
        return new int[] {width, height};
    }

    // Decoders without crop values report sizes padded to the encoder's blocks, up to 64 pixels
    // for AV1 from AMD encoders; read those as the size that was expected.
    public static int[] shownSize(int decodedWidth, int decodedHeight, int expectedWidth, int expectedHeight) {
        if (decodedWidth >= expectedWidth && decodedWidth - expectedWidth < 64 &&
                decodedHeight >= expectedHeight && decodedHeight - expectedHeight < 64) {
            return new int[] {expectedWidth, expectedHeight};
        }
        return new int[] {decodedWidth, decodedHeight};
    }

    // The adaptive-playback maximum that covers both what the decoder was configured for and the
    // size that is coming, so either can arrive first.
    public static int[] coveringMax(int maxWidth, int maxHeight, int width, int height) {
        return new int[] {Math.max(maxWidth, width), Math.max(maxHeight, height)};
    }
}
