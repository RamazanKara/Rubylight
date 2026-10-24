package com.limelight.binding.video;

import android.content.Context;

import com.limelight.R;

/**
 * Why this device's video decoder could not start a stream. The native engines only get an error
 * code back (-5 for a decoder that rejects every format), which the stream dialog used to word as
 * a host failure. This keeps the decoder, the format and the codec's own complaint for the dialog
 * and the problem report.
 */
public final class DecoderSetupFailure {
    enum Kind {
        /** The device has no decoder for the negotiated codec (or the size needs HEVC). */
        NO_DECODER,
        /** A decoder exists but rejected every format it was offered. */
        REJECTED,
    }

    private static final int MAX_DETAIL = 160;

    final Kind kind;
    final String decoderName;
    final String codec;
    final int width;
    final int height;
    final int fps;
    final String detail;

    DecoderSetupFailure(Kind kind, String decoderName, String mimeType, int width, int height, int fps, Throwable cause) {
        this.kind = kind;
        this.decoderName = decoderName;
        this.codec = codecName(mimeType);
        this.width = width;
        this.height = height;
        this.fps = fps;
        this.detail = describe(cause);
    }

    static String codecName(String mimeType) {
        if (mimeType == null) return "video";
        switch (mimeType) {
            case "video/avc": return "H.264";
            case "video/hevc": return "HEVC";
            case "video/av01": return "AV1";
            default: return mimeType;
        }
    }

    // "IllegalStateException: configure failed", cut short so one dialog line stays readable.
    static String describe(Throwable cause) {
        if (cause == null) return null;
        String message = cause.getMessage();
        String text = cause.getClass().getSimpleName() + (message == null || message.isEmpty() ? "" : ": " + message);
        return text.length() > MAX_DETAIL ? text.substring(0, MAX_DETAIL) + "..." : text;
    }

    public String message(Context context) {
        if (kind == Kind.NO_DECODER) {
            return context.getString(R.string.decoder_setup_none, codec);
        }
        return context.getString(R.string.decoder_setup_rejected, decoderName, codec, width, height, fps,
                detail == null ? "-" : detail);
    }

    /** One line for the problem report log. */
    @Override
    public String toString() {
        return kind + " " + codec + " " + width + "x" + height + "@" + fps + " decoder=" + decoderName +
                (detail == null ? "" : " cause=" + detail);
    }
}
