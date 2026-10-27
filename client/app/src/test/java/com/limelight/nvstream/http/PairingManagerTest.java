package com.limelight.nvstream.http;

import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;
import org.junit.BeforeClass;
import org.junit.Test;

import java.io.IOException;
import java.math.BigInteger;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.PrivateKey;
import java.security.Signature;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;
import java.util.ArrayDeque;
import java.util.Arrays;
import java.util.Date;
import java.util.HexFormat;

import javax.net.ssl.X509TrustManager;

import static org.junit.Assert.*;

public class PairingManagerTest {
    private static final String SERVER_INFO = "<root status_code=\"200\"><appversion>7.1.2.3</appversion></root>";
    private static KeyPair keys;
    private static X509Certificate certificate;
    private static X509Certificate otherCertificate;
    private static LimelightCryptoProvider crypto;

    @BeforeClass
    public static void createIdentity() throws Exception {
        KeyPairGenerator generator = KeyPairGenerator.getInstance("RSA");
        generator.initialize(2048);
        keys = generator.generateKeyPair();
        X500Name name = new X500Name("CN=Pairing test");
        certificate = new JcaX509CertificateConverter().getCertificate(new JcaX509v3CertificateBuilder(
                name, BigInteger.ONE, new Date(0), new Date(4102444800000L), name, keys.getPublic())
                .build(new JcaContentSignerBuilder("SHA256withRSA").build(keys.getPrivate())));
        otherCertificate = new JcaX509CertificateConverter().getCertificate(new JcaX509v3CertificateBuilder(
                name, BigInteger.TWO, new Date(0), new Date(4102444800000L), name, keys.getPublic())
                .build(new JcaContentSignerBuilder("SHA256withRSA").build(keys.getPrivate())));
        crypto = new LimelightCryptoProvider() {
            public X509Certificate getClientCertificate() { return certificate; }
            public PrivateKey getClientPrivateKey() { return keys.getPrivate(); }
            public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            public String encodeBase64String(byte[] data) { return ""; }
        };
    }

    private static String reply(String tag, String value) {
        return "<root status_code=\"200\"><paired>1</paired><" + tag + ">" + value + "</" + tag + "></root>";
    }

    private static class PairingHttp extends NvHTTP {
        final ArrayDeque<String> replies = new ArrayDeque<>();
        int cancellations;
        boolean failCancellation;

        PairingHttp(String... replies) throws IOException {
            super(new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "client", null, crypto);
            this.replies.addAll(Arrays.asList(replies));
        }

        @Override String executePairingCommand(String arguments, boolean timeout) {
            return replies.removeFirst();
        }

        @Override void cancelPairing() throws IOException {
            cancellations++;
            if (failCancellation) throw new IOException("Host disconnected during cleanup");
        }
    }

    @Test
    public void malformedCertificatesFailWithCheckedErrorsAndReleasePairing() throws Exception {
        for (String encoded : new String[] {"0", "GG", "00"}) {
            PairingHttp http = new PairingHttp(reply("plaincert", encoded));
            assertThrows(IOException.class, () -> http.getPairingManager().pair(SERVER_INFO, "1234"));
            assertEquals(1, http.cancellations);
        }
    }

    @Test
    public void truncatedEncryptedChallengesFailBeforeTheNextProtocolStep() throws Exception {
        for (String encoded : new String[] {"AA", "00".repeat(16), "00".repeat(47)}) {
            PairingHttp http = new PairingHttp(reply("plaincert", HexFormat.of().formatHex(certificate.getEncoded())),
                    reply("challengeresponse", encoded));
            assertThrows(IOException.class, () -> http.getPairingManager().pair(SERVER_INFO, "1234"));
            assertEquals(1, http.cancellations);
        }
    }

    @Test
    public void truncatedSecretsAreCheckedErrors() throws Exception {
        PairingHttp http = new PairingHttp(reply("plaincert", HexFormat.of().formatHex(certificate.getEncoded())),
                reply("challengeresponse", "00".repeat(48)), reply("pairingsecret", "AA"));
        assertThrows(IOException.class, () -> http.getPairingManager().pair(SERVER_INFO, "1234"));
        assertEquals(1, http.cancellations);
    }

    @Test
    public void invalidSignatureFailsPairingWithoutCrashing() throws Exception {
        PairingHttp http = new PairingHttp(reply("plaincert", HexFormat.of().formatHex(certificate.getEncoded())),
                reply("challengeresponse", "00".repeat(48)), reply("pairingsecret", "00".repeat(17)));
        assertEquals(PairingManager.PairState.FAILED, http.getPairingManager().pair(SERVER_INFO, "1234"));
        assertEquals(1, http.cancellations);
    }

    @Test
    public void cleanupFailureDoesNotHideWrongPin() throws Exception {
        byte[] secret = new byte[16];
        Signature signature = Signature.getInstance("SHA256withRSA");
        signature.initSign(keys.getPrivate());
        signature.update(secret);
        PairingHttp http = new PairingHttp(reply("plaincert", HexFormat.of().formatHex(certificate.getEncoded())),
                reply("challengeresponse", "00".repeat(48)),
                reply("pairingsecret", HexFormat.of().formatHex(secret) + HexFormat.of().formatHex(signature.sign())));
        http.failCancellation = true;
        assertEquals(PairingManager.PairState.PIN_WRONG, http.getPairingManager().pair(SERVER_INFO, "1234"));
        assertEquals(1, http.cancellations);
    }

    private static X509TrustManager trustManager(NvHTTP http, boolean caTrusted) throws Exception {
        java.lang.reflect.Field defaults = NvHTTP.class.getDeclaredField("defaultTrustManager");
        defaults.setAccessible(true);
        defaults.set(http, new X509TrustManager() {
            public X509Certificate[] getAcceptedIssuers() { return new X509Certificate[0]; }
            public void checkClientTrusted(X509Certificate[] chain, String authType) { }
            public void checkServerTrusted(X509Certificate[] chain, String authType) throws CertificateException {
                if (!caTrusted) throw new CertificateException("Untrusted CA");
            }
        });
        java.lang.reflect.Field field = NvHTTP.class.getDeclaredField("trustManager");
        field.setAccessible(true);
        return (X509TrustManager) field.get(http);
    }

    @Test
    public void pairedCertificateCannotBeReplacedByACaTrustedCertificate() throws Exception {
        PairingHttp http = new PairingHttp();
        http.setServerCert(certificate);
        X509TrustManager trust = trustManager(http, true);
        assertThrows(CertificateException.class,
                () -> trust.checkServerTrusted(new X509Certificate[] {otherCertificate}, "RSA"));
    }

    @Test
    public void pairedLeafRemainsTrustedWhenServerIncludesItsChain() throws Exception {
        PairingHttp http = new PairingHttp();
        http.setServerCert(certificate);
        trustManager(http, false).checkServerTrusted(new X509Certificate[] {certificate, otherCertificate}, "RSA");
    }

    @Test
    public void unpairedHostsStillRequireCaTrust() throws Exception {
        PairingHttp http = new PairingHttp();
        trustManager(http, true).checkServerTrusted(new X509Certificate[] {certificate}, "RSA");
        X509TrustManager trust = trustManager(http, false);
        assertThrows(CertificateException.class,
                () -> trust.checkServerTrusted(new X509Certificate[] {certificate}, "RSA"));
    }
}
