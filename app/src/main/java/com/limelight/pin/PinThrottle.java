package com.limelight.pin;

/**
 * Backoff after wrong PINs. Four tries are free; the fifth wrong one locks for 30 s and each further one doubles
 * that, up to 30 min. Times are on the elapsed-realtime clock, which phone clock changes cannot move; a reboot
 * restarts that clock, so the stored lock is then renewed at its current step rather than dropped.
 * Pure: {@link PinStore} keeps the state and supplies the clock.
 */
final class PinThrottle {
    static final int FREE_FAILURES = 4;
    static final long BASE_MS = 30_000;
    static final long MAX_MS = 30 * 60_000L;

    private PinThrottle() {
    }

    /** What is stored: wrong tries in a row, when the lock ends and on which boot that time counts. */
    static final class State {
        final int fails;
        final long lockUntil;
        final long lockBoot;

        State(int fails, long lockUntil, long lockBoot) {
            this.fails = fails;
            this.lockUntil = lockUntil;
            this.lockBoot = lockBoot;
        }

        static final State CLEAN = new State(0, 0, 0);
    }

    /** The wait that follows the {@code fails}-th wrong try in a row. */
    static long delayAfter(int fails) {
        if (fails <= FREE_FAILURES) return 0;
        int doublings = Math.min(fails - FREE_FAILURES - 1, 16);
        return Math.min(MAX_MS, BASE_MS << doublings);
    }

    /** The state seen from boot {@code boot}: a lock written on another boot is renewed at its current step. */
    static State onBoot(State s, long now, long boot) {
        if (s.lockBoot == boot) return s;
        long delay = delayAfter(s.fails);
        return new State(s.fails, delay > 0 ? now + delay : 0, boot);
    }

    /** How long input stays refused; never more than the longest step, whatever was stored. */
    static long remainingMs(State s, long now) {
        return Math.max(0, Math.min(MAX_MS, s.lockUntil - now));
    }

    /** The state after one more wrong (or not yet verified) try. */
    static State failed(State s, long now, long boot) {
        int fails = s.fails + 1;
        long delay = delayAfter(fails);
        return new State(fails, delay > 0 ? now + delay : 0, boot);
    }
}
