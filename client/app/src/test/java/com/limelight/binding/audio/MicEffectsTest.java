package com.limelight.binding.audio;

import android.app.Application;
import android.media.audiofx.AudioEffect;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.shadows.ShadowAudioEffect;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.EnumSet;
import java.util.List;
import java.util.Set;

import static org.junit.Assert.*;

/** An effect that is created but refuses to turn on counts as not on, so the stream says so. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class MicEffectsTest {
    private static final MicProcessing.Availability ALL = effect -> true;
    // Android's canceller is the AcousticEchoCanceler effect these tests attach.
    private final MicProcessing allOn = new MicProcessing(MicProcessing.Echo.ON, true, true, MicGain.DEFAULT_PERCENT,
            MicProcessing.EchoEngine.PLATFORM);
    private final MicProcessing allOff = new MicProcessing(MicProcessing.Echo.OFF, false, false, MicGain.DEFAULT_PERCENT);
    private final List<AudioEffect> created = new ArrayList<>();
    private Effects effects;

    @After public void release() {
        if (effects != null) effects.release();
    }

    @Test public void effectsThatTurnOnAreNotReported() {
        effects = new Effects(EnumSet.noneOf(MicProcessing.Effect.class));
        MicProcessing.Plan plan = allOn.plan(false, ALL);
        assertEquals(Collections.emptyList(), effects.attach(1, plan));
        assertEquals(3, created.size());
        for (AudioEffect effect : created) assertTrue(effect.getEnabled());
    }

    @Test public void anEffectThatRefusesToTurnOnIsInTheMissingEffectsNotice() {
        effects = new Effects(EnumSet.of(MicProcessing.Effect.NOISE_SUPPRESSOR));
        MicProcessing.Plan plan = allOn.plan(false, ALL);
        List<MicProcessing.Effect> failed = effects.attach(1, plan);
        assertEquals(Collections.singletonList(MicProcessing.Effect.NOISE_SUPPRESSOR), failed);
        assertEquals(Collections.singletonList(MicProcessing.Effect.NOISE_SUPPRESSOR), MicrophoneCapture.notOn(plan, failed, false));
        assertTrue(created.get(MicProcessing.Effect.ECHO_CANCELER.ordinal()).getEnabled());
        assertTrue(created.get(MicProcessing.Effect.AUTOMATIC_GAIN.ordinal()).getEnabled());
    }

    @Test public void everyRefusedEffectIsReportedInOrder() {
        effects = new Effects(EnumSet.allOf(MicProcessing.Effect.class));
        assertEquals(Arrays.asList(MicProcessing.Effect.values()), effects.attach(1, allOn.plan(false, ALL)));
    }

    @Test public void aRefusedTurnOffIsNotAMissingEffect() {
        // Nothing was asked for, so there is no setting to point at; the failure is only logged.
        effects = new Effects(EnumSet.allOf(MicProcessing.Effect.class));
        assertEquals(Collections.emptyList(), effects.attach(1, allOff.plan(false, ALL)));
    }

    private final class Effects extends MicEffects {
        private final Set<MicProcessing.Effect> refusing;

        Effects(Set<MicProcessing.Effect> refusing) {
            this.refusing = refusing;
        }

        @Override public boolean has(MicProcessing.Effect effect) {
            return true;
        }

        @Override AudioEffect create(MicProcessing.Effect effect, int session) {
            AudioEffect made = super.create(effect, session);
            assertNotNull(effect + " is created", made);
            if (refusing.contains(effect)) {
                ((ShadowAudioEffect) Shadow.extract(made)).setErrorCode(AudioEffect.ERROR_INVALID_OPERATION);
            }
            created.add(made);
            return made;
        }
    }
}
