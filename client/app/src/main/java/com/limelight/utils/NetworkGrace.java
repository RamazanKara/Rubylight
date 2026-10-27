package com.limelight.utils;

/**
 * How long a running stream waits for a silent network (a Wi-Fi roam or dropout, a VPN moving
 * between Wi-Fi and mobile data) before it ends. Off by default: the control stream then keeps
 * its own 10 second timeout. Only the client side is covered; the host has its own timeout.
 */
public final class NetworkGrace {
    /** The control stream's built-in timeout. A grace period must be longer to change anything. */
    public static final int DEFAULT_TIMEOUT_SECONDS = 10;
    static final int[] CHOICES_SECONDS = {0, 20, 30, 60};

    private NetworkGrace() { }

    /** A stored list value in seconds. Anything that is not one of the offered choices is off. */
    public static int parseSeconds(String stored) {
        if (stored == null) return 0;
        int seconds;
        try {
            seconds = Integer.parseInt(stored.trim());
        } catch (NumberFormatException e) {
            return 0;
        }
        for (int choice : CHOICES_SECONDS) {
            if (choice == seconds) return seconds;
        }
        return 0;
    }

    /** True when the setting changes the stream at all. */
    public static boolean enabled(int seconds) {
        return seconds > DEFAULT_TIMEOUT_SECONDS;
    }

    /** The value for MoonBridge.setNetworkGracePeriodMs(); 0 keeps the native default. */
    public static int nativeMilliseconds(int seconds) {
        return enabled(seconds) ? seconds * 1000 : 0;
    }

    /**
     * True when a stream that ended with this code may have waited out the whole grace period
     * first. The control peer timeout ends the stream with -1, an ENet transport failure; a
     * host that drops the session without a reason reports the same code, so the message
     * shown for it must allow for both.
     */
    public static boolean mayHaveGivenUp(int seconds, int errorCode) {
        return enabled(seconds) && errorCode == -1;
    }
}
