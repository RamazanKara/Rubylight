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

    /**
     * Which canceller takes the speaker out of the microphone. {@link #OWN} is Nightjar's
     * {@link EchoCanceller}, fed with the stream's sound itself; {@link #PLATFORM} is Android's
     * AcousticEchoCanceler on the voice source, which hears only what the device routes into its
     * echo reference; on some phones that misses the stream's game sound (issue 56). No
     * setting shows it; debug builds set it for measurements (EchoProbeActivity).
     */
    public enum EchoEngine {
        OWN("own"), PLATFORM("platform"), BOTH("both");

        public final String key;

        EchoEngine(String key) {
            this.key = key;
        }

        public static EchoEngine of(String key) {
            for (EchoEngine engine : values()) {
                if (engine.key.equals(key)) return engine;
            }
            return OWN;
        }

        boolean own() {
            return this != PLATFORM;
        }

        boolean platform() {
            return this != OWN;
        }

        /** Whether a canceller this engine asks for is there. */
        public boolean available(Availability availability) {
            return (own() && availability.hasOwnEchoCanceller()) ||
                    (platform() && availability.has(Effect.ECHO_CANCELER));
        }
    }

    /** An Android audio effect the settings can ask for. */
    public enum Effect { ECHO_CANCELER, NOISE_SUPPRESSOR, AUTOMATIC_GAIN }

    public static final MicProcessing DEFAULT = new MicProcessing(Echo.AUTO, true, false, MicGain.DEFAULT_PERCENT);

    public final Echo echo;
    public final boolean noiseSuppression;
    public final boolean automaticGain;
    public final int gainPercent;
    public final EchoEngine engine;

    public MicProcessing(Echo echo, boolean noiseSuppression, boolean automaticGain, int gainPercent) {
        this(echo, noiseSuppression, automaticGain, gainPercent, EchoEngine.OWN);
    }

    public MicProcessing(Echo echo, boolean noiseSuppression, boolean automaticGain, int gainPercent,
                         EchoEngine engine) {
        this.echo = echo == null ? Echo.AUTO : echo;
        this.noiseSuppression = noiseSuppression;
        this.automaticGain = automaticGain;
        this.gainPercent = MicGain.clampPercent(gainPercent);
        this.engine = engine == null ? EchoEngine.OWN : engine;
    }

    /** Whether echo cancellation runs for this route: headphones and headsets keep the speaker out of the microphone. */
    public boolean wantsEchoCancellation(boolean privateOutput) {
        return echo == Echo.ON || (echo == Echo.AUTO && !privateOutput);
    }

    /** Which effects the device has; a capture asks Android once per open. */
    public interface Availability {
        boolean has(Effect effect);

        /** Whether Nightjar's own echo canceller loads here. */
        default boolean hasOwnEchoCanceller() {
            return false;
        }
    }

    /** What one capture opens. */
    public static final class Plan {
        /** A {@link MediaRecorder.AudioSource} value. */
        public final int source;
        /** Android's effects on the record session. */
        public final boolean echoCanceler, noiseSuppressor, automaticGain;
        /** Nightjar's own echo canceller on each frame, fed with the stream's sound. */
        public final boolean ownEchoCanceller;
        /** Effects the settings ask for that this device does not have, in {@link Effect} order. */
        public final List<Effect> missing;

        Plan(int source, boolean echoCanceler, boolean ownEchoCanceller, boolean noiseSuppressor,
             boolean automaticGain, List<Effect> missing) {
            this.source = source;
            this.echoCanceler = echoCanceler;
            this.ownEchoCanceller = ownEchoCanceller;
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
     * Nightjar's own echo canceller records from the plain MIC source: the voice source's processing
     * would bend the echo it has to model and, on many devices, narrow the voice. Android's
     * canceller records from VOICE_COMMUNICATION, the source the device tunes its own echo path
     * for, and adds the AcousticEchoCanceler effect when there is one. Without echo cancellation the
     * plain MIC source keeps the voice path's processing away. Echo cancellation is missing only
     * when no canceller the engine asks for is there; an effect the device lacks is reported in
     * {@link Plan#missing}, never silently swapped for something else.
     */
    public Plan plan(boolean privateOutput, Availability availability) {
        boolean echoWanted = wantsEchoCancellation(privateOutput);
        List<Effect> missing = new ArrayList<>(3);
        boolean own = echoWanted && engine.own() && availability.hasOwnEchoCanceller();
        boolean echoCanceler = echoWanted && engine.platform() && availability.has(Effect.ECHO_CANCELER);
        if (echoWanted && !engine.available(availability)) missing.add(Effect.ECHO_CANCELER);
        boolean noise = noiseSuppression && availability.has(Effect.NOISE_SUPPRESSOR);
        if (noiseSuppression && !noise) missing.add(Effect.NOISE_SUPPRESSOR);
        boolean gain = automaticGain && availability.has(Effect.AUTOMATIC_GAIN);
        if (automaticGain && !gain) missing.add(Effect.AUTOMATIC_GAIN);
        int source = echoWanted && engine.platform() ?
                MediaRecorder.AudioSource.VOICE_COMMUNICATION : MediaRecorder.AudioSource.MIC;
        return new Plan(source, echoCanceler, own, noise, gain, missing);
    }

    @Override
    public String toString() {
        return "echo " + echo.key + (echo == Echo.OFF ? "" : " (" + engine.key + ")") + ", noise suppression " + (noiseSuppression ? "on" : "off") +
                ", automatic gain " + (automaticGain ? "on" : "off") + ", gain " + gainPercent + "%";
    }
}
