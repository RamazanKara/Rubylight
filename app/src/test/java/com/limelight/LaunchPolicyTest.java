package com.limelight;

import com.limelight.nvstream.http.NvApp;

import org.junit.Test;

import static org.junit.Assert.*;

public class LaunchPolicyTest {
    @Test public void quickResumeSkipsTheLibraryOnlyForARunningGame() {
        assertTrue(LaunchPolicy.resumesOnPcTap(true, true));
        assertFalse(LaunchPolicy.resumesOnPcTap(true, false));
    }

    @Test public void withoutQuickResumeATapStillOpensTheLibrary() {
        assertFalse(LaunchPolicy.resumesOnPcTap(false, true));
        assertFalse(LaunchPolicy.resumesOnPcTap(false, false));
    }

    @Test public void quitOnLeaveIsOfferedForGameStreamsThisDeviceMayQuit() {
        assertTrue(LaunchPolicy.offersQuitOnLeave(true, NvApp.Control.NONE, NvApp.Role.STREAM));
        assertFalse(LaunchPolicy.offersQuitOnLeave(false, NvApp.Control.NONE, NvApp.Role.STREAM));
    }

    @Test public void hostActionsAndRemoteRolesDoNotOfferQuitOnLeave() {
        for (NvApp.Control control : NvApp.Control.values()) {
            if (control == NvApp.Control.NONE) continue;
            assertFalse(control.name(), LaunchPolicy.offersQuitOnLeave(true, control, NvApp.Role.STREAM));
        }
        assertFalse(LaunchPolicy.offersQuitOnLeave(true, NvApp.Control.NONE, NvApp.Role.REMOTE_MONITOR));
        assertFalse(LaunchPolicy.offersQuitOnLeave(true, NvApp.Control.NONE, NvApp.Role.INPUT_ONLY));
    }
}
