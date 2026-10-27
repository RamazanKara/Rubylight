package com.limelight.update;

import org.junit.Test;

import java.util.Collections;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

public class UpdatePolicyTest {
    private static final long HOUR = 60L * 60 * 1000;
    private static final long NOW = 1_700_000_000_000L;

    private static ReleaseInfo release(String tag, boolean draft, boolean prerelease) {
        return new ReleaseInfo(tag, "", draft, prerelease, Collections.emptyList());
    }

    @Test
    public void checks_at_most_every_twelve_hours() {
        assertEquals(12 * HOUR, UpdatePolicy.CHECK_INTERVAL_MS);
        assertTrue(UpdatePolicy.isCheckDue(true, 0, NOW));
        assertFalse(UpdatePolicy.isCheckDue(true, NOW - 11 * HOUR - 59 * 60 * 1000, NOW));
        assertTrue(UpdatePolicy.isCheckDue(true, NOW - 12 * HOUR, NOW));
        assertTrue(UpdatePolicy.isCheckDue(true, NOW - 30 * HOUR, NOW));
        assertFalse(UpdatePolicy.isCheckDue(true, NOW, NOW));
    }

    @Test
    public void a_clock_set_back_does_not_block_checks_forever() {
        assertTrue(UpdatePolicy.isCheckDue(true, NOW + 5 * HOUR, NOW));
    }

    @Test
    public void the_switch_turns_automatic_checks_off() {
        assertFalse(UpdatePolicy.isCheckDue(false, 0, NOW));
        assertFalse(UpdatePolicy.isCheckDue(false, NOW - 100 * HOUR, NOW));
    }

    @Test
    public void offers_a_newer_final_release() {
        assertTrue(UpdatePolicy.shouldOffer(release("v0.8.0", false, false), "0.7.0", ""));
        assertTrue(UpdatePolicy.shouldOffer(release("v0.10.0", false, false), "0.9.9", null));
    }

    @Test
    public void does_not_offer_the_same_older_draft_or_prerelease() {
        assertFalse(UpdatePolicy.shouldOffer(null, "0.7.0", ""));
        assertFalse(UpdatePolicy.shouldOffer(release("v0.7.0", false, false), "0.7.0", ""));
        assertFalse(UpdatePolicy.shouldOffer(release("v0.6.0", false, false), "0.7.0", ""));
        assertFalse(UpdatePolicy.shouldOffer(release("v0.8.0", true, false), "0.7.0", ""));
        assertFalse(UpdatePolicy.shouldOffer(release("v0.8.0", false, true), "0.7.0", ""));
        assertFalse(UpdatePolicy.shouldOffer(release("v0.8.0-rc.1", false, false), "0.7.0", ""));
    }

    @Test
    public void a_skipped_version_stays_quiet_until_a_newer_one_comes() {
        assertFalse(UpdatePolicy.shouldOffer(release("v0.8.0", false, false), "0.7.0", "0.8.0"));
        assertFalse(UpdatePolicy.shouldOffer(release("v0.8.0", false, false), "0.7.0", "v0.8.0"));
        assertTrue(UpdatePolicy.shouldOffer(release("v0.8.1", false, false), "0.7.0", "0.8.0"));
        assertTrue(UpdatePolicy.shouldOffer(release("v0.10.0", false, false), "0.7.0", "0.9.0"));
    }

    @Test
    public void google_play_updates_the_app_itself() {
        assertTrue(UpdatePolicy.isManagedByStore("com.android.vending"));
        assertFalse(UpdatePolicy.isManagedByStore(null));
        assertFalse(UpdatePolicy.isManagedByStore("com.butterpollo.client"));
        assertFalse(UpdatePolicy.isManagedByStore("com.google.android.packageinstaller"));
        assertTrue(UpdatePolicy.isSelfUpdateAvailable(null, true));
        assertTrue(UpdatePolicy.isSelfUpdateAvailable("com.android.shell", true));
        assertFalse(UpdatePolicy.isSelfUpdateAvailable("com.android.vending", true));
        assertFalse(UpdatePolicy.isSelfUpdateAvailable(null, false));
    }
}
