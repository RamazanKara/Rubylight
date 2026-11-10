package com.limelight.console;

import com.limelight.nvstream.http.ComputerDetails;

import java.util.Arrays;
import java.util.List;
import java.util.function.LongSupplier;

/**
 * What a screen last drew its PCs from. The feed reports every PC with every poll, a second or two apart, whether
 * or not anything about it changed; a screen asks here first, so only news reads the library and binds the cards
 * again. It compares every field of a PC ({@link ComputerDetails#fields}) and not only the ones a screen is known
 * to read: a redraw too many costs little, a missed one leaves the screen out of date.
 */
final class HostsDrawn {
    /**
     * The cards say how long ago a game was played, so a screen left open still redraws when the clock has moved
     * on this far.
     */
    static final long CLOCK_STEP_MS = 60_000;

    private final LongSupplier clock;
    private Object[] drawn;

    HostsDrawn() {
        this(System::currentTimeMillis);
    }

    /** @param clock the wall clock in milliseconds; tests hold it still */
    HostsDrawn(LongSupplier clock) {
        this.clock = clock;
    }

    /**
     * A reload starts from these PCs.
     *
     * @param waiting the feed still waits for a saved PC ({@link HostFeed#waiting}); Home's empty state follows it
     */
    void set(List<ComputerDetails> hosts, boolean waiting) {
        drawn = key(hosts, waiting, clock.getAsLong());
    }

    /** The next poll reloads whatever it brings. */
    void forget() {
        drawn = null;
    }

    /** Whether a reload now would start from anything the last one did not have. */
    boolean changed(List<ComputerDetails> hosts, boolean waiting) {
        return drawn == null || !Arrays.deepEquals(drawn, key(hosts, waiting, clock.getAsLong()));
    }

    private static Object[] key(List<ComputerDetails> hosts, boolean waiting, long nowMs) {
        Object[] key = new Object[hosts.size() + 2];
        key[0] = waiting;
        key[1] = nowMs / CLOCK_STEP_MS;
        for (int i = 0; i < hosts.size(); i++) {
            key[i + 2] = hosts.get(i).fields();
        }
        return key;
    }
}
