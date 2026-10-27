package com.limelight.nvstream.jni;

/**
 * Which streaming engine the last stream used and, when the Rust engine was requested but the C
 * engine ran, why. Shown in the stream overlays and the problem report, so a comparison never has
 * to guess which engine produced a timing CSV. Kept apart from MoonBridge, whose static
 * initializer loads the C library, so reports and tests can read it without native code.
 */
public final class StreamEngine {
    private StreamEngine() {}

    private static volatile String summary = "no stream started in this app process";
    private static volatile boolean rust;

    /** Records the engine chosen for the stream that is starting. */
    static void record(boolean rustRequested, boolean rustUsed, String fallbackReason) {
        rust = rustUsed;
        summary = describe(rustRequested, rustUsed, fallbackReason);
    }

    /** "Rust (nightjar-core)", or the C engine with the reason when Rust was requested. */
    public static String summary() {
        return summary;
    }

    /** A short label for the overlay's first line: "Rust", "C", or "C (Rust fell back)". */
    public static String label() {
        return rust ? "Rust" : fellBack() ? "C (Rust fell back)" : "C";
    }

    public static boolean isRust() {
        return rust;
    }

    /** Whether the Rust engine was wanted but the C engine runs. */
    public static boolean fellBack() {
        return !rust && summary.contains("requested but unavailable");
    }

    static String describe(boolean rustRequested, boolean rustUsed, String fallbackReason) {
        if (rustUsed) {
            return "Rust (nightjar-core)";
        }
        if (rustRequested) {
            return "C (moonlight-common-c), Rust engine requested but unavailable: " + fallbackReason;
        }
        return "C (moonlight-common-c)";
    }
}
