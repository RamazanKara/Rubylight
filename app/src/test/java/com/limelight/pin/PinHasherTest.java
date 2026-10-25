package com.limelight.pin;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.nio.charset.StandardCharsets;
import java.security.spec.KeySpec;

import javax.crypto.SecretKeyFactory;
import javax.crypto.spec.PBEKeySpec;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class PinHasherTest {
    private static byte[] hex(String text) {
        String compact = text.replaceAll("\\s", "");
        byte[] out = new byte[compact.length() / 2];
        for (int i = 0; i < out.length; i++) out[i] = (byte) Integer.parseInt(compact.substring(2 * i, 2 * i + 2), 16);
        return out;
    }

    private static byte[] ascii(String text) {
        return text.getBytes(StandardCharsets.US_ASCII);
    }

    @Test
    public void matchesRfc7914VectorWithOneIteration() {
        byte[] expected = hex("55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc"
                + "49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783");
        assertArrayEquals(expected, PinHasher.pbkdf2(ascii("passwd"), ascii("salt"), 1, 64));
    }

    @Test
    public void matchesRfc7914VectorWithManyIterations() {
        byte[] expected = hex("4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56"
                + "a1d425a1225833549adb841b51c9b3176a272bdebba1d078478f62b397f33c8d");
        assertArrayEquals(expected, PinHasher.pbkdf2(ascii("Password"), ascii("NaCl"), 80_000, 64));
    }

    @Test
    public void agreesWithTheJvmImplementationForPins() throws Exception {
        SecretKeyFactory factory = SecretKeyFactory.getInstance("PBKDF2WithHmacSHA256");
        byte[] salt = hex("00112233445566778899aabbccddeeff");
        for (String pin : new String[] {"0000", "1234", "98765432", "000000"}) {
            KeySpec spec = new PBEKeySpec(pin.toCharArray(), salt, 25_000, 256);
            byte[] jvm = factory.generateSecret(spec).getEncoded();
            assertArrayEquals(pin, jvm, PinHasher.pbkdf2(ascii(pin), salt, 25_000, 32));
        }
    }

    @Test
    public void verifyAcceptsTheRightPinAndRejectsOthers() {
        String stored = PinHasher.create(ascii("4821"), 20_000);
        assertTrue(PinHasher.verify(stored, ascii("4821")));
        assertFalse(PinHasher.verify(stored, ascii("4822")));
        assertFalse(PinHasher.verify(stored, ascii("482")));
        assertFalse(PinHasher.verify(stored, ascii("48210")));
        assertFalse(PinHasher.verify(stored, new byte[0]));
    }

    @Test
    public void everyRecordGetsItsOwnSalt() {
        String a = PinHasher.create(ascii("1111"), 20_000);
        String b = PinHasher.create(ascii("1111"), 20_000);
        assertNotEquals(a, b);
        assertTrue(PinHasher.verify(a, ascii("1111")));
        assertTrue(PinHasher.verify(b, ascii("1111")));
    }

    @Test
    public void theRecordHoldsTheVersionIterationsSaltAndKeyButNotThePin() {
        String stored = PinHasher.create(ascii("246813"), 20_000);
        String[] parts = stored.split("\\$");
        assertEquals(4, parts.length);
        assertEquals("v1", parts[0]);
        assertEquals("20000", parts[1]);
        assertFalse(stored.contains("246813"));
    }

    @Test
    public void verifyRefusesRecordsItDidNotWrite() {
        byte[] pin = ascii("1234");
        assertFalse(PinHasher.verify(null, pin));
        assertFalse(PinHasher.verify("", pin));
        assertFalse(PinHasher.verify("v1$20000$AAAA", pin));
        assertFalse(PinHasher.verify("v2$20000$AAAAAAAAAAAAAAAAAAAAAA==$AAAA", pin));
        assertFalse(PinHasher.verify("v1$0$AAAAAAAAAAAAAAAAAAAAAA==$AAAA", pin));
        assertFalse(PinHasher.verify("v1$notanumber$AAAAAAAAAAAAAAAAAAAAAA==$AAAA", pin));
        assertFalse(PinHasher.verify("v1$99999999$AAAAAAAAAAAAAAAAAAAAAA==$AAAA", pin));
        assertFalse(PinHasher.verify("v1$20000$$", pin));
    }

    @Test
    public void iterationCountIsKeptInASaneRange() {
        assertEquals(PinHasher.MIN_ITERATIONS, PinHasher.scale(Long.MAX_VALUE / 2));
        assertEquals(PinHasher.MAX_ITERATIONS, PinHasher.scale(1));
        int calibrated = PinHasher.calibrate();
        assertTrue(calibrated >= PinHasher.MIN_ITERATIONS && calibrated <= PinHasher.MAX_ITERATIONS);
    }

    @Test
    public void constantTimeEqualsComparesLengthAndContent() {
        assertTrue(PinHasher.constantTimeEquals(ascii("1234"), ascii("1234")));
        assertFalse(PinHasher.constantTimeEquals(ascii("1234"), ascii("1235")));
        assertFalse(PinHasher.constantTimeEquals(ascii("1234"), ascii("12345")));
    }
}
