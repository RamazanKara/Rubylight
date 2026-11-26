package com.limelight.binding.video;

import android.opengl.GLES30;

/**
 * Frame Warp's shader row for the OpenGL ES presenter (docs/frame-warp.md): the last four floats
 * MoonBridge.frameWarpForPts() writes, set as warp.frag's {@code uWarpV2}. x is the edge fill's strength (0 to 1), y how
 * many HUD mask cells a pixel whose shifted position lands on the HUD looks back along the shift (0 to 8), z and w the
 * confidence and the PC's flags, for information only. Kept apart so GlesUpscaler only hands the floats over; the native
 * renderers do the same in jni/vulkan/frame_warp_v2_pc.h. The GL thread's.
 */
final class GlesFrameWarpV2 {
    /** Where the row starts among the floats (after the shift and the lens terms), and its length. */
    static final int FIRST = 8;
    static final int FLOATS = 4;

    private int uniform = -1;

    /** Finds the uniform in warp.frag's program, which is in use, and sets the row of a picture drawn as it is. */
    void locate(int program) {
        uniform = GLES30.glGetUniformLocation(program, "uWarpV2");
        GLES30.glUniform4f(uniform, 0f, 0f, 0f, 0f);
    }

    /** Sets the row from what frameWarpForPts() wrote into {@code warpOut}; zero for a picture drawn as it is. */
    void apply(boolean active, float[] warpOut) {
        GLES30.glUniform4f(uniform, value(active, warpOut, 0), value(active, warpOut, 1), value(active, warpOut, 2),
                value(active, warpOut, 3));
    }

    /** Float {@code i} of the row of a draw whose shift was {@code active}, from {@code warpOut}. */
    static float value(boolean active, float[] warpOut, int i) {
        return active && warpOut.length >= FIRST + FLOATS ? warpOut[FIRST + i] : 0f;
    }
}
