package com.limelight;

import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;

/**
 * Whether leaving a stream also quits its game. Only a leave the player asks for (Disconnect, the
 * disconnect shortcut, or Disconnect on the picture-in-picture window) can quit; a lost connection,
 * an error or the host ending the stream never does, so the game is still there to resume.
 */
final class StreamExitPolicy {
    private StreamExitPolicy() {}

    static boolean quitsOnLeave(boolean quitOnLeaveChosen, NvApp.Role role, boolean connected) {
        return quitOnLeaveChosen && connected && role == NvApp.Role.STREAM;
    }

    /**
     * Whether a stream that ended with {@code errorCode} stops on a dialog the player has to read.
     * That is only an error that comes with advice: the port test says the network blocks the
     * stream, no video ever arrived (the ports to check), no complete frame (lower the bitrate),
     * the PC could not start or encode the stream (protected content, frame conversion), or this
     * device's video decoder failed (what to change before starting again). The
     * host ending the stream or going away, a lost connection, a timeout and a reason only the PC
     * knows leave nothing to do here: the stream closes, the player is back in the list, and the
     * reason shows as a message that outlives the screen. Two more cases keep the dialog: the
     * network grace period may have run out (its text advises raising it), and automatic
     * reconnecting used up its attempts (the dialog has the Reconnect button).
     */
    static boolean terminationNeedsAction(int errorCode, boolean portTestBlocked,
                                          boolean networkGraceGaveUp, boolean recoveryExhausted) {
        if (errorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION) return false;
        if (portTestBlocked || networkGraceGaveUp || recoveryExhausted) return true;
        switch (errorCode) {
            case MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC:
            case MoonBridge.ML_ERROR_NO_VIDEO_FRAME:
            case MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION:
            case MoonBridge.ML_ERROR_PROTECTED_CONTENT:
            case MoonBridge.ML_ERROR_FRAME_CONVERSION:
            case MoonBridge.CLIENT_ERROR_DECODER:
                return true;
            default:
                return false;
        }
    }

    /**
     * The text of the dialog for a decoder that was given up: with "Render through Vulkan" on, it
     * names that setting, as the Vulkan path is what a decoder short of memory may not manage.
     */
    static int decoderFailureMessage(boolean withVulkan) {
        return withVulkan ? R.string.decoder_failed_vulkan : R.string.decoder_failed;
    }
}
