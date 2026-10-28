package com.limelight.binding.video;

import com.limelight.LimeLog;

/**
 * The steps of a decoder recovery once every thread is quiesced, apart from MediaCodec so their
 * order can be tested: restart, then reset, then a decoder set up from scratch, each the fallback
 * for the one before.
 */
final class CodecRecoverySteps {
    private CodecRecoverySteps() {}

    /** What the steps do to the decoder. */
    interface Codec {
        void stop();

        /** Releases the codec and creates a new one of the same name; false when none was created. */
        boolean recreate();

        void reset();

        /** Configures and starts the codec. IllegalArgumentException: the surface is gone. */
        void configure();

        /**
         * Sets the decoder up from scratch. codecFailed: the codec failed its restart and reset and
         * is still to be released; false: it is released already, and no new one could be created.
         */
        void rebuild(boolean codecFailed);
    }

    /**
     * Runs the recovery from a restart, or from a reset when restart is false. recreate replaces
     * the restart's stop() with a new codec. Returns false when the surface is gone and the stream
     * has to stop. An IllegalStateException from rebuild means nothing recovered the decoder.
     */
    static boolean run(boolean restart, boolean recreate, String reason, Codec codec) {
        // Released, with no new codec: there is nothing left to reset.
        boolean released = false;
        // For "recoverable" exceptions, we can just stop, reconfigure, and restart.
        if (restart) {
            LimeLog.warning("Trying to " + (recreate ? "recreate" : "restart") + " decoder after " + reason);
            try {
                if (recreate) {
                    released = !codec.recreate();
                } else {
                    codec.stop();
                }
                if (!released) {
                    codec.configure();
                    return true;
                }
            } catch (IllegalArgumentException e) {
                e.printStackTrace();

                // Our Surface is probably invalid, so just stop
                return false;
            } catch (IllegalStateException e) {
                e.printStackTrace();

                // Something went wrong during the restart, let's use a bigger hammer
                // and try a reset instead.
            }
        }

        // For "non-recoverable" exceptions on L+, we can call reset() to recover
        // without having to recreate the entire decoder again.
        if (!released) {
            LimeLog.warning("Trying to reset decoder after CodecException");
            try {
                codec.reset();
                codec.configure();
                return true;
            } catch (IllegalArgumentException e) {
                e.printStackTrace();

                // Our Surface is probably invalid, so just stop
                return false;
            } catch (IllegalStateException e) {
                e.printStackTrace();

                // Something went wrong during the reset, we'll have to resort to
                // releasing and recreating the decoder now.
            }
        }

        // If we _still_ haven't managed to recover, go for the nuclear option and just
        // throw away the old decoder and reinitialize a new one from scratch.
        LimeLog.warning("Trying to recreate decoder after CodecException");
        try {
            codec.rebuild(!released);
        } catch (IllegalArgumentException e) {
            e.printStackTrace();

            // Our Surface is probably invalid, so just stop
            return false;
        }
        return true;
    }
}
