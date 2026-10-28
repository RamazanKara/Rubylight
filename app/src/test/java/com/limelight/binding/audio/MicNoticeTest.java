package com.limelight.binding.audio;

import android.app.Application;
import android.content.Context;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.Arrays;
import java.util.Collections;

import static org.junit.Assert.*;

/** The stream's notice when the microphone settings ask for processing the device lacks. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, qualifiers = "en", application = Application.class)
public class MicNoticeTest {
    private final Context context = RuntimeEnvironment.getApplication();

    @Test public void theNoticeNamesTheMissingEffectsByTheirSettings() {
        assertEquals("Not on this device: Echo cancellation. The microphone is sent without this processing.",
                MicNotice.text(context, Collections.singletonList(MicProcessing.Effect.ECHO_CANCELER)));
        assertEquals("Not on this device: Noise suppression, Automatic gain. The microphone is sent without this processing.",
                MicNotice.text(context, Arrays.asList(MicProcessing.Effect.NOISE_SUPPRESSOR, MicProcessing.Effect.AUTOMATIC_GAIN)));
    }

    @Test public void nothingMissingSaysNothing() {
        assertNull(MicNotice.text(context, null));
        assertNull(MicNotice.text(context, Collections.emptyList()));
    }

    @Test public void eachEffectPointsAtItsOwnSetting() {
        assertEquals(com.limelight.R.string.title_list_mic_echo_cancel, MicNotice.title(MicProcessing.Effect.ECHO_CANCELER));
        assertEquals(com.limelight.R.string.title_checkbox_mic_noise_suppression, MicNotice.title(MicProcessing.Effect.NOISE_SUPPRESSOR));
        assertEquals(com.limelight.R.string.title_checkbox_mic_auto_gain, MicNotice.title(MicProcessing.Effect.AUTOMATIC_GAIN));
    }

    @Test public void theSameNoticeShowsOncePerStream() {
        MicNotice.Once stream = new MicNotice.Once();
        String echo = MicNotice.text(context, Collections.singletonList(MicProcessing.Effect.ECHO_CANCELER));
        assertEquals(echo, stream.next(echo));
        assertNull("The microphone reopening says it again", stream.next(echo));
        assertNull("Nothing missing passes nothing", stream.next(null));
        String both = MicNotice.text(context, Arrays.asList(MicProcessing.Effect.ECHO_CANCELER, MicProcessing.Effect.NOISE_SUPPRESSOR));
        assertEquals("A different gap is news", both, stream.next(both));
        assertNull(stream.next(both));
        assertEquals("A new stream starts afresh", echo, new MicNotice.Once().next(echo));
    }
}
