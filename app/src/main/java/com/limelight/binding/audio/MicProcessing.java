package com.limelight.binding.audio;

import android.media.MediaRecorder;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * How the microphone is cleaned up before it is sent: echo cancellation, noise suppression,
 * automatic gain and a manual gain. {@link #plan} turns the settings, the audio route and what the
 * device offers into what the capture opens, so the choice is plain logic that tests can check.
 */
public final class MicProcessing {
    /** Echo cancellation: on when this device's own speaker plays, or always, or never. */
    public enum Echo {
        AUTO("auto"), ON("on"), OFF("off");

        public final String key;

        Echo(String key) {
            this.key = key;
        }

        public static Echo of(String key) {
            for (Echo echo : values()) {
                if (echo.key.equals(key)) return echo;
            }
            return AUTO;
        }
    }

    /** An Android audio effect the settings can ask for. */
    public enum Effect { ECHO_CANCELER, NOISE_SUPPRESSOR, AUTOMATIC_GAIN }

    public static final MicProcessing DEFAULT = new MicProcessing(Echo.AUTO, true, false, MicGain.DEFAULT_PERCENT);

    public final Echo echo;
    public final boolean noiseSuppression;
    public final boolean automaticGain;
    public final int gainPercent;

    public MicProcessing(Echo echo, boolean noiseSuppression, boolean automaticGain, int gainPercent) {
        this.echo = echo == null ? Echo.AUTO : echo;
        this.noiseSuppression = noiseSuppression;
        this.automaticGain = automaticGain;
        this.gainPercent = MicGain.clampPercent(gainPercent);
    }

    /** Whether echo cancellation runs for this route: headphones and headsets keep the speaker out of the microphone. */
    public boolean wantsEchoCancellation(boolean privateOutput) {
        return echo == Echo.ON || (echo == Echo.AUTO && !privateOutput);
    }

    /** Which effects the device has; a capture asks Android once per open. */
    public interface Availability {
        boolean has(Effect effect);
    }

    /** What one capture opens. */
    public static final class Plan {
        /** A {@link MediaRecorder.AudioSource} value. */
        public final int source;
        public final boolean echoCanceler, noiseSuppressor, automaticGain;
        /** Effects the settings ask for that this device does not have, in {@link Effect} order. */
        public final List<Effect> missing;

        Plan(int source, boolean echoCanceler, boolean noiseSuppressor, boolean automaticGain, List<Effect> missing) {
            this.source = source;
            this.echoCanceler = echoCanceler;
            this.noiseSuppressor = noiseSuppressor;
            this.automaticGain = automaticGain;
            this.missing = Collections.unmodifiableList(missing);
        }

        /** Whether the effect is on in this plan. */
        public boolean enables(Effect effect) {
            switch (effect) {
                case ECHO_CANCELER: return echoCanceler;
                case NOISE_SUPPRESSOR: return noiseSuppressor;
                default: return automaticGain;
            }
        }
    }

    /**
     * Echo cancellation records from VOICE_COMMUNICATION, the source the device tunes its own echo
     * path for, and adds the AcousticEchoCanceler effect when there is one. Without it the plain
     * MIC source keeps the voice path's processing away. An effect the device lacks is reported in
     * {@link Plan#missing}, never silently swapped for something else.
     */
    public Plan plan(boolean privateOutput, Availability availability) {
        boolean echoWanted = wantsEchoCancellation(privateOutput);
        List<Effect> missing = new ArrayList<>(3);
        boolean echoCanceler = echoWanted && availability.has(Effect.ECHO_CANCELER);
        if (echoWanted && !echoCanceler) missing.add(Effect.ECHO_CANCELER);
        boolean noise = noiseSuppression && availability.has(Effect.NOISE_SUPPRESSOR);
        if (noiseSuppression && !noise) missing.add(Effect.NOISE_SUPPRESSOR);
        boolean gain = automaticGain && availability.has(Effect.AUTOMATIC_GAIN);
        if (automaticGain && !gain) missing.add(Effect.AUTOMATIC_GAIN);
        int source = echoWanted ? MediaRecorder.AudioSource.VOICE_COMMUNICATION : MediaRecorder.AudioSource.MIC;
        return new Plan(source, echoCanceler, noise, gain, missing);
    }

    @Override
    public String toString() {
        return "echo " + echo.key + ", noise suppression " + (noiseSuppression ? "on" : "off") +
                ", automatic gain " + (automaticGain ? "on" : "off") + ", gain " + gainPercent + "%";
    }
}
