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
    // Ignore float/reporting noise without merging distinct 59.94/60 or 119.88/120 modes.
    private static final int PANEL_RATE_TOLERANCE_MILLIHZ = 10;

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
        long fps = Math.round(panelHz * 1000d);
        return (int) Math.max(MIN_FPS_MILLIHZ, Math.min(MAX_FPS_MILLIHZ, fps));
    }

    // The frame rate to ask for: unchanged unless it follows the display and the panel's top
    // refresh rate has moved since the stream's rate was set for streamPanelHz.
    public static int targetFpsMillihz(boolean followsDisplay, int currentMillihz, float streamPanelHz, float panelHz) {
        if (!followsDisplay || !(panelHz > 0) || !Float.isFinite(panelHz)) {
            return currentMillihz;
        }
        if (streamPanelHz > 0 && Float.isFinite(streamPanelHz) &&
                Math.abs(Math.round(panelHz * 1000d) - Math.round(streamPanelHz * 1000d)) <
                        PANEL_RATE_TOLERANCE_MILLIHZ) {
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

    // Decoders without crop values report sizes padded to the encoder's blocks, up to 63 columns
    // and 15 rows for AV1 from AMD encoders; read those as the size that was expected.
    public static int[] shownSize(int decodedWidth, int decodedHeight, int expectedWidth, int expectedHeight) {
        return paddedVisibleSize(decodedWidth, decodedHeight, new int[][] {{expectedWidth, expectedHeight}});
    }

    /**
     * The decoded size the stream view enlarges its video by against the shown size: the padded
     * picture straight from the decoder, but the shown size when the GLES upscaler samples only
     * the stream's part and fills the surface with it (enlarging that would cut the stream off and
     * resize the upscaler's surface).
     */
    public static int[] viewPaddedSize(int[] decoded, int[] shown, boolean upscalerCrops) {
        return upscalerCrops ? shown : decoded;
    }

    /** The most columns an encoder's alignment adds (AMD AV1: up to a multiple of 64). */
    static final int PADDING_COLUMNS = 64;
    /** The most rows an encoder's alignment adds (AMD AV1 and H.264 macroblocks: a multiple of 16). */
    static final int PADDING_ROWS = 16;

    /**
     * nightjar-core's video::crop::visible_size, which desktop Nightjar crops by: of the requested
     * stream sizes, the one the decoded picture is plus alignment padding of, needing the least
     * padding, or the whole picture. src/test/resources/protocol/visible-size.txt holds the cases
     * both sides check.
     */
    static int[] paddedVisibleSize(int decodedWidth, int decodedHeight, int[][] requested) {
        int[] best = null;
        for (int[] size : requested) {
            if (!padded(decodedWidth, size[0], PADDING_COLUMNS) || !padded(decodedHeight, size[1], PADDING_ROWS)) continue;
            if (best == null || decodedWidth - size[0] + decodedHeight - size[1] <
                    decodedWidth - best[0] + decodedHeight - best[1]) {
                best = size;
            }
        }
        return best == null ? new int[] {decodedWidth, decodedHeight} : new int[] {best[0], best[1]};
    }

    private static boolean padded(int extent, int asked, int most) {
        return asked > 0 && extent >= asked && extent < asked + most;
    }

    // The adaptive-playback maximum that covers both what the decoder was configured for and the
    // size that is coming, so either can arrive first.
    public static int[] coveringMax(int maxWidth, int maxHeight, int width, int height) {
        return new int[] {Math.max(maxWidth, width), Math.max(maxHeight, height)};
    }

    /** android.view.Display.DEFAULT_DISPLAY: the display a fold or unfold moves between panels. */
    static final int DEFAULT_DISPLAY_ID = 0;

    /**
     * The panel sizes for coveringSize, from the displays the app sees: ids[i] with the {width,
     * height} of each of its modes in modes[i], builtIn[i] for a display built into the device
     * (DisplayManager.DISPLAY_CATEGORY_BUILT_IN_DISPLAYS, which also lists one that is off). The
     * stream's own display (videoId) always counts. On the default display, which a fold or unfold
     * moves between panels, every built-in display counts too: on a foldable whose panels are
     * separate displays (Galaxy Z Fold7: cover and inner screen, the one not in use disabled), the
     * stream's display lists only the panel it is on. External, wireless and virtual displays never
     * count, nor do other displays for a stream shown elsewhere (Dual screen's second stream).
     */
    public static int[][] coveringPanels(int videoId, int[] ids, boolean[] builtIn, int[][][] modes) {
        java.util.List<int[]> panels = new java.util.ArrayList<>();
        for (int i = 0; i < ids.length; i++) {
            if (ids[i] == videoId || (videoId == DEFAULT_DISPLAY_ID && builtIn[i])) {
                java.util.Collections.addAll(panels, modes[i]);
            }
        }
        return panels.toArray(new int[0][]);
    }

    /**
     * The size covering what a stream switches to on its own, for the decoder's maximum up front;
     * null when nothing is known. A stream that turns with the device takes either orientation of
     * its launch size. A foldable's stream at its own resolution takes any of its panels' sizes, in
     * the launch orientation, or in either when it also turns. Sizes are {width, height}.
     */
    public static int[] coveringSize(int launchWidth, int launchHeight, boolean turns, int[][] panels) {
        int[] covering = null;
        if (turns) {
            int side = Math.max(launchWidth, launchHeight);
            covering = new int[] {side, side};
        }
        if (panels != null) {
            boolean landscape = launchWidth >= launchHeight;
            for (int[] panel : panels) {
                if (panel[0] <= 0 || panel[1] <= 0) continue;
                int longSide = Math.max(panel[0], panel[1]);
                int shortSide = Math.min(panel[0], panel[1]);
                int[] size = turns ? new int[] {longSide, longSide} :
                        landscape ? new int[] {longSide, shortSide} : new int[] {shortSide, longSide};
                covering = covering == null ? size : coveringMax(covering[0], covering[1], size[0], size[1]);
            }
        }
        return covering;
    }
}
