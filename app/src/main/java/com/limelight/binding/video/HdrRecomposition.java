package com.limelight.binding.video;

/**
 * When the stream asks the system for one GPU composition of its Vulkan HDR picture (StreamGlass.recompose).
 * <p>
 * A screen (nightjar#93, OnePlus Pad 2) showed a Vulkan HDR stream that came back from the home screen washed out and
 * blue-grey, as PQ shown as SDR, until the stream menu opened. The menu changes nothing in the stream or the renderer:
 * its only effect on the picture is its blur window, which has the system compose what is under it on the GPU. The
 * stream asks for the same once each time its HDR output starts and each time its window comes back to the front,
 * never on the frame path.
 */
public final class HdrRecomposition {
    /** How long the blurred pixel stands: a few frames at any refresh rate. */
    public static final long DURATION_MS = 120;

    private boolean armed = true;

    /** The stream window came back to the front (onResume): ask again once HDR shows. */
    public void arm() {
        armed = true;
    }

    /**
     * Polled while the stream runs: true once per arming, when the renderer shows HDR and the window is in front.
     *
     * @param hdrOutput the Vulkan renderer presents HDR (rendererShowsHdr)
     * @param inFront   the stream window is in the foreground with focus, without the stream menu, picture-in-picture or an
     *                  external display
     */
    public boolean take(boolean hdrOutput, boolean inFront) {
        if (!hdrOutput) {
            // The next HDR output, after an SDR stretch or before the first HDR swapchain, is new to the compositor.
            armed = true;
            return false;
        }
        if (!armed || !inFront) return false;
        armed = false;
        return true;
    }

    /** Whether the Vulkan renderer's overlay line ("Vulkan HDR10 ...", "Vulkan HLG ...") names an HDR output. */
    public static boolean rendererShowsHdr(String rendererText) {
        return rendererText != null && (rendererText.startsWith("Vulkan HDR10") || rendererText.startsWith("Vulkan HLG"));
    }
}
