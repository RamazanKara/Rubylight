package com.limelight.nvstream;

import org.junit.Test;

import static org.junit.Assert.*;

public class StreamConfigurationPackedTest {
    // NvConnection asks the native engine for packed 4:4:4 from this value, not from the saved setting,
    // so a session Game reconnected without it (setPacked444(false)) really streams without it.
    @Test
    public void thePackedChoiceIsTheSessionsAndOffUntilSet() {
        assertFalse(new StreamConfiguration.Builder().build().isPacked444());
        assertTrue(new StreamConfiguration.Builder().setPacked444(true).build().isPacked444());
        assertFalse(new StreamConfiguration.Builder().setPacked444(true).setPacked444(false).build().isPacked444());
    }
}
