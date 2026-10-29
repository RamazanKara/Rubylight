package com.limelight.binding.video;

import com.limelight.utils.HdrBrightness;

import java.util.Locale;
import java.util.Objects;

/**
 * Tells a Rubylight host the client display's HDR luminance (control message 0x5531,
 * protocol/core/src/control.rs DisplayCaps) for output diagnostics. Sent at
 * stream start and when the output changes, for example when a foldable switches screens.
 * Luminance reported by Android can change with brightness on the same panel; feeding those
 * changes back into older hosts' HDR metadata restarts the decoder and flashes the picture.
 * Keep the first successfully reported values for each continuous output. One reporter per connection.
 */
public final class DisplayCapsReporter {
    /** Delivers the message; false when it could not go out (it is then retried on the next update). */
    public interface Sender {
        boolean send(boolean hdr, float maxNits, float maxAverageNits, float minNits);
    }

    /** Luminance in nits, 0 when unknown, limited to what the host accepts. */
    // Lower "HDR peaks" are SDR levels some phones report instead; see Caps.of.
    static final float MIN_PLAUSIBLE_HDR_PEAK_NITS = 500;

    public static final class Caps {
        public final boolean hdr;
        public final float maxNits;
        public final float maxAverageNits;
        public final float minNits;

        private Caps(boolean hdr, float maxNits, float maxAverageNits, float minNits) {
            this.hdr = hdr;
            this.maxNits = maxNits;
            this.maxAverageNits = maxAverageNits;
            this.minNits = minNits;
        }

        /**
         * Phones report 0, negative or absurd values when they do not know; those become 0
         * (unknown) because the host drops a message whose peak is outside 1-10 000 nits,
         * whose average is above the peak, or whose black level is 1 nit or more.
         */
        public static Caps of(boolean hdr, float maxNits, float maxAverageNits, float minNits) {
            return of(hdr, maxNits, maxAverageNits, minNits, Float.NaN);
        }

        /**
         * As above; a missing peak is estimated from Android's HDR/SDR ratio (NaN when it has
         * none), as Moonlight V+ does (HdrBrightness.fromDisplay, 3e954d8).
         */
        public static Caps of(boolean hdr, float maxNits, float maxAverageNits, float minNits, float hdrSdrRatio) {
            float max = known(maxNits);
            if (max < 1 || max > 10000) {
                HdrBrightness estimate = HdrBrightness.fromDisplay(maxNits, minNits, maxAverageNits, hdrSdrRatio);
                if (estimate != null) {
                    // An estimate is no SDR level a phone reports as its peak: no plausibility floor.
                    float min = estimate.minNits >= 0.9999f ? 0 : estimate.minNits;
                    return new Caps(hdr, estimate.maxNits, estimate.maxFullFrameNits, min);
                }
                max = 0;
            }
            float average = known(maxAverageNits);
            if (average > 10000) average = 0;
            // HDR panels peak well above 500 nits, but some phones report their SDR level as the
            // HDR peak (a Galaxy Z Fold 7 says 400 for a panel of about 2,600). Below that the
            // host's own default describes the panel better, so the peak and average are unknown.
            if (max != 0 && max < MIN_PLAUSIBLE_HDR_PEAK_NITS) {
                max = 0;
                average = 0;
            }
            if (max != 0 && average > max) average = max;
            float min = known(minNits);
            if (min >= 0.9999f) min = 0;
            return new Caps(hdr, max, average, min);
        }

        private static float known(float nits) {
            return Float.isNaN(nits) || Float.isInfinite(nits) || nits <= 0 ? 0 : nits;
        }

        // The wire carries hundredths of a nit (ten-thousandths for black), rounded like the
        // shared crate does; values that encode the same are the same.
        private static long wire(float nits, float perNit) {
            return (long) (nits * perNit + 0.5f);
        }

        @Override
        public boolean equals(Object other) {
            if (!(other instanceof Caps)) return false;
            Caps caps = (Caps) other;
            return hdr == caps.hdr &&
                    wire(maxNits, 100) == wire(caps.maxNits, 100) &&
                    wire(maxAverageNits, 100) == wire(caps.maxAverageNits, 100) &&
                    wire(minNits, 10000) == wire(caps.minNits, 10000);
        }

        @Override
        public int hashCode() {
            return Objects.hash(hdr, wire(maxNits, 100), wire(maxAverageNits, 100), wire(minNits, 10000));
        }

        @Override
        public String toString() {
            return "HDR " + (hdr ? "on" : "off") +
                    ", peak " + nits(maxNits, "%.0f") +
                    ", frame average " + nits(maxAverageNits, "%.0f") +
                    ", black " + nits(minNits, "%.4f");
        }

        private static String nits(float value, String format) {
            return value == 0 ? "unknown" : String.format(Locale.US, format, value) + " nits";
        }
    }

    private final Sender sender;
    private String lastDisplay;
    private Caps lastSent;

    public DisplayCapsReporter(Sender sender) {
        this.sender = sender;
    }

    /**
     * Samples a new output, identified independently of brightness and refresh rate. Values on
     * the current output stay fixed until another output is selected. Failed sends remain retryable.
     */
    public synchronized Caps update(String display, boolean hdr, float maxNits, float maxAverageNits, float minNits) {
        return update(display, hdr, maxNits, maxAverageNits, minNits, Float.NaN);
    }

    /** As above, with the output's HDR/SDR ratio (NaN when unknown) for a missing peak. */
    public synchronized Caps update(String display, boolean hdr, float maxNits, float maxAverageNits, float minNits,
                                    float hdrSdrRatio) {
        Objects.requireNonNull(display);
        if (display.equals(lastDisplay)) {
            return null;
        }
        Caps caps = Caps.of(hdr, maxNits, maxAverageNits, minNits, hdrSdrRatio);
        if (caps.equals(lastSent)) {
            lastDisplay = display;
            return null;
        }
        if (!sender.send(caps.hdr, caps.maxNits, caps.maxAverageNits, caps.minNits)) {
            return null;
        }
        lastDisplay = display;
        lastSent = caps;
        return caps;
    }
}
