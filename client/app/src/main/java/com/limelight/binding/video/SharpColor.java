package com.limelight.binding.video;

import androidx.annotation.VisibleForTesting;

import com.limelight.LimeLog;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.function.IntSupplier;

/**
 * "Sharper colored text": native 4:4:4 where this device decodes it and the PC encodes it, else packed
 * full chroma, else neither. The decision is nightjar-core's {@code sharp_color::decide}, the one desktop
 * Nightjar runs, reached through libnightjar_core (protocol/android/src/sharp_color.rs). The device's packed
 * modes cost a verification decode, so they are asked for only when the decision needs them: the first
 * answer may be {@code need_device_modes}, and the request goes again with them.
 * SharpColorTest holds this class to protocol/client/src/sharp_color_contract.json, the file
 * nightjar-core's test reads too.
 */
public final class SharpColor {
    private SharpColor() { }

    /** One JSON request in, one JSON answer out: {@code {"ok": …}} or {@code {"error": code, "detail": …}}. */
    public interface Calls {
        String call(String request);
    }

    private static Calls calls;

    private static synchronized Calls calls() {
        if (calls == null) {
            System.loadLibrary("nightjar_core");
            calls = SharpColor::call;
        }
        return calls;
    }

    /** JVM tests cannot load the library. */
    @VisibleForTesting
    public static synchronized void use(Calls replacement) {
        calls = replacement;
    }

    private static native String call(String request);

    /** What the decision reads. Formats are MoonBridge VIDEO_FORMAT bits, PC support ServerCodecModeSupport bits. */
    public static final class Request {
        /** The setting is Automatic. */
        public boolean auto;
        /** HDR is on for this stream. */
        public boolean hdr;
        /** 10-bit color (SDR) is on for this stream. */
        public boolean sdr10;
        /** PyroWave is the chosen codec. */
        public boolean pyroWave;
        /** This engine can carry packed full chroma (the new streaming engine, not turned off for this session). */
        public boolean packedEngine;
        /** What the codec choice offers without 4:4:4 and 10-bit SDR. */
        public int base;
        /** The 4:4:4 and 10-bit SDR formats this device decodes and draws. */
        public int decodable;
        /** Why the others are not decodable, for the log. */
        public String decoderReason = "";
        /** The PC's ServerCodecModeSupport. */
        public int hostScm;
        /** The PC's packed modes before the launch, or -1 when unknown. */
        public int hostPacked = -1;
        /** The PC refused packed the last time and has not said its modes since. */
        public boolean packedRefusedBefore;

        JSONObject json(Integer deviceModes) throws JSONException {
            return new JSONObject().put("op", "decide").put("auto", auto).put("hdr", hdr).put("sdr10", sdr10)
                    .put("pyrowave", pyroWave).put("packed_engine", packedEngine).put("base", base)
                    .put("decodable", decodable).put("decoder_reason", decoderReason).put("host_scm", hostScm)
                    .put("host_packed", hostPacked < 0 ? JSONObject.NULL : hostPacked)
                    .put("packed_refused_before", packedRefusedBefore)
                    .put("device_modes", deviceModes == null ? JSONObject.NULL : deviceModes);
        }
    }

    /** The decision. */
    public static final class Pick {
        public enum Kind { NATIVE, PACKED, WITHOUT }

        /** Why neither runs, as the shared model names it. */
        public static final String OFF = "off", PYROWAVE = "pyrowave", HDR = "hdr", DECODER = "decoder", HOST = "host",
                DEVICE = "device", NO_COMMON_MODE = "no_common_mode";
        /** The model could not be reached or answered something else: the stream starts without either. */
        public static final String UNANSWERED = "unanswered";

        public final Kind kind;
        /** NATIVE: the formats to offer. */
        public final int formats;
        /** PACKED: the modes the handshake can use. */
        public final int modes;
        /** WITHOUT: why. */
        public final String why;
        /** WITHOUT for the decoder: the decoder check's reason, else null. */
        public final String detail;
        /** WITHOUT: the model's word on whether the player hears it, else null. */
        private final Boolean tell;

        private Pick(Kind kind, int formats, int modes, String why, String detail, Boolean tell) {
            this.kind = kind;
            this.formats = formats;
            this.modes = modes;
            this.why = why;
            this.detail = detail;
            this.tell = tell;
        }

        static Pick without(String why) {
            return new Pick(Kind.WITHOUT, 0, 0, why, null, null);
        }

        /** Whether the player hears that the stream starts without it: the setting, PyroWave and HDR are silent. */
        public boolean worthSaying() {
            if (kind != Kind.WITHOUT) return false;
            if (tell != null) return tell;
            return DECODER.equals(why) || HOST.equals(why) || DEVICE.equals(why) || NO_COMMON_MODE.equals(why);
        }

        /** The PC's side ruled it out, which picks the notice's wording. */
        public boolean fromHost() {
            return HOST.equals(why) || NO_COMMON_MODE.equals(why);
        }

        @Override
        public String toString() {
            switch (kind) {
                case NATIVE: return "native 4:4:4, formats 0x" + Integer.toHexString(formats);
                case PACKED: return "packed full chroma, modes 0x" + Integer.toHexString(modes);
                default: return "neither (" + why + (detail != null && !detail.isEmpty() ? ": " + detail : "") + ")";
            }
        }
    }

    private static JSONObject ask(JSONObject request) throws JSONException {
        String answer = calls().call(request.toString());
        JSONObject json = new JSONObject(answer);
        JSONObject ok = json.optJSONObject("ok");
        if (ok == null) LimeLog.warning("Sharper colored text: model answered " + answer);
        return ok;
    }

    /** The pick an answer's {@code ok} object names. */
    public static Pick read(JSONObject ok) throws JSONException {
        switch (ok.getString("pick")) {
            case "native": return new Pick(Pick.Kind.NATIVE, ok.getInt("formats"), 0, null, null, null);
            case "packed": return new Pick(Pick.Kind.PACKED, 0, ok.getInt("modes"), null, null, null);
            case "without": {
                String detail = ok.optString("detail", "");
                return new Pick(Pick.Kind.WITHOUT, 0, 0, ok.getString("why"), detail.isEmpty() ? null : detail,
                        ok.has("tell") ? ok.getBoolean("tell") : null);
            }
            default: throw new JSONException("unknown pick " + ok);
        }
    }

    /**
     * Decides for one start. {@code devicePacked} runs at most once, and only when the decision reaches
     * packed full chroma.
     */
    public static Pick decide(Request request, IntSupplier devicePacked) {
        try {
            JSONObject ok = ask(request.json(null));
            if (ok != null && ok.optBoolean("need_device_modes", false)) {
                ok = ask(request.json(devicePacked.getAsInt()));
            }
            return ok == null ? Pick.without(Pick.UNANSWERED) : read(ok);
        } catch (JSONException | LinkageError | RuntimeException e) {
            LimeLog.warning("Sharper colored text: model unavailable: " + e);
            return Pick.without(Pick.UNANSWERED);
        }
    }
}
