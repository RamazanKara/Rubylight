package com.limelight;

import com.limelight.nvstream.StreamConfiguration;
import org.junit.Test;
import static org.junit.Assert.*;

public class GameLaunchTest {
    @Test
    public void missingRestoredLaunchDataIsRejectedBeforeResourcesAreAcquired() {
        assertFalse(Game.isValidLaunch(null, 47989, "client", 1, null));
        assertFalse(Game.isValidLaunch(" ", 47989, "client", 1, null));
        assertFalse(Game.isValidLaunch("host", 0, "client", 1, null));
        assertFalse(Game.isValidLaunch("host", 65536, "client", 1, null));
        assertFalse(Game.isValidLaunch("host", 47989, null, 1, null));
        assertFalse(Game.isValidLaunch("host", 47989, "client", StreamConfiguration.INVALID_APP_ID, null));
    }

    @Test
    public void numericAndUuidLaunchesRemainSupported() {
        assertTrue(Game.isValidLaunch("host", 47989, "client", 1, null));
        assertTrue(Game.isValidLaunch("::1", 47989, "client", StreamConfiguration.INVALID_APP_ID, "app-uuid"));
    }
}
