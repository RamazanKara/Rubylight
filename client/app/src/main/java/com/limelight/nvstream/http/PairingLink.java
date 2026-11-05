package com.limelight.nvstream.http;

import java.io.ByteArrayOutputStream;
import java.math.BigInteger;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.security.cert.CertificateEncodingException;
import java.security.cert.X509Certificate;
import java.util.HashMap;
import java.util.Map;

/**
 * The pairing link a host shows as a QR code (Experimental):
 * {@code rubylight://pair?v=1&host=&port=&otp=&pass=&id=&fp=&name=}. It carries the one-time PIN
 * and the passphrase it is hashed with, where to reach the host, and the SHA-256 of the host
 * certificate's DER; pairing is refused when the host presents another certificate.
 *
 * <p>Reads links exactly like the protocol crate's {@code pairing_uri}: another scheme, more than
 * {@link #MAX_LENGTH} bytes, a version other than 1, a missing or empty host, otp or fp, a host with
 * anything but an address's characters, a port that is 0 or not a number, a fingerprint that is not
 * 64 hex digits, a bad percent escape, text that is not UTF-8 and a repeated key refuse it. Unknown
 * keys are ignored; {@code pass}, {@code id} and {@code name} may be absent.
 *
 * <p>A link can come from any web page, so one opened from outside the app asks first, showing
 * {@link #shownName()} and {@link #shownAddress()}.
 */
public final class PairingLink {
    public static final String PREFIX = "rubylight://pair?";
    /** The longest link read, in bytes; a real one stays far below it. */
    static final int MAX_LENGTH = 4096;
    /** The longest address a link may name, as DNS allows. */
    private static final int MAX_HOST_LENGTH = 255;
    /** How many characters of a link's name are shown. */
    private static final int MAX_SHOWN_NAME = 64;

    public final String host;
    public final int port;
    public final String otp;
    public final String passphrase;
    /** The host's {@code uniqueid}, empty when the link has none. */
    public final String uniqueId;
    private final byte[] certSha256;
    /** The host's name, for display; empty when the link has none. */
    public final String name;
    /** The link as it was read. It holds the PIN and passphrase: never for a log. */
    public final String text;

    private PairingLink(String text, String host, int port, String otp, String passphrase, String uniqueId,
                        byte[] certSha256, String name) {
        this.text = text;
        this.host = host;
        this.port = port;
        this.otp = otp;
        this.passphrase = passphrase;
        this.uniqueId = uniqueId;
        this.certSha256 = certSha256;
        this.name = name;
    }

    /** True when the text is meant as a pairing link, whether or not it is a valid one. */
    public static boolean looksLikeLink(String text) {
        return text != null && text.trim().startsWith(PREFIX);
    }

    /** The link, or null when it is refused (see the class comment). Surrounding spaces are ignored. */
    public static PairingLink parse(String text) {
        if (text == null) {
            return null;
        }
        text = text.trim();
        // The first test spares a very long text from being encoded for the second.
        if (!text.startsWith(PREFIX) || text.length() > MAX_LENGTH
                || text.getBytes(StandardCharsets.UTF_8).length > MAX_LENGTH) {
            return null;
        }
        Map<String, String> values = new HashMap<>();
        for (String part : text.substring(PREFIX.length()).split("&", -1)) {
            int equals = part.indexOf('=');
            if (equals < 0) {
                return null;
            }
            String key = part.substring(0, equals);
            String value = decode(part.substring(equals + 1));
            if (value == null || values.put(key, value) != null) {
                return null;
            }
        }
        if (!"1".equals(values.get("v"))) {
            return null;
        }
        String host = required(values, "host");
        String port = required(values, "port");
        String otp = required(values, "otp");
        String fp = required(values, "fp");
        if (host == null || !addressLike(host) || port == null || otp == null || fp == null || !port.matches("[0-9]+")) {
            return null;
        }
        BigInteger number = new BigInteger(port);
        byte[] fingerprint = fingerprint(fp);
        if (number.signum() == 0 || number.compareTo(BigInteger.valueOf(65535)) > 0 || fingerprint == null) {
            return null;
        }
        return new PairingLink(text, host, number.intValue(), otp, orEmpty(values.get("pass")), orEmpty(values.get("id")),
                fingerprint, orEmpty(values.get("name")));
    }

    /**
     * The link's name for a confirmation: without control characters and the invisible ones that can
     * reorder or hide text, each run of spaces one space, so that it cannot pass for the address line,
     * and cut to a length a dialog can show. Empty when none is left.
     */
    public String shownName() {
        StringBuilder out = new StringBuilder();
        int shown = 0;
        boolean space = false;
        for (int at = 0; at < name.length(); ) {
            int c = name.codePointAt(at);
            at += Character.charCount(c);
            if (hiddenChar(c)) {
                continue;
            }
            if (blank(c)) {
                space = true;
                continue;
            }
            if (shown == MAX_SHOWN_NAME) {
                break;
            }
            if (space && shown > 0) {
                out.append(' ');
                shown++;
                if (shown == MAX_SHOWN_NAME) {
                    break;
                }
            }
            space = false;
            out.appendCodePoint(c);
            shown++;
        }
        return out.toString();
    }

    private static boolean blank(int c) {
        return Character.isWhitespace(c) || Character.isSpaceChar(c);
    }

    /** Where the link pairs, for a confirmation: {@code host:port}, {@code [v6-address]:port}. */
    public String shownAddress() {
        return (host.indexOf(':') >= 0 && !host.startsWith("[") ? "[" + host + "]" : host) + ":" + port;
    }

    /**
     * A name or an IP address ({@code pc.local}, {@code 192.168.4.2}, {@code ::1},
     * {@code [fe80::1%eth0]}): nothing that can end the authority of a URL, or make the address read
     * as something else. A {@code %} only starts an IPv6 literal's zone, because a URL parser may
     * decode one anywhere else ({@code 192.168.4.2%2Eevil.example}), and then connect elsewhere than
     * the dialog says.
     */
    private static boolean addressLike(String host) {
        return host.length() <= MAX_HOST_LENGTH && host.matches(
                "[A-Za-z0-9._-]+"
                        + "|\\[[0-9A-Fa-f:.]*:[0-9A-Fa-f:.]*(%[A-Za-z0-9._-]+)?\\]"
                        + "|[0-9A-Fa-f:.]*:[0-9A-Fa-f:.]*(%[A-Za-z0-9._-]+)?");
    }

    /**
     * Controls, Unicode's format characters (Cf), and what else draws nothing: combining and variation
     * marks with no shape, Hangul and Khmer fillers, the blank Braille cell and the tag block. The same
     * list as the protocol crate, so both clients show one name the same.
     */
    private static boolean hiddenChar(int c) {
        return Character.isISOControl(c) || c == 0xAD || c == 0x34F || (c >= 0x600 && c <= 0x605) || c == 0x61C
                || c == 0x6DD || c == 0x70F || (c >= 0x890 && c <= 0x891) || c == 0x8E2
                || (c >= 0x115F && c <= 0x1160) || (c >= 0x17B4 && c <= 0x17B5) || (c >= 0x180B && c <= 0x180F)
                || (c >= 0x200B && c <= 0x200F) || (c >= 0x2028 && c <= 0x202E) || (c >= 0x2060 && c <= 0x206F)
                || c == 0x2800 || c == 0x3164 || (c >= 0xFE00 && c <= 0xFE0F) || c == 0xFEFF || c == 0xFFA0
                || (c >= 0xFFF0 && c <= 0xFFFB) || c == 0x110BD || c == 0x110CD || (c >= 0x13430 && c <= 0x1343F)
                || (c >= 0x1BCA0 && c <= 0x1BCA3) || (c >= 0x1D173 && c <= 0x1D17A) || (c >= 0xE0000 && c <= 0xE0FFF);
    }

    /** True when the certificate's DER hashes to the link's fingerprint. */
    public boolean matches(X509Certificate certificate) {
        return certificate != null && matches(certSha256, certificate);
    }

    /** The fingerprint the host certificate must have, for {@link PairingManager}. */
    public byte[] certSha256() {
        return certSha256.clone();
    }

    static boolean matches(byte[] sha256, X509Certificate certificate) {
        try {
            return MessageDigest.isEqual(sha256,
                    MessageDigest.getInstance("SHA-256").digest(certificate.getEncoded()));
        } catch (CertificateEncodingException | NoSuchAlgorithmException e) {
            return false;
        }
    }

    private static String required(Map<String, String> values, String key) {
        String value = values.get(key);
        return value == null || value.isEmpty() ? null : value;
    }

    private static String orEmpty(String value) {
        return value == null ? "" : value;
    }

    private static String decode(String text) {
        ByteArrayOutputStream out = new ByteArrayOutputStream(text.length());
        int at = 0;
        while (at < text.length()) {
            if (text.charAt(at) == '%') {
                int high = at + 1 < text.length() ? hexDigit(text.charAt(at + 1)) : -1;
                int low = at + 2 < text.length() ? hexDigit(text.charAt(at + 2)) : -1;
                if (high < 0 || low < 0) {
                    return null;
                }
                out.write((high << 4) | low);
                at += 3;
            } else {
                // Anything else is taken as its UTF-8 bytes, as the protocol crate reads it.
                int end = at + Character.charCount(text.codePointAt(at));
                byte[] bytes = text.substring(at, end).getBytes(StandardCharsets.UTF_8);
                out.write(bytes, 0, bytes.length);
                at = end;
            }
        }
        try {
            return StandardCharsets.UTF_8.newDecoder()
                    .onMalformedInput(CodingErrorAction.REPORT)
                    .onUnmappableCharacter(CodingErrorAction.REPORT)
                    .decode(ByteBuffer.wrap(out.toByteArray())).toString();
        } catch (CharacterCodingException e) {
            return null;
        }
    }

    /** An ASCII hex digit's value, else -1 ({@link Character#digit} also takes other scripts' digits). */
    private static int hexDigit(char c) {
        if (c >= '0' && c <= '9') return c - '0';
        if (c >= 'a' && c <= 'f') return c - 'a' + 10;
        if (c >= 'A' && c <= 'F') return c - 'A' + 10;
        return -1;
    }

    private static byte[] fingerprint(String text) {
        if (text.length() != 64) {
            return null;
        }
        byte[] out = new byte[32];
        for (int i = 0; i < 32; i++) {
            int high = hexDigit(text.charAt(2 * i));
            int low = hexDigit(text.charAt(2 * i + 1));
            if (high < 0 || low < 0) {
                return null;
            }
            out[i] = (byte) ((high << 4) | low);
        }
        return out;
    }
}
