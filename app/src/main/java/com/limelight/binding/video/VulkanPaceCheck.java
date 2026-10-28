package com.limelight.binding.video;

/**
 * Whether Render through Vulkan keeps up with the stream on this display. It falls behind once the
 * frames it presents stay under {@link #MIN_SHARE} of the frames it could present for {@link #BEHIND_MS}
 * of stats windows. It presents at most one frame per display refresh (the newest, vsync-paced), so it
 * could present the frames received, capped at the refreshes in the window: a 120 fps stream on a
 * 60 Hz display that presents 60 keeps up. A window that presents nothing (a decoder stall, not a slow renderer) or receives
 * under {@link #MIN_RECEIVED_FPS} (a still picture) starts the count again. Reported once per stream;
 * the renderer stays on, since the setting is the user's choice.
 */
final class VulkanPaceCheck {
    static final float MIN_SHARE = 0.8f;
    static final long BEHIND_MS = 5000;
    static final float MIN_RECEIVED_FPS = 10;

    private long behindMs;
    private boolean reported;

    /**
     * One stats window of windowMs on a display refreshing at refreshHz (0 or less when unknown: the
     * frames received count then); true once, when the renderer has stayed behind for BEHIND_MS.
     */
    boolean window(int received, int presented, long windowMs, float refreshHz) {
        if (reported || windowMs <= 0) return false;
        float presentable = refreshHz > 0 ? Math.min(received, refreshHz * windowMs / 1000f) : received;
        boolean behind = presented > 0 && received * 1000f / windowMs >= MIN_RECEIVED_FPS &&
                presented < presentable * MIN_SHARE;
        behindMs = behind ? behindMs + windowMs : 0;
        if (behindMs < BEHIND_MS) return false;
        reported = true;
        return true;
    }
}
