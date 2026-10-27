package com.limelight.binding.audio;

import android.media.AudioDeviceInfo;
import android.media.MediaRecorder;

import com.limelight.binding.audio.MicProcessing.Echo;
import com.limelight.binding.audio.MicProcessing.Effect;

import org.junit.Test;

import java.util.Arrays;
import java.util.Collections;
import java.util.EnumSet;
import java.util.Set;

import static org.junit.Assert.*;

public class MicProcessingTest {
    private static MicProcessing.Availability has(Effect... effects) {
        Set<Effect> set = effects.length == 0 ? EnumSet.noneOf(Effect.class) : EnumSet.copyOf(Arrays.asList(effects));
        return set::contains;
    }

    private static final MicProcessing.Availability ALL = has(Effect.values());

    @Test public void automaticEchoFollowsTheRoute() {
        MicProcessing auto = MicProcessing.DEFAULT;
        assertEquals(Echo.AUTO, auto.echo);
        MicProcessing.Plan speaker = auto.plan(false, ALL);
        assertTrue(speaker.echoCanceler);
        assertEquals(MediaRecorder.AudioSource.VOICE_COMMUNICATION, speaker.source);
        MicProcessing.Plan headphones = auto.plan(true, ALL);
        assertFalse(headphones.echoCanceler);
        assertEquals(MediaRecorder.AudioSource.MIC, headphones.source);
    }

    @Test public void onAndOffIgnoreTheRoute() {
        MicProcessing on = new MicProcessing(Echo.ON, false, false, 100);
        assertTrue(on.plan(true, ALL).echoCanceler);
        MicProcessing off = new MicProcessing(Echo.OFF, false, false, 100);
        assertFalse(off.plan(false, ALL).echoCanceler);
        assertEquals(MediaRecorder.AudioSource.MIC, off.plan(false, ALL).source);
    }

    @Test public void missingEffectsAreNamedNotSwapped() {
        MicProcessing all = new MicProcessing(Echo.ON, true, true, 100);
        MicProcessing.Plan plan = all.plan(false, has());
        assertEquals(Arrays.asList(Effect.ECHO_CANCELER, Effect.NOISE_SUPPRESSOR, Effect.AUTOMATIC_GAIN), plan.missing);
        assertFalse(plan.echoCanceler || plan.noiseSuppressor || plan.automaticGain);
        // The echo path keeps the voice source, which tunes its own echo handling, even without the effect.
        assertEquals(MediaRecorder.AudioSource.VOICE_COMMUNICATION, plan.source);
        MicProcessing.Plan some = all.plan(false, has(Effect.NOISE_SUPPRESSOR));
        assertEquals(Arrays.asList(Effect.ECHO_CANCELER, Effect.AUTOMATIC_GAIN), some.missing);
        assertTrue(some.enables(Effect.NOISE_SUPPRESSOR));
    }

    @Test public void effectsTurnedOffAreNeverMissing() {
        MicProcessing none = new MicProcessing(Echo.OFF, false, false, 100);
        MicProcessing.Plan plan = none.plan(false, has());
        assertTrue(plan.missing.isEmpty());
        for (Effect effect : Effect.values()) assertFalse(plan.enables(effect));
    }

    @Test public void failedAttachesJoinTheMissingOnesOnce() {
        MicProcessing all = new MicProcessing(Echo.ON, true, false, 100);
        MicProcessing.Plan plan = all.plan(false, has(Effect.NOISE_SUPPRESSOR));
        assertEquals(Arrays.asList(Effect.ECHO_CANCELER, Effect.NOISE_SUPPRESSOR),
                MicrophoneCapture.notOn(plan, Arrays.asList(Effect.NOISE_SUPPRESSOR, Effect.ECHO_CANCELER)));
        assertEquals(Collections.singletonList(Effect.ECHO_CANCELER), MicrophoneCapture.notOn(plan, Collections.emptyList()));
    }

    @Test public void unknownStoredValuesFallBackToTheDefaults() {
        assertEquals(Echo.AUTO, Echo.of(null));
        assertEquals(Echo.AUTO, Echo.of("bogus"));
        assertEquals(Echo.OFF, Echo.of("off"));
        assertEquals(MicGain.MAX_PERCENT, new MicProcessing(null, true, false, 9000).gainPercent);
        assertEquals(Echo.AUTO, new MicProcessing(null, true, false, 100).echo);
    }

    @Test public void headphonesAndHeadsetsArePrivateSpeakersAreNot() {
        assertTrue(MicRoute.privateOutput(new int[] {AudioDeviceInfo.TYPE_BUILTIN_SPEAKER, AudioDeviceInfo.TYPE_WIRED_HEADPHONES}));
        assertTrue(MicRoute.privateOutput(new int[] {AudioDeviceInfo.TYPE_BLUETOOTH_A2DP}));
        assertTrue(MicRoute.privateOutput(new int[] {AudioDeviceInfo.TYPE_BLUETOOTH_SCO}));
        assertTrue(MicRoute.privateOutput(new int[] {AudioDeviceInfo.TYPE_USB_HEADSET}));
        assertTrue(MicRoute.privateOutput(new int[] {MicRoute.TYPE_BLE_HEADSET}));
        assertFalse(MicRoute.privateOutput(new int[] {AudioDeviceInfo.TYPE_BUILTIN_SPEAKER, AudioDeviceInfo.TYPE_HDMI}));
        assertFalse(MicRoute.privateOutput(new int[0]));
    }

    @Test public void settingsBuildTheProcessing() {
        com.limelight.preferences.PreferenceConfiguration config = new com.limelight.preferences.PreferenceConfiguration();
        config.micEcho = "off";
        config.micNoiseSuppression = false;
        config.micAutoGain = true;
        config.micGainPercent = 150;
        MicProcessing processing = config.micProcessing();
        assertEquals(Echo.OFF, processing.echo);
        assertFalse(processing.noiseSuppression);
        assertTrue(processing.automaticGain);
        assertEquals(150, processing.gainPercent);
    }
}
