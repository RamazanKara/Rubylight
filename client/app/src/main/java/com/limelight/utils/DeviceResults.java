package com.limelight.utils;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;
import java.util.regex.Pattern;

/**
 * One device's decoder benchmark and latency test results, as a redacted, versioned JSON text
 * the user copies or shares themselves. Nothing is sent automatically.
 *
 * Only fixed fields are written. Codec and vendor key names must look like identifiers or are
 * omitted; free text such as the model or GPU name goes through {@link RedactedLog#redact},
 * which removes known PC names, addresses, e-mail addresses, IDs and long tokens.
 */
public final class DeviceResults {
    public static final String FORMAT = "nightjar-device-results";
    public static final int VERSION = 1;
    /** The public GitHub issue form for results. It carries no device data; the user pastes the text. */
    public static final String REPORT_FORM_URL =
            "https://github.com/RamazanKara/nightjar/issues/new?template=device-results.yml";
    static final int MAX_TEXT = 96;
    static final String OMITTED = "[omitted]";
    /** JSON names of the latency test metrics, indexed like {@link LatencyBenchmarkStats}. */
    static final String[] METRICS = {"frameIntervalMs", "frameCallbackDelayMs", "inputDispatchAgeMs", "timerWakeupDelayMs"};
    private static final Pattern IDENTIFIER = Pattern.compile("[A-Za-z0-9._/-]{1,96}");
    private static final Pattern VENDOR_KEY = Pattern.compile("vendor\\.[A-Za-z0-9._-]{1,120}");

    public String appVersion;
    public int appBuild;
    public String manufacturer;
    public String model;
    public String codename;
    public String socManufacturer;
    public String socModel;
    public String hardware;
    public String board;
    public String androidRelease;
    public int sdk;
    public String gpu;

    public boolean decoderBenchmarkMeasured;
    public int clipWidth;
    public int clipHeight;
    public int clipFps;
    public final List<Decoder> decoders = new ArrayList<>();

    /** Null until the latency test has run in this screen. */
    public Latency latency;

    public static final class Decoder {
        public final String codec;
        public final String mime;
        public final String name;
        public final long medianDecodeUs;
        /** Null when unknown, for example from a benchmark recorded by an older version. */
        public final Boolean featureLowLatency;
        /** Null when unknown; empty when the decoder declares none. */
        public final List<String> vendorLowLatencyKeys;

        public Decoder(String codec, String mime, String name, long medianDecodeUs,
                       Boolean featureLowLatency, List<String> vendorLowLatencyKeys) {
            this.codec = codec;
            this.mime = mime;
            this.name = name;
            this.medianDecodeUs = medianDecodeUs;
            this.featureLowLatency = featureLowLatency;
            this.vendorLowLatencyKeys = vendorLowLatencyKeys;
        }
    }

    public static final class Latency {
        public double panelHz;
        public Integer audioSampleRate;
        public Integer audioFramesPerBuffer;
        /** {count, mean, p95, p99} per metric as {@link LatencyBenchmarkStats#summary} returns them. */
        public final double[][] metrics = new double[METRICS.length][];

        /** Parses an AudioManager property; null when absent or not a positive number. */
        public static Integer audioProperty(String value) {
            if (value == null) return null;
            try {
                int parsed = Integer.parseInt(value.trim());
                return parsed > 0 ? parsed : null;
            } catch (NumberFormatException e) {
                return null;
            }
        }
    }

    public String toJson() {
        Json json = new Json();
        json.open();
        json.field("format").string(FORMAT);
        json.field("version").number(VERSION);
        json.field("app").open();
        json.field("name").string("Nightjar");
        json.field("version").string(text(appVersion));
        json.field("build").number(appBuild);
        json.close();
        json.field("device").open();
        json.field("manufacturer").string(text(manufacturer));
        json.field("model").string(text(model));
        json.field("codename").string(text(codename));
        json.field("socManufacturer").string(text(socManufacturer));
        json.field("socModel").string(text(socModel));
        json.field("hardware").string(text(hardware));
        json.field("board").string(text(board));
        json.field("android").string(text(androidRelease));
        json.field("sdk").number(sdk);
        json.field("gpu").string(text(gpu));
        json.close();

        json.field("decoderBenchmark").open();
        json.field("measured").bool(decoderBenchmarkMeasured);
        if (decoderBenchmarkMeasured) {
            json.field("clip").string(clipWidth + "x" + clipHeight + "@" + clipFps);
        }
        json.field("results").openArray();
        for (Decoder decoder : decoders) {
            json.item().open();
            json.field("codec").string(text(decoder.codec));
            json.field("mime").string(identifier(decoder.mime));
            json.field("decoder").string(identifier(decoder.name));
            json.field("medianDecodeMs").decimal(decoder.medianDecodeUs > 0 ? decoder.medianDecodeUs / 1000.0 : -1);
            json.field("featureLowLatency");
            if (decoder.featureLowLatency == null) json.nul();
            else json.bool(decoder.featureLowLatency);
            json.field("vendorLowLatencyKeys");
            if (decoder.vendorLowLatencyKeys == null) {
                json.nul();
            } else {
                List<String> keys = new ArrayList<>();
                for (String key : decoder.vendorLowLatencyKeys) {
                    if (key != null && VENDOR_KEY.matcher(key).matches() && !keys.contains(key)) keys.add(key);
                }
                Collections.sort(keys);
                json.openArray();
                for (String key : keys) json.item().string(key);
                json.closeArray();
            }
            json.close();
        }
        json.closeArray();
        json.close();

        json.field("latencyTest");
        if (latency == null) {
            json.nul();
        } else {
            json.open();
            json.field("panelHz").decimal(latency.panelHz);
            json.field("audioSampleRate").integer(latency.audioSampleRate);
            json.field("audioFramesPerBuffer").integer(latency.audioFramesPerBuffer);
            for (int i = 0; i < METRICS.length; i++) {
                double[] summary = latency.metrics[i];
                json.field(METRICS[i]);
                if (summary == null || summary.length < 4 || summary[0] <= 0) {
                    json.nul();
                    continue;
                }
                json.open();
                json.field("samples").number((long) summary[0]);
                json.field("mean").decimal(summary[1]);
                json.field("p95").decimal(summary[2]);
                json.field("p99").decimal(summary[3]);
                json.close();
            }
            json.close();
        }
        json.close();
        return json.toString();
    }

    /** Free text, redacted and capped; null when empty. */
    static String text(String value) {
        if (value == null) return null;
        String redacted = RedactedLog.redact(value.trim());
        if (redacted == null || redacted.isEmpty()) return null;
        return redacted.length() > MAX_TEXT ? redacted.substring(0, MAX_TEXT) : redacted;
    }

    /** Codec, MIME and decoder names: kept only when they look like an identifier. */
    static String identifier(String value) {
        if (value == null) return null;
        return IDENTIFIER.matcher(value).matches() ? value : OMITTED;
    }

    private static final class Json {
        private final StringBuilder out = new StringBuilder();
        private final ArrayList<Boolean> first = new ArrayList<>();

        void open() {
            out.append('{');
            first.add(true);
        }

        void close() {
            boolean empty = first.remove(first.size() - 1);
            if (!empty) newline();
            out.append('}');
        }

        void openArray() {
            out.append('[');
            first.add(true);
        }

        void closeArray() {
            boolean empty = first.remove(first.size() - 1);
            if (!empty) newline();
            out.append(']');
        }

        Json field(String name) {
            item();
            quote(name);
            out.append(": ");
            return this;
        }

        Json item() {
            int last = first.size() - 1;
            if (!first.get(last)) out.append(',');
            first.set(last, false);
            newline();
            return this;
        }

        void string(String value) {
            if (value == null) nul();
            else quote(value);
        }

        void number(long value) {
            out.append(value);
        }

        void integer(Integer value) {
            if (value == null) nul();
            else out.append(value.intValue());
        }

        void decimal(double value) {
            if (Double.isNaN(value) || Double.isInfinite(value) || value < 0) nul();
            else out.append(String.format(Locale.US, "%.2f", value));
        }

        void bool(boolean value) {
            out.append(value);
        }

        void nul() {
            out.append("null");
        }

        private void newline() {
            out.append('\n');
            for (int i = 0; i < first.size(); i++) out.append("  ");
        }

        private void quote(String value) {
            out.append('"');
            for (int i = 0; i < value.length(); i++) {
                char c = value.charAt(i);
                if (c == '"' || c == '\\') out.append('\\').append(c);
                else if (c < 0x20 || c == ' ' || c == ' ') out.append(String.format(Locale.US, "\\u%04x", (int) c));
                else out.append(c);
            }
            out.append('"');
        }

        @Override public String toString() {
            return out.toString();
        }
    }
}
