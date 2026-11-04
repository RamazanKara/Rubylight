package com.limelight.binding.video;

import android.app.Application;
import android.media.MediaCodecInfo;
import android.media.MediaFormat;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import static com.limelight.binding.video.UpscalingPolicy.Reason.*;
import static org.junit.Assert.*;

/**
 * The upscaler's check of each decoder output format: AV1 from AMD encoders keeps its alignment
 * padding (1920x1080 decodes as 1920x1082, 1872 wide as 1920), which used to end upscaling for the
 * whole stream as a size change.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class UpscalingOutputFormatTest {
    // SurfaceTexture's usual transform for a decoder buffer: rows flipped, origin bottom left.
    private static final float[] FLIP = {1, 0, 0, 0, 0, -1, 0, 0, 0, 0, 1, 0, 0, 1, 0, 1};

    private static MediaFormat format(int width, int height) {
        MediaFormat format = MediaFormat.createVideoFormat("video/av01", width, height);
        format.setInteger("crop-left", 0);
        format.setInteger("crop-top", 0);
        format.setInteger("crop-right", width - 1);
        format.setInteger("crop-bottom", height - 1);
        return format;
    }

    private static float[] check(MediaFormat format, boolean packed, int width, int height, boolean inset,
                                 UpscalingPolicy policy) {
        return UpscalingPolicy.checkOutputFormat(format, packed, width, height, inset, false, false, policy);
    }

    /** The texel a texture coordinate of the cropped picture samples, top-left origin. */
    private static float[] texel(float[] crop, float u, float v, int decodedWidth, int decodedHeight) {
        float[] transform = FLIP.clone();
        UpscalingPolicy.applyCrop(transform, crop);
        float s = transform[0] * u + transform[4] * v + transform[12];
        float t = transform[1] * u + transform[5] * v + transform[13];
        return new float[] {s * decodedWidth, t * decodedHeight};
    }

    @Test
    public void paddedAv1KeepsTheUpscalerAndSamplesOnlyTheStream() {
        UpscalingPolicy policy = new UpscalingPolicy(60);
        float[] crop = check(format(1920, 1082), false, 1920, 1080, false, policy);
        assertEquals(NONE, policy.getReason());
        assertArrayEquals(new float[] {1f, 1080f / 1082, 0f, 1f - 1080f / 1082}, crop, 1e-6f);
        // EASU and SGSR clamp to texel centres: the picture's edges are the stream's.
        assertArrayEquals(new float[] {0, 0}, texel(crop, 0, 1, 1920, 1082), 1e-3f);
        assertArrayEquals(new float[] {1920, 1080}, texel(crop, 1, 0, 1920, 1082), 1e-3f);

        // 1872x1053 codes as 1920x1056: padding at the right and the bottom.
        policy = new UpscalingPolicy(60);
        crop = check(format(1920, 1056), false, 1872, 1053, false, policy);
        assertEquals(NONE, policy.getReason());
        assertArrayEquals(new float[] {1872, 1053}, texel(crop, 1, 0, 1920, 1056), 1e-3f);
        assertArrayEquals(new float[] {0, 0}, texel(crop, 0, 1, 1920, 1056), 1e-3f);
    }

    @Test
    public void bilinearEndsACroppedEdgeHalfATexelInside() {
        UpscalingPolicy policy = new UpscalingPolicy(60);
        float[] crop = check(format(1920, 1056), false, 1872, 1053, true, policy);
        assertEquals(NONE, policy.getReason());
        // The last column and row are sampled at their centres, never blended with padding.
        assertArrayEquals(new float[] {1871.5f, 1052.5f}, texel(crop, 1, 0, 1920, 1056), 1e-3f);
        assertArrayEquals(new float[] {0, 0}, texel(crop, 0, 1, 1920, 1056), 1e-3f);
        // An edge without padding is sampled to the end as before.
        crop = check(format(1920, 1082), false, 1920, 1080, true, policy);
        assertArrayEquals(new float[] {1920, 1079.5f}, texel(crop, 1, 0, 1920, 1082), 1e-3f);
    }

    @Test
    public void aPictureOfTheStreamSizeIsUntouched() {
        UpscalingPolicy policy = new UpscalingPolicy(60);
        float[] crop = check(format(1280, 720), false, 1280, 720, true, policy);
        assertEquals(NONE, policy.getReason());
        assertArrayEquals(new float[] {1, 1, 0, 0}, crop, 0f);
        float[] transform = FLIP.clone();
        UpscalingPolicy.applyCrop(transform, crop);
        assertArrayEquals(FLIP, transform, 0f);
    }

    @Test
    public void aRealSizeChangeStillEndsUpscaling() {
        int[][] decoded = {{1984, 1080}, {1920, 1096}, {1280, 720}, {2560, 1440}, {1904, 1080}};
        for (int[] size : decoded) {
            UpscalingPolicy policy = new UpscalingPolicy(60);
            assertNull(check(format(size[0], size[1]), false, 1920, 1080, false, policy));
            assertEquals(size[0] + "x" + size[1], SIZE_CHANGED, policy.getReason());
        }
        // The crop window is read as before: a decoder that crops to the stream needs no crop.
        UpscalingPolicy policy = new UpscalingPolicy(60);
        MediaFormat cropped = MediaFormat.createVideoFormat("video/av01", 1920, 1088);
        cropped.setInteger("crop-left", 0);
        cropped.setInteger("crop-top", 0);
        cropped.setInteger("crop-right", 1919);
        cropped.setInteger("crop-bottom", 1079);
        assertArrayEquals(new float[] {1, 1, 0, 0}, check(cropped, false, 1920, 1080, true, policy), 0f);
        assertEquals(NONE, policy.getReason());
    }

    @Test
    public void hdrAndP010StillEndUpscaling() {
        UpscalingPolicy policy = new UpscalingPolicy(60);
        MediaFormat hdr = format(1920, 1082);
        hdr.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_ST2084);
        check(hdr, false, 1920, 1080, false, policy);
        assertEquals(HDR, policy.getReason());

        policy = new UpscalingPolicy(60);
        MediaFormat hlg = format(1280, 720);
        hlg.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_HLG);
        check(hlg, false, 1280, 720, false, policy);
        assertEquals(HDR, policy.getReason());

        MediaFormat p010 = format(1920, 1082);
        p010.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatYUVP010);
        policy = new UpscalingPolicy(60);
        UpscalingPolicy.checkOutputFormat(p010, false, 1920, 1080, false, false, true, policy);
        assertEquals(TEN_BIT, policy.getReason());
        policy = new UpscalingPolicy(60);
        UpscalingPolicy.checkOutputFormat(p010, false, 1920, 1080, false, true, true, policy);
        assertEquals(HDR, policy.getReason());

        MediaFormat sdr = format(1280, 720);
        sdr.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO);
        policy = new UpscalingPolicy(60);
        check(sdr, false, 1280, 720, false, policy);
        assertEquals(NONE, policy.getReason());
    }

    @Test
    public void packedChromaChecksOnlyItsPicture() {
        // Packed full chroma's picture is 64-column and 16-row aligned already; padding past it is
        // cropped exactly (its shader addresses texel centres), an offset picture is a failure.
        UpscalingPolicy policy = new UpscalingPolicy(60);
        MediaFormat packed = format(1920, 2176);
        packed.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_ST2084);
        assertArrayEquals(new float[] {1, 1, 0, 0}, check(packed, true, 1920, 2176, false, policy), 0f);
        assertEquals(NONE, policy.getReason());
        float[] crop = check(format(1920, 2186), true, 1920, 2176, false, policy);
        assertArrayEquals(new float[] {1920, 2176}, texel(crop, 1, 0, 1920, 2186), 1e-3f);
        assertEquals(NONE, policy.getReason());

        MediaFormat offset = format(1920, 2176);
        offset.setInteger("crop-top", 8);
        assertNull(check(offset, true, 1920, 2176, false, policy));
        assertEquals(SIZE_CHANGED, policy.getReason());

        policy = new UpscalingPolicy(60);
        assertNull(check(format(1984, 2176), true, 1920, 2176, false, policy));
        assertEquals(SIZE_CHANGED, policy.getReason());
    }
}
