package com.limelight.nvstream.http;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.IOException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

public final class HardwareMetrics {
    static final int MAX_RESPONSE_BYTES = 256 * 1024;
    private static final String[] CPU_METRICS = {"cpu_usage_percent"};
    private static final String[] GPU_METRICS = {
            "gpu_usage_percent", "gpu_clock_mhz", "gpu_memory_clock_mhz",
            "gpu_temperature_c", "gpu_hotspot_temperature_c", "gpu_memory_temperature_c",
            "gpu_voltage_mv", "gpu_fan_rpm", "gpu_fan_percent", "gpu_power_w",
            "gpu_board_power_w", "gpu_vram_used_mb", "gpu_vram_usage_percent"
    };

    public final long sampledAtUnixMs;
    public final int intervalMs;
    public final Map<String, Double> cpu;
    public final List<Gpu> gpus;

    public HardwareMetrics(long sampledAtUnixMs, int intervalMs, Map<String, Double> cpu, List<Gpu> gpus) {
        this.sampledAtUnixMs = sampledAtUnixMs;
        this.intervalMs = intervalMs;
        this.cpu = Collections.unmodifiableMap(new LinkedHashMap<>(cpu));
        this.gpus = Collections.unmodifiableList(new ArrayList<>(gpus));
    }

    public static final class Gpu {
        public final int id;
        public final String name;
        public final Map<String, Double> metrics;

        public Gpu(int id, String name, Map<String, Double> metrics) {
            this.id = id;
            this.name = name;
            this.metrics = Collections.unmodifiableMap(new LinkedHashMap<>(metrics));
        }
    }

    public static final class UnsupportedVersionException extends IOException {
        public UnsupportedVersionException() {
            super("Unsupported hardware metrics version");
        }
    }

    public static HardwareMetrics parse(String json) throws IOException {
        if (json.length() > MAX_RESPONSE_BYTES) {
            throw new IOException("Hardware metrics response too large");
        }
        try {
            JSONObject root = new JSONObject(json);
            if (integer(root, "version", 0, Integer.MAX_VALUE) != 1) {
                throw new UnsupportedVersionException();
            }
            long sampledAt = integer(root, "sampled_at_unix_ms", 0, Long.MAX_VALUE);
            int interval = (int) integer(root, "interval_ms", 500, 10000);
            Map<String, Double> cpu = readings(root.getJSONObject("cpu"), CPU_METRICS);
            JSONArray entries = root.getJSONArray("gpus");
            if (entries.length() > 64) {
                throw new IOException("Too many hardware metrics GPUs");
            }
            List<Gpu> gpus = new ArrayList<>(entries.length());
            for (int i = 0; i < entries.length(); i++) {
                JSONObject entry = entries.getJSONObject(i);
                int id = (int) integer(entry, "id", Integer.MIN_VALUE, Integer.MAX_VALUE);
                Object name = entry.get("name");
                if (!(name instanceof String) || ((String) name).length() > 512) {
                    throw new IOException("Invalid hardware metrics GPU name");
                }
                gpus.add(new Gpu(id, (String) name, readings(entry.getJSONObject("metrics"), GPU_METRICS)));
            }
            return new HardwareMetrics(sampledAt, interval, cpu, gpus);
        } catch (JSONException e) {
            throw new IOException("Invalid hardware metrics response", e);
        }
    }

    private static long integer(JSONObject object, String key, long min, long max) throws IOException, JSONException {
        Object value = object.get(key);
        if (!(value instanceof Integer) && !(value instanceof Long)) {
            throw new IOException("Invalid hardware metrics " + key);
        }
        long number = ((Number) value).longValue();
        if (number < min || number > max) {
            throw new IOException("Invalid hardware metrics " + key);
        }
        return number;
    }

    private static Map<String, Double> readings(JSONObject object, String[] supported) {
        Map<String, Double> result = new LinkedHashMap<>();
        for (String key : supported) {
            Object value = object.opt(key);
            if (value instanceof Number) {
                double number = ((Number) value).doubleValue();
                if (!Double.isNaN(number) && !Double.isInfinite(number) && number >= 0) {
                    result.put(key, number);
                }
            }
        }
        return result;
    }
}
