package com.limelight;

import com.limelight.nvstream.http.NvApp;

/** What a tap on a PC does with the game already running on it. */
final class LaunchPolicy {
    private LaunchPolicy() {}

    /**
     * With quick resume on, a tap on a PC that runs a game goes straight back into that game;
     * otherwise, and for any other PC, the tap opens its library.
     */
    static boolean resumesOnPcTap(boolean quickResume, boolean gameRunning) {
        return quickResume && gameRunning;
    }

    /** Quit on leave is offered for a game stream this device may quit; actions and other roles end differently. */
    static boolean offersQuitOnLeave(boolean canLaunch, NvApp.Control control, NvApp.Role role) {
        return canLaunch && control == NvApp.Control.NONE && role == NvApp.Role.STREAM;
    }
}
