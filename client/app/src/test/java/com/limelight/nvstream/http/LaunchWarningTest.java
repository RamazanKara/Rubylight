/**
 * The launch notice's text choice; the reply itself is parsed and cleaned in Rust
 * (launch_warning_tests.rs covers the hostile replies).
 */
package com.limelight.nvstream.http;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

import org.junit.Test;

public class LaunchWarningTest {
    @Test
    public void prefersThePcsTextAndKnowsTwoCodes() {
        assertEquals("Scale lowered.", LaunchWarning.text("display_scale", "Scale lowered.", "u", "d"));
        assertEquals("u", LaunchWarning.text("virtual_display_unavailable", null, "u", "d"));
        assertEquals("d", LaunchWarning.text("virtual_display_disabled_by_client", "", "u", "d"));
        assertNull(LaunchWarning.text("future_code", null, "u", "d"));
        assertNull(LaunchWarning.text(null, "orphan", "u", "d"));
    }
}
