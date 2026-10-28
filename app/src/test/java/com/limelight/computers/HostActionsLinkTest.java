package com.limelight.computers;

import com.limelight.R;
import com.limelight.nvstream.http.PairingLink;
import com.limelight.nvstream.http.PairingManager.PairState;

import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;
import org.junit.Test;

import java.math.BigInteger;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.cert.X509Certificate;
import java.util.Date;

import static org.junit.Assert.*;

public class HostActionsLinkTest {
    @Test
    public void aLinkPairsAgainUntilACertificateIsPinned() throws Exception {
        KeyPairGenerator generator = KeyPairGenerator.getInstance("RSA");
        generator.initialize(2048);
        KeyPair keys = generator.generateKeyPair();
        X500Name name = new X500Name("CN=Host actions link test");
        X509Certificate pinned = new JcaX509CertificateConverter().getCertificate(new JcaX509v3CertificateBuilder(
                name, BigInteger.ONE, new Date(0), new Date(4102444800000L), name, keys.getPublic())
                .build(new JcaContentSignerBuilder("SHA256withRSA").build(keys.getPrivate())));
        PairingLink link = PairingLink.parse("rubylight://pair?v=1&host=pc&port=47989&otp=1234&fp=" + "00".repeat(32));
        assertNotNull(link);

        // A PIN pairing keeps trusting the host's answer, as before.
        assertTrue(HostActions.alreadyPaired(PairState.PAIRED, null, null));
        // A link with nothing pinned pairs, so the link's fingerprint is checked before anything is trusted.
        assertFalse(HostActions.alreadyPaired(PairState.PAIRED, link, null));
        // With a certificate pinned, the pinned one is compared with the link instead.
        assertTrue(HostActions.alreadyPaired(PairState.PAIRED, link, pinned));
        assertFalse(link.matches(pinned));
        assertFalse(HostActions.alreadyPaired(PairState.NOT_PAIRED, link, pinned));
        assertFalse(HostActions.alreadyPaired(PairState.NOT_PAIRED, null, null));
    }

    @Test
    public void aRefusedPairingSaysWhatToDoInsteadOfBlamingTheNetwork() {
        // A replayed or expired QR code is refused by the host with an HTTP error; it used to show the
        // network and firewall advice.
        assertEquals(R.string.pair_qr_incorrect_pin, HostActions.refusalMessage(true, false));
        assertEquals(R.string.pair_otp_incorrect_pin, HostActions.refusalMessage(false, true));
        assertEquals(R.string.pair_fail, HostActions.refusalMessage(false, false));
        assertNotEquals(R.string.pair_connection_fix, HostActions.refusalMessage(true, false));
        // Only a refusal: the host's "OTP pairing is not available" is status 400; a 503 or a 5xx is no answer.
        assertTrue(HostActions.refused(400));
        assertTrue(HostActions.refused(403));
        assertFalse(HostActions.refused(503));
        assertFalse(HostActions.refused(599));
    }
}
