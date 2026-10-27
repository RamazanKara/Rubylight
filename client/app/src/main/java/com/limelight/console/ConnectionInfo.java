package com.limelight.console;

import android.content.Context;

import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.utils.WifiFacts;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

/**
 * What the stream menu says about the connection: host, codec, resolution and frame rate, bitrate and
 * link. A snapshot taken when the page opens, from values the stream already holds; nothing here asks
 * the network or the decoder anything.
 */
public final class ConnectionInfo {
    public final String hostName;
    /** The video format flags of the stream (MoonBridge.VIDEO_FORMAT_*). */
    public final int videoFormat;
    public final int width, height, fps;
    public final int bitrateKbps;
    /** Null when Android has not said yet. */
    public final WifiFacts link;
    /** Round-trip time in ms, or -1 when unknown. */
    public final int rttMs;

    public ConnectionInfo(String hostName, int videoFormat, int width, int height, int fps, int bitrateKbps,
                          WifiFacts link, int rttMs) {
        this.hostName = hostName;
        this.videoFormat = videoFormat;
        this.width = width;
        this.height = height;
        this.fps = fps;
        this.bitrateKbps = bitrateKbps;
        this.link = link;
        this.rttMs = rttMs;
    }

    /** From MoonBridge.getEstimatedRttInfo(): the round trip in the top 32 bits, -1 when there is none. */
    public static int rttMs(long rttInfo) {
        return rttInfo == -1 ? -1 : (int) (rttInfo >> 32);
    }

    public static String codec(int videoFormat) {
        return (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0 ? "PyroWave" :
                (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0 ? "AV1" :
                (videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0 ? "HEVC" : "H.264";
    }

    /** Label and value, in the order shown. */
    public static final class Line {
        public final int label;
        public final String value;

        Line(int label, String value) {
            this.label = label;
            this.value = value;
        }
    }

    public List<Line> lines(Context context) {
        List<Line> lines = new ArrayList<>();
        lines.add(new Line(R.string.native_info_host, hostName));
        lines.add(new Line(R.string.native_info_codec, codec(videoFormat)));
        lines.add(new Line(R.string.native_info_resolution,
                context.getString(R.string.native_info_resolution_value, width, height, fps)));
        lines.add(new Line(R.string.native_info_bitrate, context.getString(R.string.native_info_bitrate_value,
                String.format(Locale.getDefault(), bitrateKbps % 1000 == 0 ? "%.0f" : "%.1f", bitrateKbps / 1000.0))));
        lines.add(new Line(R.string.native_info_link, linkText(context)));
        return lines;
    }

    String linkText(Context context) {
        List<String> parts = new ArrayList<>();
        if (link == null || link.link == null) {
            parts.add(context.getString(R.string.native_link_unknown));
        } else {
            switch (link.link) {
                case WIFI:
                    WifiFacts.Band band = link.band();
                    parts.add(link.generation == WifiFacts.UNKNOWN ? context.getString(R.string.native_link_wifi)
                            : link.generation == 0 ? context.getString(R.string.wifi_standard_legacy)
                            : link.generation == 6 && band == WifiFacts.Band.GHZ_6 ? context.getString(R.string.wifi_standard_6e)
                            : context.getString(R.string.wifi_standard, link.generation));
                    if (band != null) {
                        parts.add(context.getString(band == WifiFacts.Band.GHZ_2_4 ? R.string.wifi_band_24
                                : band == WifiFacts.Band.GHZ_5 ? R.string.wifi_band_5 : R.string.wifi_band_6));
                    }
                    int speed = link.linkMbps != WifiFacts.UNKNOWN ? link.linkMbps : link.rxMbps;
                    if (speed != WifiFacts.UNKNOWN) {
                        parts.add(context.getString(R.string.wifi_mbps, speed));
                    }
                    break;
                case ETHERNET:
                    parts.add(context.getString(R.string.native_link_ethernet));
                    break;
                case CELLULAR:
                    parts.add(context.getString(R.string.native_link_cellular));
                    break;
                case OTHER:
                    parts.add(context.getString(R.string.native_link_other));
                    break;
                default:
                    parts.add(context.getString(R.string.native_link_none));
                    break;
            }
        }
        if (rttMs >= 0) {
            parts.add(context.getString(R.string.native_link_rtt, rttMs));
        }
        StringBuilder text = new StringBuilder();
        for (String part : parts) {
            if (text.length() > 0) {
                text.append(" · ");
            }
            text.append(part);
        }
        return text.toString();
    }
}
