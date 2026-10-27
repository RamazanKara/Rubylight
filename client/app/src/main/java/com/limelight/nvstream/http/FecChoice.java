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
}
