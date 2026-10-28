package com.limelight.binding.video;

/**
 * Paces decoder recoveries that keep failing before the decoder puts out a frame. A decoder that
 * is out of memory fails every reset at once (a TV's HEVC decoder failed ten in 2 s), so each
 * recovery in a row without a frame waits longer, and then the decoder is given up: the stream
 * ends and says why.
 */
final class DecoderRecoveryBackoff {
    /** The wait before the 1st, 2nd... recovery in a row without a frame; one more gives up. */
    static final long[] DELAYS_MS = {0, 250, 500, 1000, 2000};

    private final long[] delaysMs;
    private int framesOutAtLastRecovery;
    private int withoutFrame;

    DecoderRecoveryBackoff() {
        this(DELAYS_MS);
    }

    DecoderRecoveryBackoff(long[] delaysMs) {
        this.delaysMs = delaysMs;
    }

    /**
     * Before a recovery, with the frames the decoder has put out so far: how long to wait first,
     * or -1 when that many recoveries in a row brought no frame and the decoder is given up.
     */
    long beforeRecovery(int framesOut) {
        withoutFrame = framesOut == framesOutAtLastRecovery ? withoutFrame + 1 : 1;
        framesOutAtLastRecovery = framesOut;
        return withoutFrame > delaysMs.length ? -1 : delaysMs[withoutFrame - 1];
    }

    /** The recoveries in a row that brought no frame, the one just asked about included. */
    int recoveriesWithoutFrame() {
        return withoutFrame;
    }
}
