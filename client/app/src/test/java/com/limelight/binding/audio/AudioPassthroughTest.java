package com.limelight.binding.audio;

import android.media.AudioFormat;
import android.media.AudioManager;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

public class AudioPassthroughTest {
    @Test public void aSoundbarThatDecodesDolbyButNotSurroundPcmGetsOnlyDolby() {
        List<Integer> masks = new ArrayList<>();
        int codecs = AudioPassthrough.playableCodecs(29, 6, (encoding, mask) -> {
            masks.add(mask);
            return encoding == AudioFormat.ENCODING_AC3 || encoding == AudioFormat.ENCODING_E_AC3;
        });
        assertEquals(AudioPassthrough.CODEC_AC3 | AudioPassthrough.CODEC_EAC3, codecs);
        for (int mask : masks) assertEquals(AudioFormat.CHANNEL_OUT_5POINT1, mask);
        assertEquals("AC3, E-AC3", AudioPassthrough.names(codecs));
    }

    @Test public void anOutputThatTakesPcmAsIsGetsPcm() {
        int codecs = AudioPassthrough.playableCodecs(29, 2,
                (encoding, mask) -> encoding == AudioFormat.ENCODING_PCM_16BIT && mask == AudioFormat.CHANNEL_OUT_STEREO);
        assertEquals(AudioPassthrough.CODEC_PCM, codecs);
        assertEquals("PCM", AudioPassthrough.names(codecs));
    }

    @Test public void nothingIsAskedForWhereTheRouteCannotBeAskedOrHasNoLayout() {
        AudioPassthrough.DirectPlayback everything = (encoding, mask) -> true;
        // Android 9 has no isDirectPlaybackSupported.
        assertEquals(0, AudioPassthrough.playableCodecs(28, 6, everything));
        // No passthrough layout for quad or 7.1.4.
        assertEquals(0, AudioPassthrough.playableCodecs(29, 4, everything));
        assertEquals(0, AudioPassthrough.playableCodecs(29, 12, everything));
        assertEquals(0, AudioPassthrough.playableCodecs(29, 6, (encoding, mask) -> false));
        assertEquals("none", AudioPassthrough.names(0));
    }

    @Test public void theCodecBitsAreTheProtocolsOwn() throws java.io.IOException {
        java.io.File source = new java.io.File("../protocol/core/src/audio_ext.rs");
        if (!source.isFile()) source = new java.io.File("protocol/core/src/audio_ext.rs");
        String rust = new String(java.nio.file.Files.readAllBytes(source.toPath()),
                java.nio.charset.StandardCharsets.UTF_8);
        assertTrue(rust.contains("pub const CODEC_PCM: u8 = " + AudioPassthrough.CODEC_PCM + ";"));
        assertTrue(rust.contains("pub const CODEC_AC3: u8 = " + AudioPassthrough.CODEC_AC3 + ";"));
        assertTrue(rust.contains("pub const CODEC_EAC3: u8 = " + AudioPassthrough.CODEC_EAC3 + ";"));
    }

    @Test public void android13AsksGetDirectPlaybackSupportAndOlderAsksTheOldCall() {
        AudioPassthrough.DirectPlayback legacy = (encoding, mask) -> encoding == AudioFormat.ENCODING_AC3;
        AudioPassthrough.DirectSupport modern = (encoding, mask) ->
                encoding == AudioFormat.ENCODING_E_AC3 ? AudioManager.DIRECT_PLAYBACK_BITSTREAM_SUPPORTED
                        : AudioManager.DIRECT_PLAYBACK_NOT_SUPPORTED;
        assertEquals(AudioPassthrough.CODEC_AC3,
                AudioPassthrough.playableCodecs(29, 6, AudioPassthrough.select(29, legacy, modern)));
        assertEquals(AudioPassthrough.CODEC_AC3,
                AudioPassthrough.playableCodecs(32, 6, AudioPassthrough.select(32, legacy, modern)));
        assertEquals(AudioPassthrough.CODEC_EAC3,
                AudioPassthrough.playableCodecs(33, 6, AudioPassthrough.select(33, legacy, modern)));
        assertEquals(AudioPassthrough.CODEC_EAC3,
                AudioPassthrough.playableCodecs(36, 6, AudioPassthrough.select(36, legacy, modern)));
    }

    @Test public void everyDirectModeCountsAsTheOldCallCountedIt() {
        int[] modes = {
                AudioManager.DIRECT_PLAYBACK_BITSTREAM_SUPPORTED,
                AudioManager.DIRECT_PLAYBACK_OFFLOAD_SUPPORTED,
                AudioManager.DIRECT_PLAYBACK_OFFLOAD_GAPLESS_SUPPORTED,
                AudioManager.DIRECT_PLAYBACK_BITSTREAM_SUPPORTED | AudioManager.DIRECT_PLAYBACK_OFFLOAD_SUPPORTED,
        };
        for (int mode : modes) {
            assertTrue(AudioPassthrough.select(33, null, (encoding, mask) -> mode)
                    .supports(AudioFormat.ENCODING_AC3, AudioFormat.CHANNEL_OUT_5POINT1));
        }
        assertFalse(AudioPassthrough.select(33, null, (encoding, mask) -> AudioManager.DIRECT_PLAYBACK_NOT_SUPPORTED)
                .supports(AudioFormat.ENCODING_AC3, AudioFormat.CHANNEL_OUT_5POINT1));
        assertEquals(0, AudioPassthrough.playableCodecs(33, 6,
                AudioPassthrough.select(33, null, (encoding, mask) -> AudioManager.DIRECT_PLAYBACK_NOT_SUPPORTED)));
    }

    @Test public void onlyDolbyCodecsHaveACompressedEncoding() {
        assertEquals(AudioFormat.ENCODING_AC3, AudioPassthrough.encoding(AudioPassthrough.CODEC_AC3));
        assertEquals(AudioFormat.ENCODING_E_AC3, AudioPassthrough.encoding(AudioPassthrough.CODEC_EAC3));
        assertEquals(0, AudioPassthrough.encoding(AudioPassthrough.CODEC_PCM));
        assertEquals(0, AudioPassthrough.encoding(1));
    }
}
