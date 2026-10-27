package com.limelight.ui;

import com.limelight.nvstream.http.NvApp;

public final class PictureInPicturePolicy {
    public static boolean shouldAutoEnter(boolean connected, NvApp.Role role, int suppressCount, boolean multiWindow) {
        return connected && role != NvApp.Role.INPUT_ONLY && suppressCount == 0 && !multiWindow;
    }

    public static int[] aspectRatio(int width, int height) {
        // Android rejects ratios outside [1 / 2.39, 2.39], including valid ultrawide stream profiles.
        if ((long) width * 100 > (long) height * 239) return new int[] {239, 100};
        if ((long) height * 100 > (long) width * 239) return new int[] {100, 239};
        return new int[] {width, height};
    }

    public static boolean canDisconnect(boolean connected, boolean inPictureInPicture, NvApp.Role role) {
        return connected && inPictureInPicture && role != NvApp.Role.INPUT_ONLY;
    }
}
