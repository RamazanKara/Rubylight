package com.limelight.preferences;

import com.limelight.nvstream.http.NvApp;

import org.junit.Test;

import static org.junit.Assert.*;

public class HostDisplayChoiceTest {
    @Test
    public void wireValuesMatchTheHostsHostDisplayParameter() {
        assertEquals("hostDisplay", HostDisplayChoice.LAUNCH_PARAMETER);
        assertNull(HostDisplayChoice.HOST_DEFAULT.wireValue);
        assertEquals("exclusive", HostDisplayChoice.EXCLUSIVE.wireValue);
        assertEquals("extended_primary", HostDisplayChoice.EXTENDED_PRIMARY.wireValue);
        assertEquals("extended", HostDisplayChoice.EXTENDED.wireValue);
        assertEquals("extended_isolated", HostDisplayChoice.EXTENDED_ISOLATED.wireValue);
        assertEquals("extended_primary_isolated", HostDisplayChoice.EXTENDED_PRIMARY_ISOLATED.wireValue);
        assertEquals("physical", HostDisplayChoice.PHYSICAL.wireValue);
    }

    @Test
    public void virtualChoicesRequestAVirtualDisplayAndTheDefaultKeepsTheSetting() {
        for (boolean setting : new boolean[] {false, true}) {
            assertEquals(setting, HostDisplayChoice.HOST_DEFAULT.requestsVirtualDisplay(setting));
            assertTrue(HostDisplayChoice.EXCLUSIVE.requestsVirtualDisplay(setting));
            assertTrue(HostDisplayChoice.EXTENDED_PRIMARY.requestsVirtualDisplay(setting));
            assertTrue(HostDisplayChoice.EXTENDED.requestsVirtualDisplay(setting));
            assertTrue(HostDisplayChoice.EXTENDED_ISOLATED.requestsVirtualDisplay(setting));
            assertTrue(HostDisplayChoice.EXTENDED_PRIMARY_ISOLATED.requestsVirtualDisplay(setting));
            assertFalse(HostDisplayChoice.PHYSICAL.requestsVirtualDisplay(setting));
        }
    }

    @Test
    public void onlyAStreamChooses() {
        for (HostDisplayChoice choice : HostDisplayChoice.values()) {
            assertEquals(choice, choice.forRole(NvApp.Role.STREAM));
            assertEquals(HostDisplayChoice.HOST_DEFAULT, choice.forRole(NvApp.Role.REMOTE_MONITOR));
            assertEquals(HostDisplayChoice.HOST_DEFAULT, choice.forRole(NvApp.Role.INPUT_ONLY));
        }
    }

    @Test
    public void storedNamesRoundTripAndUnknownValuesAreTheHostDefault() {
        for (HostDisplayChoice choice : HostDisplayChoice.values()) {
            assertEquals(choice, HostDisplayChoice.fromStored(choice.name()));
        }
        for (String unknown : new String[] {null, "", "MIRROR", "exclusive"}) {
            assertEquals(HostDisplayChoice.HOST_DEFAULT, HostDisplayChoice.fromStored(unknown));
        }
        assertEquals(HostDisplayChoice.HOST_DEFAULT, HostDisplayChoice.load(null, "uuid"));
    }
}
