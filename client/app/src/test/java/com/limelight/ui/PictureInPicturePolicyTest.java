package com.limelight.ui;

import com.limelight.nvstream.http.NvApp;
import org.junit.Test;
import static org.junit.Assert.*;

public class PictureInPicturePolicyTest {
    @Test
    public void desktopFocusChangesDoNotAutoEnterPip() {
        for (NvApp.Role role : NvApp.Role.values()) {
            assertFalse(PictureInPicturePolicy.shouldAutoEnter(true, role, 0, true));
            assertFalse(PictureInPicturePolicy.shouldAutoEnter(false, role, 0, false));
            assertFalse(PictureInPicturePolicy.shouldAutoEnter(true, role, 1, false));
            assertEquals(role != NvApp.Role.INPUT_ONLY,
                    PictureInPicturePolicy.shouldAutoEnter(true, role, 0, false));
        }
    }

    @Test
    public void customStreamRatiosStayInsideAndroidPipLimits() {
        assertArrayEquals(new int[] {1920, 1080}, PictureInPicturePolicy.aspectRatio(1920, 1080));
        assertArrayEquals(new int[] {1080, 1920}, PictureInPicturePolicy.aspectRatio(1080, 1920));
        assertArrayEquals(new int[] {239, 100}, PictureInPicturePolicy.aspectRatio(5120, 1440));
        assertArrayEquals(new int[] {100, 239}, PictureInPicturePolicy.aspectRatio(1440, 5120));
        assertArrayEquals(new int[] {239, 100}, PictureInPicturePolicy.aspectRatio(16384, 64));
    }

    @Test
    public void pipControlOnlyDisconnectsAnActiveMediaSessionInPip() {
        for (NvApp.Role role : NvApp.Role.values()) {
            assertFalse(PictureInPicturePolicy.canDisconnect(false, true, role));
            assertFalse(PictureInPicturePolicy.canDisconnect(true, false, role));
            assertEquals(role != NvApp.Role.INPUT_ONLY, PictureInPicturePolicy.canDisconnect(true, true, role));
        }
    }
}
