package com.limelight.binding.audio;

import android.media.AudioDeviceInfo;
import android.media.MediaRecorder;

import com.limelight.binding.audio.MicProcessing.Echo;
import com.limelight.binding.audio.MicProcessing.EchoEngine;
import com.limelight.binding.audio.MicProcessing.Effect;

import org.junit.Test;

import java.util.Arrays;
import java.util.Collections;
import java.util.EnumSet;
import java.util.List;
import java.util.Set;

import static org.junit.Assert.*;

public class MicProcessingTest {
    private static MicProcessing.Availability has(Effect... effects) {
        Set<Effect> set = effects.length == 0 ? EnumSet.noneOf(Effect.class) : EnumSet.copyOf(Arrays.asList(effects));
        return set::contains;
    }

    /** Android's effects as listed, and Nightjar's own echo canceller when {@code own}. */
    private static MicProcessing.Availability has(boolean own, Effect... effects) {
        MicProcessing.Availability android = has(effects);
        return new MicProcessing.Availability() {
            @Override public boolean has(Effect effect) {
                return android.has(effect);
            }

            @Override public boolean hasOwnEchoCanceller() {
                return own;
            }
        };
    }

    private static final MicProcessing.Availability ALL = has(true, Effect.values());

    @Test public void automaticEchoFollowsTheRoute() {
        MicProcessing auto = MicProcessing.DEFAULT;
        assertEquals(Echo.AUTO, auto.echo);
        MicProcessing.Plan speaker = auto.plan(false, ALL);
        assertTrue(speaker.ownEchoCanceller);
        MicProcessing.Plan headphones = auto.plan(true, ALL);
        assertFalse(headphones.ownEchoCanceller || headphones.echoCanceler);
        assertEquals(MediaRecorder.AudioSource.MIC, headphones.source);
    }

    // Issue 56: with Echo cancellation on or off the speaker's game sound reached the host alike.
    // Android's canceller hears only the device's echo reference, which on that phone left the
    // stream's game-usage output out, so by default Nightjar's own canceller, fed with that sound,
    // runs on the plain microphone.
    @Test public void theStreamsSoundIsCancelledByNightjarsOwnCanceller() {
        MicProcessing.Plan plan = MicProcessing.DEFAULT.plan(false, ALL);
        assertEquals(EchoEngine.OWN, MicProcessing.DEFAULT.engine);
        assertTrue(plan.ownEchoCanceller);
        assertFalse(plan.echoCanceler);
        assertEquals(MediaRecorder.AudioSource.MIC, plan.source);
        assertTrue(plan.missing.isEmpty());
    }

    @Test public void thePlatformEngineKeepsAndroidsCancellerOnTheVoiceSource() {
        MicProcessing platform = new MicProcessing(Echo.ON, false, false, 100, EchoEngine.PLATFORM);
        MicProcessing.Plan plan = platform.plan(false, ALL);
        assertTrue(plan.echoCanceler);
        assertFalse(plan.ownEchoCanceller);
        assertEquals(MediaRecorder.AudioSource.VOICE_COMMUNICATION, plan.source);
        // The voice source tunes the device's own echo handling, even without the effect.
        MicProcessing.Plan bare = platform.plan(false, has(true));
        assertEquals(MediaRecorder.AudioSource.VOICE_COMMUNICATION, bare.source);
        assertEquals(Collections.singletonList(Effect.ECHO_CANCELER), bare.missing);
    }

    @Test public void bothEnginesNeedOnlyOneCanceller() {
        MicProcessing both = new MicProcessing(Echo.ON, false, false, 100, EchoEngine.BOTH);
        MicProcessing.Plan all = both.plan(false, ALL);
        assertTrue(all.ownEchoCanceller && all.echoCanceler);
        assertEquals(MediaRecorder.AudioSource.VOICE_COMMUNICATION, all.source);
        assertTrue(both.plan(false, has(true)).missing.isEmpty());
        assertTrue(both.plan(false, has(false, Effect.ECHO_CANCELER)).missing.isEmpty());
        assertEquals(Collections.singletonList(Effect.ECHO_CANCELER), both.plan(false, has(false)).missing);
    }

    @Test public void aMissingOwnCancellerIsNamedNotSwappedForAndroids() {
        MicProcessing on = new MicProcessing(Echo.ON, false, false, 100);
        MicProcessing.Plan plan = on.plan(false, has(false, Effect.values()));
        assertEquals(Collections.singletonList(Effect.ECHO_CANCELER), plan.missing);
        assertFalse(plan.echoCanceler || plan.ownEchoCanceller);
        assertEquals(MediaRecorder.AudioSource.MIC, plan.source);
    }

    @Test public void onAndOffIgnoreTheRoute() {
        MicProcessing on = new MicProcessing(Echo.ON, false, false, 100);
        assertTrue(on.plan(true, ALL).ownEchoCanceller);
        MicProcessing off = new MicProcessing(Echo.OFF, false, false, 100, EchoEngine.BOTH);
        assertFalse(off.plan(false, ALL).echoCanceler || off.plan(false, ALL).ownEchoCanceller);
        assertEquals(MediaRecorder.AudioSource.MIC, off.plan(false, ALL).source);
    }

    @Test public void missingEffectsAreNamedNotSwapped() {
        MicProcessing all = new MicProcessing(Echo.ON, true, true, 100);
        MicProcessing.Plan plan = all.plan(false, has());
        assertEquals(Arrays.asList(Effect.ECHO_CANCELER, Effect.NOISE_SUPPRESSOR, Effect.AUTOMATIC_GAIN), plan.missing);
        assertFalse(plan.echoCanceler || plan.ownEchoCanceller || plan.noiseSuppressor || plan.automaticGain);
        MicProcessing.Plan some = all.plan(false, has(Effect.NOISE_SUPPRESSOR));
        assertEquals(Arrays.asList(Effect.ECHO_CANCELER, Effect.AUTOMATIC_GAIN), some.missing);
        assertTrue(some.enables(Effect.NOISE_SUPPRESSOR));
    }

    @Test public void theLogNamesTheCancellers() {
        assertEquals("echo canceller Nightjar's", MicrophoneCapture.echoCancellers(false, true));
        assertEquals("echo canceller Android's", MicrophoneCapture.echoCancellers(true, false));
        assertEquals("echo cancellers Nightjar's and Android's", MicrophoneCapture.echoCancellers(true, true));
        assertEquals("no echo canceller", MicrophoneCapture.echoCancellers(false, false));
        assertTrue(MicProcessing.DEFAULT.toString().startsWith("echo auto (own), "));
        assertTrue(new MicProcessing(Echo.OFF, true, false, 100).toString().startsWith("echo off, "));
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
                MicrophoneCapture.notOn(plan, Arrays.asList(Effect.NOISE_SUPPRESSOR, Effect.ECHO_CANCELER), false));
        assertEquals(Collections.singletonList(Effect.ECHO_CANCELER), MicrophoneCapture.notOn(plan, Collections.emptyList(), false));
    }

    @Test public void echoCancellationIsOnWhileEitherPlannedCancellerRuns() {
        List<Effect> none = Collections.emptyList(), androidRefused = Collections.singletonList(Effect.ECHO_CANCELER);
        MicProcessing.Plan own = new MicProcessing(Echo.ON, false, false, 100).plan(false, ALL);
        assertEquals(none, MicrophoneCapture.notOn(own, none, true));
        assertEquals(androidRefused, MicrophoneCapture.notOn(own, none, false));
        MicProcessing.Plan both = new MicProcessing(Echo.ON, false, false, 100, EchoEngine.BOTH).plan(false, ALL);
        assertEquals(none, MicrophoneCapture.notOn(both, androidRefused, true));
        assertEquals(none, MicrophoneCapture.notOn(both, none, false));
        assertEquals(androidRefused, MicrophoneCapture.notOn(both, androidRefused, false));
        // A plan without it already names it once.
        MicProcessing.Plan missing = new MicProcessing(Echo.ON, false, false, 100).plan(false, has(false));
        assertEquals(androidRefused, MicrophoneCapture.notOn(missing, none, false));
    }

    @Test public void theCancellersStatsSayWhatItDid() {
        assertEquals("echo removed 23.4 dB, delay 72 ms", EchoCanceller.describe(new int[] {1, 234, 72, 1}));
        assertEquals("echo removed -0.1 dB", EchoCanceller.describe(new int[] {1, -1, -1, 1}));
        assertEquals("echo not measured, delay 40 ms", EchoCanceller.describe(new int[] {0, 0, 40, 1}));
        assertEquals("heard none of the stream's sound, so it removed nothing", EchoCanceller.describe(new int[] {0, 0, -1, 0}));
        assertEquals("no estimate", EchoCanceller.describe(null));
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
        assertEquals(EchoEngine.OWN, processing.engine);
        config.micEchoEngine = "platform";
        assertEquals(EchoEngine.PLATFORM, config.micProcessing().engine);
        config.micEchoEngine = "bogus";
        assertEquals(EchoEngine.OWN, config.micProcessing().engine);
    }
}
