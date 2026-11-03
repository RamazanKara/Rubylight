package com.limelight;

import com.limelight.binding.audio.AndroidAudioRenderer;
import com.limelight.binding.audio.AudioPassthrough;
import com.limelight.binding.audio.SilentAudioRenderer;
import com.limelight.binding.video.DiscardingDecoderRenderer;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;

import org.junit.Test;

import static org.junit.Assert.*;

public class GameConnectionRenderersTest {
    private final VideoDecoderRenderer decoder = new DiscardingDecoderRenderer();

    @Test public void muteAudioShowsTheStreamAndPlaysNothing() {
        GameConnection.Renderers renderers = GameConnection.renderers(false, true, decoder, null, false, null);
        assertTrue(renderers.audio instanceof SilentAudioRenderer);
        assertSame(decoder, renderers.video);
    }

    @Test public void anUnmutedStreamPlaysOnThisDevice() {
        GameConnection.Renderers renderers = GameConnection.renderers(false, false, decoder, null, false, null);
        assertTrue(renderers.audio instanceof AndroidAudioRenderer);
        assertSame(decoder, renderers.video);
    }

    @Test public void inputOnlyShowsAndPlaysNothingMutedOrNot() {
        for (boolean mute : new boolean[] {false, true}) {
            GameConnection.Renderers renderers = GameConnection.renderers(true, mute, decoder, null, false, null);
            assertTrue(renderers.audio instanceof SilentAudioRenderer);
            assertTrue(renderers.video instanceof DiscardingDecoderRenderer);
            assertNotSame(decoder, renderers.video);
        }
    }

    @Test public void aMutedPassthroughStreamStillStarts() {
        SilentAudioRenderer silent = new SilentAudioRenderer();
        assertEquals(0, silent.setupEncoded(null, AudioPassthrough.CODEC_AC3));
        assertEquals(0, silent.setupEncoded(null, AudioPassthrough.CODEC_EAC3));
        assertEquals(0, silent.setup(null, 48000, 240));
    }
}
