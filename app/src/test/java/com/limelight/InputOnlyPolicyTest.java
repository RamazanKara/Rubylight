package com.limelight;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;

import static org.junit.Assert.*;

public class InputOnlyPolicyTest {
    private static ComputerDetails pc(PairingManager.PairState pairState, long permission) {
        ComputerDetails pc = new ComputerDetails();
        pc.pairState = pairState;
        pc.permission = permission;
        return pc;
    }

    @Test public void hostsWithTheirOwnInputOnlyModeKeepIt() {
        for (String host : new String[] {"rubylight-serverinfo.xml", "vibepollo-serverinfo.xml", "butterpollo-serverinfo.xml"}) {
            HostCapabilities caps = TestHosts.fixture(host);
            assertEquals(host, InputOnlyPolicy.Mode.NATIVE, InputOnlyPolicy.mode(true, caps, false));
            assertEquals(host, InputOnlyPolicy.Mode.NATIVE, InputOnlyPolicy.mode(false, caps, false));
        }
    }

    @Test public void aListedInputOnlyAppWinsOnAnyHost() {
        // Apollo lists its Remote Input app only while its input-only mode is on.
        HostCapabilities apollo = TestHosts.fixture("apollo-input-only-serverinfo.xml");
        assertEquals(InputOnlyPolicy.Mode.NATIVE, InputOnlyPolicy.mode(true, apollo, true));
        assertEquals(InputOnlyPolicy.Mode.NATIVE, InputOnlyPolicy.mode(true, HostCapabilities.UNKNOWN, true));
    }

    @Test public void hostsWithoutTheModeGetTheSmallStreamOnlyWhenTheSettingIsOn() {
        for (String host : new String[] {"sunshine-serverinfo.xml", "gamestream-serverinfo.xml", "apollo-serverinfo.xml"}) {
            HostCapabilities caps = TestHosts.fixture(host);
            assertEquals(host, InputOnlyPolicy.Mode.SMALL_STREAM, InputOnlyPolicy.mode(true, caps, false));
            assertEquals(host, InputOnlyPolicy.Mode.OFF, InputOnlyPolicy.mode(false, caps, false));
        }
        assertEquals(InputOnlyPolicy.Mode.SMALL_STREAM, InputOnlyPolicy.mode(true, HostCapabilities.UNKNOWN, false));
        assertEquals(InputOnlyPolicy.Mode.OFF, InputOnlyPolicy.mode(false, HostCapabilities.UNKNOWN, false));
    }

    @Test public void anUnpairedPcMustPairFirst() {
        assertEquals(HostCapabilities.Reason.PAIR.text,
                InputOnlyPolicy.smallStreamBlocker(pc(PairingManager.PairState.NOT_PAIRED, -1)));
        assertEquals(HostCapabilities.Reason.PAIR.text,
                InputOnlyPolicy.smallStreamBlocker(pc(null, -1)));
    }

    @Test public void missingPermissionsSayWhichOne() {
        assertEquals(HostCapabilities.Reason.VIEW_PERMISSION.text, InputOnlyPolicy.smallStreamBlocker(
                pc(PairingManager.PairState.PAIRED, ComputerDetails.PERMISSION_INPUT | ComputerDetails.PERMISSION_LIST)));
        assertEquals(R.string.input_only_reason_input_permission, InputOnlyPolicy.smallStreamBlocker(
                pc(PairingManager.PairState.PAIRED, ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH)));
        assertEquals(0, InputOnlyPolicy.smallStreamBlocker(
                pc(PairingManager.PairState.PAIRED, ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_CONTROLLER)));
    }

    @Test public void aPairedHostWithoutPermissionsCanStart() {
        assertEquals(0, InputOnlyPolicy.smallStreamBlocker(pc(PairingManager.PairState.PAIRED, -1)));
    }

    @Test public void theSmallStreamNeverReplacesARunningGame() {
        assertTrue(InputOnlyPolicy.keepsRunningGame(false, false));
        assertTrue(InputOnlyPolicy.keepsRunningGame(true, true));
        assertFalse(InputOnlyPolicy.keepsRunningGame(true, false));
    }

    @Test public void theEstimateCountsErrorCorrectionAndAudio() {
        assertEquals(696, InputOnlyPolicy.estimatedKbps());
        assertEquals(313, InputOnlyPolicy.estimatedMegabytesPerHour());
        assertTrue(InputOnlyPolicy.estimatedKbps() < 1000);
    }
}
