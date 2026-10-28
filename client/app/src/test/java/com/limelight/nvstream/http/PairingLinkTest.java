package com.limelight.nvstream.http;

import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;
import org.junit.Test;

import java.math.BigInteger;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.MessageDigest;
import java.security.cert.X509Certificate;
import java.util.Date;
import java.util.HexFormat;

import static org.junit.Assert.*;

public class PairingLinkTest {
    // What the host's pairing_uri writes for the protocol crate's test invitation.
    private static final String FP;
    private static final String GOOD;
    static {
        StringBuilder fp = new StringBuilder();
        for (int index = 0; index < 32; index++) fp.append(String.format("%02x", index * 8));
        FP = fp.toString();
        GOOD = "rubylight://pair?v=1&host=192.168.4.2&port=47989&otp=4821&pass=a%26b%3Dc%20d&id=0123ABCD&fp=" + FP
                + "&name=Wohnzimmer-PC%20%C3%A4";
    }

    @Test
    public void readsTheLinkTheHostWrites() {
        PairingLink link = PairingLink.parse("  " + GOOD + "\n");
        assertNotNull(link);
        assertEquals("192.168.4.2", link.host);
        assertEquals(47989, link.port);
        assertEquals("4821", link.otp);
        assertEquals("a&b=c d", link.passphrase);
        assertEquals("0123ABCD", link.uniqueId);
        assertEquals("Wohnzimmer-PC ä", link.name);
        assertArrayEquals(HexFormat.of().parseHex(FP), link.certSha256());
        assertTrue(PairingLink.looksLikeLink(GOOD));

        PairingLink v6 = PairingLink.parse("rubylight://pair?v=1&host=%5Bfe80%3A%3A1%2525eth0%5D&port=00080&otp=1"
                + "&fp=" + FP.toUpperCase() + "&name=%CF%80%20%2F%20100%25");
        assertNotNull(v6);
        assertEquals("[fe80::1%25eth0]", v6.host);
        assertEquals(80, v6.port);
        assertEquals("π / 100%", v6.name);
    }

    @Test
    public void optionalKeysMayBeMissingAndUnknownOnesAreIgnored() {
        PairingLink link = PairingLink.parse("rubylight://pair?v=1&host=pc&port=1&otp=1234&fp=" + "AB".repeat(32)
                + "&extra=x");
        assertNotNull(link);
        assertEquals("", link.passphrase);
        assertEquals("", link.uniqueId);
        assertEquals("", link.name);
    }

    @Test
    public void badLinksAreRefused() {
        assertNotNull(PairingLink.parse(GOOD));
        String[][] edits = {
                {"v=1", "v=2"},
                {"v=1&", ""},
                {"otp=4821&", ""},
                {"otp=4821", "otp="},
                {"port=47989", "port=0"},
                {"port=47989", "port=65536"},
                {"port=47989", "port=99999999999999999999"},
                {"port=47989", "port=+80"},
                {"port=47989", "port=4٩"},
                {"port=47989&", ""},
                {"host=192.168.4.2&", ""},
                {"host=192.168.4.2", "host="},
                {"rubylight://", "https://"},
                {"pair?", "Pair?"},
                {"pass=a%26b", "pass=a%2"},
                {"pass=a%26b", "pass=a%G6b"},
                {"pass=a%26b", "pass=a%١٢b"},
                {"pass=a%26b", "pass=a%FFb"},
                {"&name=", "&otp=1&name="},
                {"&id=", "&id&id2="},
                {"fp=" + FP, "fp="},
                {"fp=" + FP, "fp=" + "00".repeat(31)},
                {"fp=" + FP, "fp=" + "00".repeat(33)},
                {"fp=" + FP, "fp=" + "00".repeat(31) + "zz"},
                {"fp=" + FP, "fp=" + "00".repeat(31) + "١٢"},
                {"&fp=" + FP, ""},
        };
        for (String[] edit : edits) {
            String bad = GOOD.replace(edit[0], edit[1]);
            assertNotEquals(edit[0], GOOD, bad);
            assertNull(bad, PairingLink.parse(bad));
        }
        assertNull(PairingLink.parse("rubylight://pair?v=1&host"));
        assertNull(PairingLink.parse(null));
        assertNull(PairingLink.parse("192.168.4.2"));
        assertFalse(PairingLink.looksLikeLink("192.168.4.2:47989"));
        // Meant as a link even when it is not a valid one, so Add PC does not read it as an address.
        assertTrue(PairingLink.looksLikeLink("rubylight://pair?v=9"));
    }

    @Test
    public void onlyTheCertificateTheLinkNamesMatches() throws Exception {
        KeyPairGenerator generator = KeyPairGenerator.getInstance("RSA");
        generator.initialize(2048);
        KeyPair keys = generator.generateKeyPair();
        X509Certificate host = certificate(keys, 1);
        X509Certificate forged = certificate(keys, 2);
        String fp = HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(host.getEncoded()));
        PairingLink link = PairingLink.parse("rubylight://pair?v=1&host=pc&port=47989&otp=1234&fp=" + fp);
        assertNotNull(link);
        assertTrue(link.matches(host));
        assertFalse(link.matches(forged));
        assertFalse(link.matches(null));
        // The fingerprint handed out is a copy.
        link.certSha256()[0] ^= 1;
        assertTrue(link.matches(host));
    }

    static X509Certificate certificate(KeyPair keys, long serial) throws Exception {
        X500Name name = new X500Name("CN=Pairing link test");
        return new JcaX509CertificateConverter().getCertificate(new JcaX509v3CertificateBuilder(
                name, BigInteger.valueOf(serial), new Date(0), new Date(4102444800000L), name, keys.getPublic())
                .build(new JcaContentSignerBuilder("SHA256withRSA").build(keys.getPrivate())));
    }
}
