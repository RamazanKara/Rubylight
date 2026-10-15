package com.limelight;

import com.limelight.nvstream.http.NvApp;

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
}
