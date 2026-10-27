package com.limelight.binding.crypto;

import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.openssl.jcajce.JcaPEMWriter;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.StringWriter;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.Signature;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.Date;

import static org.junit.Assert.*;

public class AndroidCryptoProviderTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private static byte[] pem(X509Certificate certificate) throws Exception {
        StringWriter text = new StringWriter();
        try (JcaPEMWriter writer = new JcaPEMWriter(text)) { writer.writeObject(certificate); }
        return text.toString().getBytes(StandardCharsets.US_ASCII);
    }

    private static void assertIdentityMatches(AndroidCryptoProvider provider) throws Exception {
        Signature signature = Signature.getInstance("SHA256withRSA");
        byte[] challenge = "pairing challenge".getBytes(StandardCharsets.US_ASCII);
        signature.initSign(provider.getClientPrivateKey());
        signature.update(challenge);
        byte[] signed = signature.sign();
        signature.initVerify(provider.getClientCertificate());
        signature.update(challenge);
        assertTrue(signature.verify(signed));
        assertNotNull(provider.getPemEncodedClientCertificate());
        assertEquals(provider.getClientCertificate(), CertificateFactory.getInstance("X.509")
                .generateCertificate(new ByteArrayInputStream(provider.getPemEncodedClientCertificate())));
    }

    @Test
    public void interruptedIdentityWriteDoesNotLoadMismatchedKeys() throws Exception {
        File directory = temporary.newFolder();
        KeyPairGenerator generator = KeyPairGenerator.getInstance("RSA");
        generator.initialize(2048);
        KeyPair first = generator.generateKeyPair();
        KeyPair second = generator.generateKeyPair();
        X500Name name = new X500Name("CN=Interrupted write");
        X509Certificate certificate = new JcaX509CertificateConverter().getCertificate(
                new JcaX509v3CertificateBuilder(name, BigInteger.ONE, new Date(0),
                        new Date(4102444800000L), name, first.getPublic())
                        .build(new JcaContentSignerBuilder("SHA256withRSA").build(first.getPrivate())));
        Files.write(new File(directory, "client.crt").toPath(), pem(certificate));
        Files.write(new File(directory, "client.key").toPath(), second.getPrivate().getEncoded());

        AndroidCryptoProvider provider = new AndroidCryptoProvider(directory);
        provider.getClientCertificate();
        assertIdentityMatches(provider);
        assertIdentityMatches(new AndroidCryptoProvider(directory));
    }

    @Test
    public void failedPersistenceStillLeavesACompleteInMemoryIdentity() throws Exception {
        File directory = temporary.newFolder();
        assertTrue(new File(directory, "client.key").mkdir());
        AndroidCryptoProvider provider = new AndroidCryptoProvider(directory);
        provider.getClientCertificate();
        assertIdentityMatches(provider);
    }

    @Test
    public void existingIdentityIsReused() throws Exception {
        File directory = temporary.newFolder();
        AndroidCryptoProvider provider = new AndroidCryptoProvider(directory);
        X509Certificate certificate = provider.getClientCertificate();
        assertIdentityMatches(provider);
        AndroidCryptoProvider reloaded = new AndroidCryptoProvider(directory);
        assertEquals(certificate, reloaded.getClientCertificate());
        assertIdentityMatches(reloaded);
    }
}
