package com.limelight.ui;

import android.content.pm.ActivityInfo;

import com.limelight.binding.video.StreamResizePolicy;

/**
 * Which way the stream window may turn, and the stream's shape when it does. Automatic keeps the
 * long-standing rule: phones in landscape; tablets, desktop-class devices, squarish screens,
 * windows and video on another display in any orientation. Landscape and Portrait lock the window;
 * Rotate follows the device within the system rotation lock.
 * <p>
 * The stream itself turns only where the host draws it at the shape asked for, on a host virtual
 * display. It then starts in the window's shape, and while it runs a host that takes in-stream
 * reconfigure is asked for the turned size. Elsewhere the stream keeps its size and the video
 * scaling places it in the turned window.
 */
public final class StreamOrientation {
    public static final String PREF = "stream_orientation";

    // Positions in the stream_orientation_values array.
    public static final int AUTO = 0;
    public static final int LANDSCAPE = 1;
    public static final int PORTRAIT = 2;
    public static final int ROTATE = 3;

    // A turn waits longer than a fold or window change before the host is asked for a new size, so
    // the rotation animation, or a phone turned over and straight back, sends nothing.
    public static final long ROTATION_SETTLE_MS = 1000;

    private static final String[] VALUES = {"auto", "landscape", "portrait", "rotate"};

    private StreamOrientation() {}

    /** A stored value; anything unknown (missing, newer or damaged) is Automatic. */
    public static int fromValue(String stored) {
        if (stored != null) {
            for (int i = 0; i < VALUES.length; i++) {
                if (VALUES[i].equals(stored)) return i;
            }
        }
        return AUTO;
    }

    public static String value(int mode) {
        return VALUES[mode];
    }

    /**
     * The orientation to request for the stream window, as an {@code ActivityInfo} constant.
     *
     * @param freeWindow  the app cannot turn the window: multi-window, or video on another display
     * @param largeScreen smallest width of 600 dp or more, or a desktop-class device
     * @param squarish    the panel's long side is less than 1.3 times its short side
     * @param nativeResolution the stream follows the panel's own size
     */
    public static int requested(int mode, boolean freeWindow, boolean largeScreen, boolean squarish,
                                boolean onscreenController, boolean nativeResolution,
                                int streamWidth, int streamHeight) {
        if (freeWindow) {
            return ActivityInfo.SCREEN_ORIENTATION_FULL_USER;
        }
        switch (mode) {
            case LANDSCAPE:
                return ActivityInfo.SCREEN_ORIENTATION_USER_LANDSCAPE;
            case PORTRAIT:
                return ActivityInfo.SCREEN_ORIENTATION_USER_PORTRAIT;
            case ROTATE:
                return ActivityInfo.SCREEN_ORIENTATION_FULL_USER;
            default:
                break;
        }
        if (largeScreen) {
            return ActivityInfo.SCREEN_ORIENTATION_FULL_USER;
        }
        if (!squarish) {
            return ActivityInfo.SCREEN_ORIENTATION_USER_LANDSCAPE;
        }
        // A native stream keeps the shape it was asked for; on-screen controls are laid out for landscape.
        if (nativeResolution) {
            return streamWidth > streamHeight ? ActivityInfo.SCREEN_ORIENTATION_USER_LANDSCAPE :
                    ActivityInfo.SCREEN_ORIENTATION_USER_PORTRAIT;
        }
        return onscreenController ? ActivityInfo.SCREEN_ORIENTATION_USER_LANDSCAPE :
                ActivityInfo.SCREEN_ORIENTATION_FULL_USER;
    }

    /**
     * The size to launch with. Portrait, or Rotate in a portrait window, starts a host that draws
     * the stream on its virtual display in portrait; Rotate in a landscape window starts it in
     * landscape. Automatic, Landscape and hosts that only scale their own screen keep the size.
     */
    public static int[] launchSize(int mode, boolean hostShapesStream, boolean windowPortrait, int width, int height) {
        boolean turn = false;
        if (hostShapesStream && width != height) {
            if (mode == PORTRAIT) {
                turn = width > height;
            } else if (mode == ROTATE) {
                turn = windowPortrait == (width > height);
            }
        }
        return turn ? new int[] {height, width} : new int[] {width, height};
    }

    /** Whether a turned window asks the host for a turned stream. */
    public static boolean followsRotation(int mode, boolean hostShapesStream) {
        return mode == ROTATE && hostShapesStream;
    }

    /**
     * The {@link StreamResizePolicy} orientation of a full-screen stream while it runs: the window's
     * while the stream follows rotation, otherwise the one it started in.
     */
    public static int streamOrientation(int mode, boolean hostShapesStream, boolean startedLandscape,
                                        boolean windowLandscape) {
        boolean landscape = followsRotation(mode, hostShapesStream) ? windowLandscape : startedLandscape;
        return landscape ? StreamResizePolicy.ORIENTATION_LANDSCAPE : StreamResizePolicy.ORIENTATION_PORTRAIT;
    }

    /**
     * A fixed stream size turned to the window's shape, shrunk with its aspect ratio kept until the
     * decoder takes it and rounded down to even sizes and the decoder's alignment. Null when no
     * size the host accepts fits.
     */
    public static int[] turnedSize(int width, int height, boolean landscape, int maxWidth, int maxHeight,
                                   int widthAlignment, int heightAlignment) {
        return StreamResizePolicy.nativeSize(width, height, 1,
                landscape ? StreamResizePolicy.ORIENTATION_LANDSCAPE : StreamResizePolicy.ORIENTATION_PORTRAIT,
                maxWidth, maxHeight, widthAlignment, heightAlignment);
    }

    /** How long a change waits before the host is asked for a new size. */
    public static long settleMs(int mode, boolean hostShapesStream, boolean turned, long defaultMs) {
        return turned && followsRotation(mode, hostShapesStream) ? Math.max(defaultMs, ROTATION_SETTLE_MS) : defaultMs;
    }
}
