package com.limelight;

import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;

/**
 * Whether leaving a stream also quits its game. Only a leave the player asks for (Disconnect, the
 * disconnect shortcut, or Disconnect on the picture-in-picture window) can quit; a lost connection,
 * an error or the host ending the stream never does, so the game is still there to resume. Nor does
 * leaving Dual screen mode's second screen: the game is still on the first.
 */
final class StreamExitPolicy {
    private StreamExitPolicy() {}

    static boolean quitsOnLeave(boolean quitOnLeaveChosen, NvApp.Role role, boolean connected,
                                boolean secondScreen) {
        return quitOnLeaveChosen && connected && role == NvApp.Role.STREAM && !secondScreen;
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
     * Whether the PC itself ended the stream with {@code errorCode}, so it may say why
     * ({@code RustHostLastFailure} in its serverinfo): moonlight-common-c's codes for the PC's
     * termination reasons, and the HRESULTs it passes through unchanged (0x80004005, the generic
     * failure). Lost connections, timeouts, socket errors and this device's own failures are not.
     */
    static boolean hostEndedStream(int errorCode) {
        switch (errorCode) {
            case MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION:
            case MoonBridge.ML_ERROR_PROTECTED_CONTENT:
            case MoonBridge.ML_ERROR_FRAME_CONVERSION:
                return true;
            default:
                // A failure HRESULT (0x8xxxxxxx and up) as a signed int, beyond the small
                // negative codes moonlight-common-c and ENet use (-1, -100 to -107).
                return errorCode < -0xFFFF;
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
