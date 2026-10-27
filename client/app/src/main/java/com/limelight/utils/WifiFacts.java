package com.limelight.utils;

/** An immutable snapshot of the active connection's Wi-Fi link. Unknown values stay unknown. */
public final class WifiFacts {
    public static final int UNKNOWN = Integer.MIN_VALUE;
    public enum Link { WIFI, ETHERNET, CELLULAR, OTHER, NONE }
    public enum Band { GHZ_2_4, GHZ_5, GHZ_6 }
    public enum Signal { EXCELLENT, GOOD, FAIR, WEAK }

    /** Null when Android did not say which kind of network carries the stream. */
    public final Link link;
    public final int rssiDbm, frequencyMhz, widthMhz, txMbps, rxMbps, linkMbps;
    /** Wi-Fi generation (4 to 7), 0 for older standards, or {@link #UNKNOWN}. */
    public final int generation;

    public WifiFacts(Link link, int rssiDbm, int frequencyMhz, int widthMhz, int txMbps, int rxMbps,
                     int linkMbps, int generation) {
        this.link = link;
        this.rssiDbm = rssiDbm < -126 || rssiDbm >= 0 ? UNKNOWN : rssiDbm;
        this.frequencyMhz = frequencyMhz > 0 ? frequencyMhz : UNKNOWN;
        this.widthMhz = widthMhz > 0 ? widthMhz : UNKNOWN;
        this.txMbps = txMbps > 0 ? txMbps : UNKNOWN;
        this.rxMbps = rxMbps > 0 ? rxMbps : UNKNOWN;
        this.linkMbps = linkMbps > 0 ? linkMbps : UNKNOWN;
        this.generation = generation;
    }

    public static WifiFacts notWifi(Link link) {
        return new WifiFacts(link, UNKNOWN, UNKNOWN, UNKNOWN, UNKNOWN, UNKNOWN, UNKNOWN, UNKNOWN);
    }

    public Band band() {
        if (frequencyMhz >= 2400 && frequencyMhz < 2500) return Band.GHZ_2_4;
        if (frequencyMhz >= 4900 && frequencyMhz < 5925) return Band.GHZ_5;
        if (frequencyMhz >= 5925 && frequencyMhz <= 7125) return Band.GHZ_6;
        return null;
    }

    public int channel() {
        Band band = band();
        if (band == null) return UNKNOWN;
        switch (band) {
            case GHZ_2_4: return frequencyMhz == 2484 ? 14 : (frequencyMhz - 2407) / 5;
            case GHZ_5: return (frequencyMhz - 5000) / 5;
            default: return frequencyMhz == 5935 ? 2 : (frequencyMhz - 5950) / 5;
        }
    }

    public Signal signal() {
        if (rssiDbm == UNKNOWN) return null;
        if (rssiDbm >= -55) return Signal.EXCELLENT;
        if (rssiDbm >= -67) return Signal.GOOD;
        if (rssiDbm >= -75) return Signal.FAIR;
        return Signal.WEAK;
    }

    public boolean onWifi() {
        return link == Link.WIFI;
    }

    public boolean slowBand() {
        return onWifi() && band() == Band.GHZ_2_4;
    }

    public boolean weakSignal() {
        return onWifi() && signal() == Signal.WEAK;
    }

    /** Maps android.net.wifi.WifiInfo.getWifiStandard() to a Wi-Fi generation. */
    public static int generationOf(int wifiStandard) {
        switch (wifiStandard) {
            case 1: return 0;  // legacy 802.11a/b/g
            case 4: return 4;  // 802.11n
            case 5: return 5;  // 802.11ac
            case 6: return 6;  // 802.11ax
            case 8: return 7;  // 802.11be
            default: return UNKNOWN;
        }
    }

    /**
     * The channel width an access point announces in one information element, or {@link #UNKNOWN}
     * when the element does not say. Callers take the widest answer across the elements.
     *
     * @param id element ID; 255 for extension elements
     * @param idExt extension ID for ID 255
     * @param body the element body, without ID, length or extension ID
     */
    public static int widthFromElement(int id, int idExt, byte[] body) {
        if (body == null) return UNKNOWN;
        if (id == 61 && body.length >= 2) {
            // HT Operation: secondary channel offset plus "STA channel width" bit.
            return (body[1] & 0x04) != 0 ? 40 : 20;
        }
        if (id == 191 && body.length >= 3) return vhtWidth(body, 0);
        if (id == 255 && idExt == 36 && body.length >= 6) {
            // HE Operation: parameters (3), BSS color (1), basic HE-MCS (2), then optional fields.
            boolean vht = (body[1] & 0x40) != 0, cohosted = (body[1] & 0x80) != 0, sixGhz = (body[2] & 0x02) != 0;
            int offset = 6;
            int width = UNKNOWN;
            if (vht) {
                if (body.length < offset + 3) return UNKNOWN;
                width = vhtWidth(body, offset);
                offset += 3;
            }
            if (cohosted) offset++;
            if (sixGhz) {
                if (body.length < offset + 2) return width;
                switch (body[offset + 1] & 0x03) {
                    case 0: return 20;
                    case 1: return 40;
                    case 2: return 80;
                    default: return 160;
                }
            }
            return width;
        }
        if (id == 255 && idExt == 106 && body.length >= 5) {
            // EHT Operation: parameters (1), basic EHT-MCS (4), then the operation information.
            if ((body[0] & 0x01) == 0 || body.length < 6) return UNKNOWN;
            switch (body[5] & 0x07) {
                case 0: return 20;
                case 1: return 40;
                case 2: return 80;
                case 3: return 160;
                case 4: return 320;
                default: return UNKNOWN;
            }
        }
        return UNKNOWN;
    }

    private static int vhtWidth(byte[] body, int offset) {
        switch (body[offset] & 0xff) {
            case 1: return body[offset + 2] != 0 ? 160 : 80;
            case 2:
            case 3: return 160;
            default: return UNKNOWN;
        }
    }
}
