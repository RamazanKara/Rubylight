package com.limelight.pin;

import android.util.Base64;

import java.security.GeneralSecurityException;
import java.security.SecureRandom;
import java.util.Arrays;
import java.util.regex.Pattern;

import javax.crypto.Mac;
import javax.crypto.spec.SecretKeySpec;

/**
 * Salted PBKDF2-HMAC-SHA256 for the PIN. Written over {@code Mac} because SecretKeyFactory's PBKDF2WithHmacSHA256
 * needs Android 8 and the app supports 5. Stored as {@code v1$iterations$salt$key}, the two binary parts in Base64.
 * Nothing here logs or keeps a PIN; callers pass the digits as bytes and get to zero them.
 */
public final class PinHasher {
    static final int KEY_BYTES = 32;
    static final int SALT_BYTES = 16;
    /** The iteration count is measured on this device so a check costs about this long. */
    static final long TARGET_MS = 150;
    static final int MIN_ITERATIONS = 20_000;
    static final int MAX_ITERATIONS = 600_000;
    private static final int PROBE_ITERATIONS = 10_000;
    /** A stored count above this is corrupt or hostile, not something this class wrote. */
    private static final int MAX_STORED_ITERATIONS = 5_000_000;
    private static final String VERSION = "v1";
    private static final SecureRandom RANDOM = new SecureRandom();

    private PinHasher() {
    }

    /** PBKDF2 with HMAC-SHA256 (RFC 8018). */
    public static byte[] pbkdf2(byte[] password, byte[] salt, int iterations, int keyBytes) {
        if (password.length == 0) throw new IllegalArgumentException("empty password");
        try {
            Mac mac = Mac.getInstance("HmacSHA256");
            mac.init(new SecretKeySpec(password, "HmacSHA256"));
            int hLen = mac.getMacLength();
            int blocks = (keyBytes + hLen - 1) / hLen;
            byte[] out = new byte[keyBytes];
            byte[] block = new byte[salt.length + 4];
            System.arraycopy(salt, 0, block, 0, salt.length);
            for (int i = 1; i <= blocks; i++) {
                block[salt.length] = (byte) (i >>> 24);
                block[salt.length + 1] = (byte) (i >>> 16);
                block[salt.length + 2] = (byte) (i >>> 8);
                block[salt.length + 3] = (byte) i;
                byte[] u = mac.doFinal(block);
                byte[] t = u.clone();
                for (int c = 1; c < iterations; c++) {
                    u = mac.doFinal(u);
                    for (int j = 0; j < t.length; j++) t[j] ^= u[j];
                }
                System.arraycopy(t, 0, out, (i - 1) * hLen, Math.min(hLen, keyBytes - (i - 1) * hLen));
            }
            return out;
        } catch (GeneralSecurityException e) {
            throw new IllegalStateException(e);
        }
    }

    /** A new record for {@code pin} with a fresh salt. The caller still owns (and zeroes) {@code pin}. */
    public static String create(byte[] pin, int iterations) {
        byte[] salt = new byte[SALT_BYTES];
        RANDOM.nextBytes(salt);
        return format(iterations, salt, pbkdf2(pin, salt, iterations, KEY_BYTES));
    }

    /** Whether {@code pin} matches {@code stored}; false for anything that is not a record this class wrote. */
    public static boolean verify(String stored, byte[] pin) {
        if (stored == null || pin.length == 0) return false;
        String[] parts = stored.split(Pattern.quote("$"), -1);
        if (parts.length != 4 || !VERSION.equals(parts[0])) return false;
        try {
            int iterations = Integer.parseInt(parts[1]);
            if (iterations < 1 || iterations > MAX_STORED_ITERATIONS) return false;
            byte[] salt = Base64.decode(parts[2], Base64.NO_WRAP);
            byte[] expected = Base64.decode(parts[3], Base64.NO_WRAP);
            if (salt.length == 0 || expected.length == 0) return false;
            byte[] actual = pbkdf2(pin, salt, iterations, expected.length);
            boolean same = constantTimeEquals(expected, actual);
            Arrays.fill(actual, (byte) 0);
            return same;
        } catch (IllegalArgumentException e) {
            return false;
        }
    }

    /** The iteration count that makes one check take about {@link #TARGET_MS} on this device. */
    public static int calibrate() {
        byte[] probe = {'1', '2', '3', '4'};
        byte[] salt = new byte[SALT_BYTES];
        pbkdf2(probe, salt, 1_000, KEY_BYTES); // warm up the JIT and the provider
        long start = System.nanoTime();
        pbkdf2(probe, salt, PROBE_ITERATIONS, KEY_BYTES);
        return scale(System.nanoTime() - start);
    }

    /** Scales the probe run's time to the target and keeps the result in a sane range. */
    static int scale(long probeNanos) {
        double perIteration = Math.max(1, probeNanos) / (double) PROBE_ITERATIONS;
        double wanted = TARGET_MS * 1_000_000d / perIteration;
        return (int) Math.max(MIN_ITERATIONS, Math.min(MAX_ITERATIONS, wanted));
    }

    static String format(int iterations, byte[] salt, byte[] key) {
        return VERSION + "$" + iterations + "$" + Base64.encodeToString(salt, Base64.NO_WRAP)
                + "$" + Base64.encodeToString(key, Base64.NO_WRAP);
    }

    static boolean constantTimeEquals(byte[] a, byte[] b) {
        int diff = a.length ^ b.length;
        for (int i = 0; i < Math.min(a.length, b.length); i++) diff |= a[i] ^ b[i];
        return diff == 0;
    }
}
