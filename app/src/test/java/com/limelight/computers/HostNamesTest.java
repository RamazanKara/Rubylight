package com.limelight.computers;

import static org.junit.Assert.*;

import org.junit.Test;

public class HostNamesTest {
    @Test
    public void typedNamesAreTrimmedAndCappedAtFortyCharacters() {
        assertEquals("Den PC", HostNames.normalize("  Den PC \n", "DESKTOP-1"));
        String forty = "a".repeat(HostNames.MAX_LENGTH);
        assertEquals(forty, HostNames.normalize(forty + "bcd", "DESKTOP-1"));
        // Characters outside the BMP count once and are never cut in half.
        String emoji = "🖥".repeat(HostNames.MAX_LENGTH + 5);
        assertEquals("🖥".repeat(HostNames.MAX_LENGTH), HostNames.normalize(emoji, "x"));
    }

    @Test
    public void anEmptyNameOrThePcsOwnNameMeansNoAlias() {
        assertNull(HostNames.normalize(null, "DESKTOP-1"));
        assertNull(HostNames.normalize("", "DESKTOP-1"));
        assertNull(HostNames.normalize("   ", "DESKTOP-1"));
        assertNull(HostNames.normalize(" DESKTOP-1 ", "DESKTOP-1"));
        assertEquals("desktop-1", HostNames.normalize("desktop-1", "DESKTOP-1"));
        assertEquals("Den", HostNames.normalize("Den", null));
    }
}
