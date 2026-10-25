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
     * or the PC could not start or encode the stream (protected content, frame conversion). The
     * host ending the stream or going away, a lost connection, a timeout and a reason only the PC
     * knows leave nothing to do here: the stream closes, the player is back in the list, and the
     * reason shows as a message that outlives the screen.
     */
    static boolean terminationNeedsAction(int errorCode, boolean portTestBlocked) {
        if (errorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION) return false;
        if (portTestBlocked) return true;
        switch (errorCode) {
            case MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC:
            case MoonBridge.ML_ERROR_NO_VIDEO_FRAME:
            case MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION:
            case MoonBridge.ML_ERROR_PROTECTED_CONTENT:
            case MoonBridge.ML_ERROR_FRAME_CONVERSION:
                return true;
            default:
                return false;
        }
    }
}
