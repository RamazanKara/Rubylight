package com.limelight.update;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

public class SemVerTest {
    @Test
    public void compares_numbers_not_text() {
        assertTrue(SemVer.isNewer("0.10.0", "0.9.9"));
        assertFalse(SemVer.isNewer("0.9.9", "0.10.0"));
        assertTrue(SemVer.isNewer("1.0.0", "0.99.99"));
        assertTrue(SemVer.isNewer("0.7.1", "0.7.0"));
    }

    @Test
    public void equal_versions_are_not_an_update() {
        assertFalse(SemVer.isNewer("0.7.0", "0.7.0"));
        assertFalse(SemVer.isNewer("v0.7.0", "0.7.0"));
        assertEquals(0, SemVer.parse("v0.7.0").compareTo(SemVer.parse("0.7.0")));
        assertEquals(SemVer.parse("1.2"), SemVer.parse("v1.2.0"));
    }

    @Test
    public void ignores_a_leading_v() {
        assertEquals("0.7.1", SemVer.normalize("v0.7.1"));
        assertEquals("0.7.1", SemVer.normalize("V0.7.1"));
        assertTrue(SemVer.isNewer("v0.8.0", "0.7.0"));
        assertTrue(SemVer.isNewer("0.8.0", "v0.7.0"));
    }

    @Test
    public void a_prerelease_is_older_than_its_release() {
        assertTrue(SemVer.parse("1.0.0-rc.1").isPrerelease());
        assertFalse(SemVer.parse("1.0.0").isPrerelease());
        assertTrue(SemVer.isNewer("1.0.0", "1.0.0-rc.1"));
        assertFalse(SemVer.isNewer("1.0.0-rc.1", "1.0.0"));
        assertTrue(SemVer.isNewer("1.0.0-rc.10", "1.0.0-rc.2"));
        assertTrue(SemVer.isNewer("1.0.0-rc.1", "1.0.0-beta.5"));
        assertTrue(SemVer.isNewer("1.0.0-rc.1.1", "1.0.0-rc.1"));
    }

    @Test
    public void the_3_0_release_candidate_orders_between_0_8_and_3_0() {
        assertEquals("3.0.0-rc.1", SemVer.normalize("v3.0.0-rc.1"));
        assertTrue(SemVer.isNewer("v3.0.0-rc.1", "0.8.0"));
        assertFalse(SemVer.isNewer("0.8.0", "3.0.0-rc.1"));
        assertFalse(SemVer.isNewer("0.8.1", "3.0.0-rc.1"));
        assertTrue(SemVer.isNewer("3.0.0-rc.2", "3.0.0-rc.1"));
        assertTrue(SemVer.isNewer("3.0.0", "3.0.0-rc.1"));
    }

    @Test
    public void build_metadata_does_not_count() {
        assertEquals(0, SemVer.parse("1.2.3+abc").compareTo(SemVer.parse("1.2.3+xyz")));
    }

    @Test
    public void unreadable_versions_are_never_newer() {
        assertNull(SemVer.parse("nightly"));
        assertNull(SemVer.parse(""));
        assertNull(SemVer.parse(null));
        assertNotNull(SemVer.parse(" v1.0.0 "));
        assertFalse(SemVer.isNewer("nightly", "0.7.0"));
        assertFalse(SemVer.isNewer("0.8.0", "dev"));
        assertEquals("nightly", SemVer.normalize("nightly"));
    }
}
