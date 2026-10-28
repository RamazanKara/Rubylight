package com.limelight.binding.video;

/**
 * Whether Render through Vulkan keeps up with the stream on this display. It falls behind once the
 * frames it presents stay under {@link #MIN_SHARE} of the frames received for {@link #BEHIND_MS} of
 * stats windows. A window that presents nothing (a decoder stall, not a slow renderer) or receives
 * under {@link #MIN_RECEIVED_FPS} (a still picture) starts the count again. Reported once per stream;
 * the renderer stays on, since the setting is the user's choice.
 */
final class VulkanPaceCheck {
    static final float MIN_SHARE = 0.8f;
    static final long BEHIND_MS = 5000;
    static final float MIN_RECEIVED_FPS = 10;

    private long behindMs;
    private boolean reported;

    /** One stats window of windowMs; true once, when the renderer has stayed behind for BEHIND_MS. */
    boolean window(int received, int presented, long windowMs) {
        if (reported || windowMs <= 0) return false;
        boolean behind = presented > 0 && received * 1000f / windowMs >= MIN_RECEIVED_FPS &&
                presented < received * MIN_SHARE;
        behindMs = behind ? behindMs + windowMs : 0;
        if (behindMs < BEHIND_MS) return false;
        reported = true;
        return true;
    }
}
