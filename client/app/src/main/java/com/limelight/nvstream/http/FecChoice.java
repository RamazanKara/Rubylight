package com.limelight.nvstream.http;

/**
 * The video FEC this client asks for at launch (rubylight_protocol::fec_report::FecChoice). It is sent only
 * to a host with {@link HostCapabilities.Feature#CLIENT_FEC}; without a choice the launch query is unchanged.
 */
public final class FecChoice {
    /** The /launch and /resume query parameter. */
    public static final String LAUNCH_PARAMETER = "rlFecPercentage";
    /** The host's own fec_percentage and adaptive FEC, as when the parameter is absent. */
    public static final FecChoice HOST = new FecChoice("host");
    /** Statistical FEC from this client's FEC reports. */
    public static final FecChoice AUTO = new FecChoice("auto");

    /** The query value: host, auto or a percentage. */
    public final String wireValue;

    private FecChoice(String wireValue) {
        this.wireValue = wireValue;
    }

    /** A fixed percentage, 0 to 255; the host still caps it at its fec_max_percentage. */
    public static FecChoice fixed(int percentage) {
        if (percentage < 0 || percentage > 255) {
            throw new IllegalArgumentException("FEC percentage " + percentage + " is not 0-255");
        }
        return new FecChoice(Integer.toString(percentage));
    }

    /** A stored list_fec_choice value (host, auto or 0-255); anything else is the host's own. */
    public static FecChoice fromPreference(String value) {
        if (AUTO.wireValue.equals(value)) return AUTO;
        if (value != null && value.matches("[0-9]{1,3}")) {
            int percentage = Integer.parseInt(value);
            if (percentage <= 255) return fixed(percentage);
        }
        return HOST;
    }

    /**
     * The x-ml-video.fecPercentage a Foundation Sunshine host takes in the ANNOUNCE
     * (rubylight_protocol::fec_report::FecChoice::summary_percentage, V+'s videoFecPercentage):
     * -2 for the host's own, -1 for automatic, else the fixed share, at most 100.
     */
    public int summaryPercentage() {
        if (equals(HOST)) return -2;
        if (equals(AUTO)) return -1;
        return Math.min(Integer.parseInt(wireValue), 100);
    }

    @Override public boolean equals(Object other) {
        return other instanceof FecChoice && ((FecChoice) other).wireValue.equals(wireValue);
    }

    @Override public int hashCode() {
        return wireValue.hashCode();
    }

    @Override public String toString() {
        return wireValue;
    }
}
