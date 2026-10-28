package com.limelight.binding.video;

import android.view.Surface;

import com.limelight.LimeLog;

/**
 * Asks a stream's output window whether it takes a dataspace before the host is asked for it
 * (jni/window-dataspace): the public ANativeWindow_setBuffersDataSpace first, then, where that
 * refuses it, ANativeWindow::perform(NATIVE_WINDOW_SET_BUFFERS_DATASPACE), which skips the public
 * call's list. The window's dataspace is put back afterwards. Whether a display takes HLG is this
 * answer, per device, never the Android version: the list differs between Android builds.
 * PyroWave asks it in MediaCodecDecoderRenderer.pickHdrFormat; any renderer that draws HDR through
 * the window dataspace can call probeHlg with its Surface the same way.
 */
public final class WindowDataspace {
    private WindowDataspace() {}

    /** ADATASPACE_BT2020_HLG */
    public static final int BT2020_HLG = 168165376;

    // Which call took the dataspace (nwds::DataspaceCall)
    public static final int CALL_NONE = 0;
    public static final int CALL_PUBLIC = 1;
    public static final int CALL_PERFORM = 2;
    // window_dataspace.h results that are never the platform's (those are 0 or a negative errno)
    static final int NOT_CALLED = 1;
    static final int MISSING = 2;
    static final int NOT_MIRRORED = 3;

    private static Boolean loaded;

    /** What one probe found, with a log line naming the call that answered. */
    public static final class Probe {
        /** CALL_PUBLIC, CALL_PERFORM or CALL_NONE when neither took it */
        public final int call;
        private final String unavailable;
        private final int publicResult;
        private final int performResult;
        private final int previous;
        private final int restorePublic;
        private final int restorePerform;

        /**
         * @param call   which call took it (CALL_*)
         * @param detail the public call's result, perform's, the previous dataspace (-1 when the
         *               window did not say), and the two results of putting it back
         */
        Probe(int call, int[] detail) {
            this.call = call == CALL_PUBLIC || call == CALL_PERFORM ? call : CALL_NONE;
            unavailable = null;
            publicResult = detail[0];
            performResult = detail[1];
            previous = detail[2];
            restorePublic = detail[3];
            restorePerform = detail[4];
        }

        /** Nothing was asked: the window cannot be reached (no surface, no native library). */
        Probe(String why) {
            call = CALL_NONE;
            unavailable = why;
            publicResult = performResult = restorePublic = restorePerform = NOT_CALLED;
            previous = -1;
        }

        public boolean accepted() {
            return call != CALL_NONE;
        }

        public String describe() {
            if (unavailable != null) return "the window was not asked (" + unavailable + ")";
            if (publicResult == NOT_CALLED) return "the window was not asked (the Surface gave no ANativeWindow)";
            StringBuilder text = new StringBuilder("ANativeWindow_setBuffersDataSpace ")
                    .append(publicResult == 0 ? "took BT2020_HLG" : "refused BT2020_HLG (" + result(publicResult) + ")");
            if (publicResult != 0) {
                text.append(performResult == NOT_MIRRORED ?
                        "; ANativeWindow::perform was not called: the window is not the platform's ANativeWindow" :
                        "; ANativeWindow::perform(NATIVE_WINDOW_SET_BUFFERS_DATASPACE) " +
                        (performResult == 0 ? "took it" : "refused it (" + result(performResult) + ")"));
            }
            if (accepted()) {
                String what = previous >= 0 ? "dataspace 0x" + Integer.toHexString(previous) :
                        "UNKNOWN (the window did not say its dataspace)";
                text.append(restorePublic == 0 || restorePerform == 0 ? "; " + what + " put back" :
                        "; putting " + what + " back failed (" + result(restorePublic) + ", perform " +
                        result(restorePerform) + ")");
            }
            return text.toString();
        }

        private static String result(int result) {
            switch (result) {
                case MISSING: return "missing";
                case NOT_CALLED: return "not called";
                case NOT_MIRRORED: return "not the platform's window";
                default: return Integer.toString(result);
            }
        }
    }

    /** Whether the window behind the surface takes BT2020_HLG, leaving its dataspace as it was. */
    public static Probe probeHlg(Surface surface) {
        if (surface == null) return new Probe("no surface");
        if (!load()) return new Probe("libwindow-dataspace not loaded");
        int[] detail = {NOT_CALLED, NOT_CALLED, -1, NOT_CALLED, NOT_CALLED};
        return new Probe(nativeProbe(surface, BT2020_HLG, detail), detail);
    }

    private static synchronized boolean load() {
        if (loaded == null) {
            try {
                System.loadLibrary("window-dataspace");
                loaded = true;
            } catch (UnsatisfiedLinkError e) {
                LimeLog.severe("Window dataspace probe unavailable: " + e.getMessage());
                loaded = false;
            }
        }
        return loaded;
    }

    private static native int nativeProbe(Surface surface, int dataspace, int[] detail);
}
