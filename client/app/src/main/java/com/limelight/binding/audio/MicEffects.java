package com.limelight.binding.audio;

import android.media.audiofx.AcousticEchoCanceler;
import android.media.audiofx.AudioEffect;
import android.media.audiofx.AutomaticGainControl;
import android.media.audiofx.NoiseSuppressor;

import com.limelight.LimeLog;

/**
 * Android's capture effects on one recording session. Each effect the device has is created and set
 * to what the plan says, so "off" also turns off an effect the voice source would add by itself.
 */
public class MicEffects implements MicProcessing.Availability {
    private final AudioEffect[] effects = new AudioEffect[MicProcessing.Effect.values().length];

    /** What this device offers, from Android's effect list. */
    public static final MicProcessing.Availability DEVICE = effect -> {
        try {
            switch (effect) {
                case ECHO_CANCELER: return AcousticEchoCanceler.isAvailable();
                case NOISE_SUPPRESSOR: return NoiseSuppressor.isAvailable();
                default: return AutomaticGainControl.isAvailable();
            }
        } catch (RuntimeException e) {
            LimeLog.warning("Microphone: could not query " + effect + ": " + e);
            return false;
        }
    };

    @Override
    public boolean has(MicProcessing.Effect effect) {
        return DEVICE.has(effect);
    }

    /**
     * Attaches the effects to {@code session} as {@code plan} says. Returns the effects that were on
     * the plan but could not be created or turned on; the caller reports them.
     */
    public java.util.List<MicProcessing.Effect> attach(int session, MicProcessing.Plan plan) {
        java.util.List<MicProcessing.Effect> failed = new java.util.ArrayList<>(0);
        for (MicProcessing.Effect effect : MicProcessing.Effect.values()) {
            if (!has(effect)) continue;
            boolean on = plan.enables(effect);
            AudioEffect created = null;
            try {
                created = create(effect, session);
                if (created != null) {
                    // Some HALs, and any after an audio server restart, refuse without throwing.
                    int status = created.setEnabled(on);
                    if (status != AudioEffect.SUCCESS) {
                        throw new IllegalStateException("setEnabled(" + on + ") returned " + status);
                    }
                }
            } catch (RuntimeException e) {
                LimeLog.warning("Microphone: " + effect + " failed: " + e);
                if (created != null) created.release();
                created = null;
            }
            effects[effect.ordinal()] = created;
            if (created == null && on) failed.add(effect);
        }
        return failed;
    }

    // Package-visible so tests can hand in effects that refuse to turn on.
    AudioEffect create(MicProcessing.Effect effect, int session) {
        switch (effect) {
            case ECHO_CANCELER: return AcousticEchoCanceler.create(session);
            case NOISE_SUPPRESSOR: return NoiseSuppressor.create(session);
            default: return AutomaticGainControl.create(session);
        }
    }

    public void release() {
        for (int i = 0; i < effects.length; i++) {
            if (effects[i] != null) {
                try {
                    effects[i].release();
                } catch (RuntimeException ignored) {
                }
                effects[i] = null;
            }
        }
    }
}
