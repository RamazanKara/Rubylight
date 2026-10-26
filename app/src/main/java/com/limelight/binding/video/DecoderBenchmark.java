package com.limelight.binding.video;

import android.annotation.TargetApi;
import android.content.Context;
import android.content.SharedPreferences;
import android.content.res.AssetFileDescriptor;
import android.graphics.SurfaceTexture;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaExtractor;
import android.media.MediaFormat;
import android.os.Build;
import android.preference.PreferenceManager;
import android.view.Surface;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.utils.DeviceResults;

import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * Times each hardware decoder on a short 1080p60 game-like clip with the same low-latency
 * settings a stream uses, once per phone and app version, so Automatic can skip codecs that
 * decode measurably slower here than another.
 */
public final class DecoderBenchmark {
    static final String[] MIMES = {"video/avc", "video/hevc", "video/av01"};
    static final String[] CODEC_NAMES = {"H.264", "HEVC", "AV1"};
    private static final String[] CLIPS = {"bench/h264.mp4", "bench/hevc.mp4", "bench/av1.mp4"};
    public static final int CLIP_WIDTH = 1920;
    public static final int CLIP_HEIGHT = 1080;
    public static final int CLIP_FPS = 60;
    // Recorded beside each result so shared results show which low-latency switches the decoder offers.
    static final String FEATURE_PREFIX = "lowlatency|";
    static final String VENDOR_KEYS_PREFIX = "vendorkeys|";
    private static final String PREFS = "decoder_benchmark";
    private static final int ROUNDS = 3;
    private static final long TIMEOUT_US = 200_000;
    // A codec is only skipped when it is clearly slower: both margins must be exceeded.
    static final float SLOWER_RATIO = 1.15f;
    static final int SLOWER_US = 700;

    private static volatile boolean running;

    private DecoderBenchmark() {}

    /** Starts the benchmark in the background unless this build already measured this phone. */
    public static void runIfNeeded(Context context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q || running) return;
        Context app = context.getApplicationContext();
        boolean disableHevcLowLatency = disableHevcLowLatency(app);
        boolean chipsetTuning = chipsetTuning(app);
        String benchmarkKey = key(disableHevcLowLatency, chipsetTuning);
        String glRenderer = GlPreferences.readPreferences(app).glRenderer;
        // Decoder lists depend on the GPU; Game records it on the first stream.
        if (glRenderer == null || glRenderer.isEmpty() || prefs(app).getString("key", "").equals(benchmarkKey)) return;
        running = true;
        Thread thread = new Thread(() -> {
            try {
                MediaCodecHelper.initialize(app, glRenderer);
                SharedPreferences.Editor editor = prefs(app).edit().clear();
                for (int i = 0; i < MIMES.length; i++) {
                    MediaCodecInfo decoder = MediaCodecHelper.findProbableSafeDecoder(MIMES[i], -1);
                    if (decoder == null) continue;
                    long us = measure(app, decoder, MIMES[i], CLIPS[i], disableHevcLowLatency, chipsetTuning);
                    LimeLog.info("Decoder benchmark " + decoder.getName() + ": " + (us > 0 ? us + " us/frame" : "failed"));
                    if (us > 0) {
                        editor.putLong(decoder.getName() + "|" + MIMES[i], us);
                        String suffix = MIMES[i] + "|" + decoder.getName();
                        editor.putBoolean(FEATURE_PREFIX + suffix,
                                MediaCodecHelper.decoderSupportsAndroidRLowLatency(decoder, MIMES[i]));
                        List<String> params = MediaCodecHelper.getIntegerVendorParameters(decoder.getName());
                        if (params != null) {
                            editor.putString(VENDOR_KEYS_PREFIX + suffix,
                                    String.join(",", VendorLowLatencyKeys.forSupported(params).keySet()));
                        }
                    }
                }
                // Keep the key for the options actually measured if the user changes them mid-run.
                editor.putString("key", benchmarkKey).apply();
            } catch (Exception e) {
                LimeLog.warning("Decoder benchmark failed: " + e);
            } finally {
                running = false;
            }
        }, "Decoder benchmark");
        thread.setPriority(Thread.MIN_PRIORITY);
        thread.start();
    }

    /** Median decode time per frame in microseconds, or 0 when unmeasured. */
    public static long result(Context context, MediaCodecInfo decoder, String mime) {
        if (decoder == null) return 0;
        SharedPreferences prefs = prefs(context);
        if (!prefs.getString("key", "").equals(key(context))) return 0;
        return prefs.getLong(decoder.getName() + "|" + mime, 0);
    }

    /**
     * Index into MIMES of codecs that are clearly slower than the fastest usable one.
     * H.264 is never skipped, and with HDR it does not count because it can't carry HDR.
     */
    static boolean[] slower(long[] us, boolean hdr) {
        long best = Long.MAX_VALUE;
        for (int i = hdr ? 1 : 0; i < us.length; i++) {
            if (us[i] > 0) best = Math.min(best, us[i]);
        }
        boolean[] skip = new boolean[us.length];
        if (best == Long.MAX_VALUE) return skip;
        for (int i = 1; i < us.length; i++) {
            skip[i] = us[i] > 0 && us[i] > best * SLOWER_RATIO && us[i] - best > SLOWER_US;
        }
        return skip;
    }

    /** "H.264 2.1 ms · HEVC 2.4 ms · AV1 4.0 ms", or null before the first measurement. */
    public static String summary(Context context) {
        SharedPreferences prefs = prefs(context);
        if (!prefs.getString("key", "").equals(key(context))) return null;
        List<String> parts = new ArrayList<>();
        for (String entry : prefs.getAll().keySet()) {
            for (int i = 0; i < MIMES.length; i++) {
                if (entry.endsWith("|" + MIMES[i])) {
                    parts.add(CODEC_NAMES[i] + String.format(Locale.US, " %.1f ms", prefs.getLong(entry, 0) / 1000f));
                }
            }
        }
        Collections.sort(parts);
        return parts.isEmpty() ? null : String.join(" · ", parts);
    }

    /** Adds this build's measurements on this phone to a shareable results export. */
    public static void addTo(Context context, DeviceResults results) {
        SharedPreferences prefs = prefs(context);
        results.clipWidth = CLIP_WIDTH;
        results.clipHeight = CLIP_HEIGHT;
        results.clipFps = CLIP_FPS;
        results.decoderBenchmarkMeasured = prefs.getString("key", "").equals(key(context));
        if (results.decoderBenchmarkMeasured) results.decoders.addAll(decoders(prefs.getAll()));
    }

    /** Stored results in codec order; low-latency details are null when they were not recorded. */
    static List<DeviceResults.Decoder> decoders(Map<String, ?> stored) {
        List<DeviceResults.Decoder> decoders = new ArrayList<>();
        for (int i = 0; i < MIMES.length; i++) {
            String suffix = "|" + MIMES[i];
            List<String> names = new ArrayList<>();
            for (Map.Entry<String, ?> entry : stored.entrySet()) {
                String key = entry.getKey();
                if (key.endsWith(suffix) && key.length() > suffix.length() && entry.getValue() instanceof Long) {
                    names.add(key.substring(0, key.length() - suffix.length()));
                }
            }
            Collections.sort(names);
            for (String name : names) {
                Object feature = stored.get(FEATURE_PREFIX + MIMES[i] + "|" + name);
                Object keys = stored.get(VENDOR_KEYS_PREFIX + MIMES[i] + "|" + name);
                List<String> keyList = null;
                if (keys instanceof String) {
                    keyList = new ArrayList<>();
                    for (String key : ((String) keys).split(",")) {
                        if (!key.isEmpty()) keyList.add(key);
                    }
                }
                decoders.add(new DeviceResults.Decoder(CODEC_NAMES[i], MIMES[i], name,
                        (Long) stored.get(name + suffix),
                        feature instanceof Boolean ? (Boolean) feature : null, keyList));
            }
        }
        return decoders;
    }

    private static SharedPreferences prefs(Context context) {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    private static boolean disableHevcLowLatency(Context context) {
        return PreferenceManager.getDefaultSharedPreferences(context).getBoolean(
                PreferenceConfiguration.DISABLE_HEVC_LOW_LATENCY_PREF_STRING, false);
    }

    private static boolean chipsetTuning(Context context) {
        return PreferenceManager.getDefaultSharedPreferences(context).getBoolean(
                PreferenceConfiguration.CHIPSET_TUNING_PREF_STRING, true);
    }

    private static String key(Context context) {
        return key(disableHevcLowLatency(context), chipsetTuning(context));
    }

    private static String key(boolean disableHevcLowLatency, boolean chipsetTuning) {
        return Build.FINGERPRINT + "|" + BuildConfig.VERSION_CODE + "|hevc-low-latency-off=" + disableHevcLowLatency +
                "|chipset-tuning=" + chipsetTuning;
    }

    // Feeds one frame at a time and waits for it, as a low-latency stream does.
    @TargetApi(Build.VERSION_CODES.Q)
    private static long measure(Context context, MediaCodecInfo decoder, String mime, String clip,
                                boolean disableHevcLowLatency, boolean chipsetTuning) {
        MediaExtractor extractor = new MediaExtractor();
        MediaCodec codec = null;
        SurfaceTexture texture = null;
        Surface surface = null;
        try (AssetFileDescriptor fd = context.getAssets().openFd(clip)) {
            extractor.setDataSource(fd.getFileDescriptor(), fd.getStartOffset(), fd.getLength());
            extractor.selectTrack(0);
            MediaFormat format = extractor.getTrackFormat(0);
            List<ByteBuffer> frames = new ArrayList<>();
            ByteBuffer sample = ByteBuffer.allocate(4 << 20);
            int size;
            while ((size = extractor.readSampleData(sample, 0)) >= 0) {
                ByteBuffer frame = ByteBuffer.allocate(size);
                sample.limit(size);
                frame.put(sample).flip();
                frames.add(frame);
                sample.clear();
                extractor.advance();
            }
            if (frames.isEmpty()) return 0;

            MediaCodecHelper.setDecoderLowLatencyOptions(format, decoder, 0, true, true, true, disableHevcLowLatency,
                    chipsetTuning);
            texture = new SurfaceTexture(false);
            texture.setDefaultBufferSize(format.getInteger(MediaFormat.KEY_WIDTH), format.getInteger(MediaFormat.KEY_HEIGHT));
            surface = new Surface(texture);
            codec = MediaCodec.createByCodecName(decoder.getName());
            codec.configure(format, surface, null, 0);
            codec.start();

            MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
            List<Long> samples = new ArrayList<>();
            long pts = 0;
            for (int round = 0; round < ROUNDS; round++) {
                // The clip starts with its only keyframe; flush so each round decodes it again.
                if (round > 0) codec.flush();
                for (int i = 0; i < frames.size(); i++) {
                    ByteBuffer frame = frames.get(i);
                    int input = codec.dequeueInputBuffer(TIMEOUT_US);
                    if (input < 0) return 0;
                    ByteBuffer buffer = codec.getInputBuffer(input);
                    buffer.clear();
                    buffer.put(frame.duplicate());
                    long start = System.nanoTime();
                    codec.queueInputBuffer(input, 0, frame.remaining(), pts, 0);
                    pts += 16_667;
                    int output;
                    do {
                        output = codec.dequeueOutputBuffer(info, TIMEOUT_US);
                    } while (output == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED || output == MediaCodec.INFO_OUTPUT_BUFFERS_CHANGED);
                    if (output < 0) return 0;
                    long elapsed = (System.nanoTime() - start) / 1000;
                    codec.releaseOutputBuffer(output, true);
                    // Round one warms up clocks and caches, and keyframes are rare in a stream.
                    if (round > 0 && i > 0) samples.add(elapsed);
                }
            }
            if (samples.isEmpty()) return 0;
            Long[] sorted = samples.toArray(new Long[0]);
            Arrays.sort(sorted);
            return sorted[sorted.length / 2];
        } catch (Exception e) {
            LimeLog.warning("Decoder benchmark " + decoder.getName() + " failed: " + e);
            return 0;
        } finally {
            if (codec != null) {
                try {
                    codec.stop();
                } catch (Exception ignored) {
                }
                codec.release();
            }
            if (surface != null) surface.release();
            if (texture != null) texture.release();
            extractor.release();
        }
    }
}
