package com.limelight.nvstream;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import android.app.Application;

import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeApiTransport;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;
import java.lang.reflect.Field;
import java.math.BigInteger;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.Date;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;

/** The stream's end-of-session actions: native API hosts get the native calls, the others keep Quit app. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NvConnectionNativeActionsTest {
    private static final HostCapabilities NATIVE = TestHosts.caps("<root status_code=\"200\">"
            + "<RustHostVersion>3.0.0</RustHostVersion><PairStatus>1</PairStatus>"
            + "<NativeApiVersion>1</NativeApiVersion></root>");
    private static final String SESSION = "{\"ok\":{\"revision\":3,\"state\":\"idle\",\"owner\":\"none\","
            + "\"app\":null,\"game_running\":false,\"actions\":[]}}";

    private static final class Identity implements LimelightCryptoProvider {
        final X509Certificate certificate;
        final PrivateKey key;

        Identity() throws Exception {
            KeyPairGenerator generator = KeyPairGenerator.getInstance("RSA");
            generator.initialize(2048);
            KeyPair keys = generator.generateKeyPair();
            X500Name name = new X500Name("CN=Test");
            certificate = new JcaX509CertificateConverter().getCertificate(
                    new JcaX509v3CertificateBuilder(name, BigInteger.ONE, new Date(0), new Date(4102444800000L),
                            name, keys.getPublic()).build(new JcaContentSignerBuilder("SHA256withRSA").build(keys.getPrivate())));
            key = keys.getPrivate();
        }

        @Override public X509Certificate getClientCertificate() { return certificate; }
        @Override public PrivateKey getClientPrivateKey() { return key; }
        @Override public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
        @Override public String encodeBase64String(byte[] data) { return ""; }
    }

    private static final class Recording implements NativeApiTransport {
        final List<String> arguments = new CopyOnWriteArrayList<>();
        final CountDownLatch called = new CountDownLatch(1);

        @Override public String request(HostEndpoint host, String op, String argument) throws NativeApiException {
            if (!op.equals("session_action")) {
                throw new NativeApiException(NativeApiException.Kind.INVALID_ARGUMENT, op);
            }
            arguments.add(argument);
            called.countDown();
            return SESSION;
        }
    }

    private static final class Http extends NvHTTP {
        final HostCapabilities caps;
        int quits;

        Http(HostCapabilities caps, LimelightCryptoProvider crypto) throws IOException {
            super(new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test", null, crypto);
            this.caps = caps;
        }

        @Override public ComputerDetails getComputerDetails(boolean fetchAppList) {
            ComputerDetails details = new ComputerDetails();
            details.uuid = "host";
            details.caps = caps;
            details.pairState = PairingManager.PairState.PAIRED;
            return details;
        }

        @Override public boolean quitApp() {
            quits++;
            return true;
        }
    }

    private static NvConnection connection(HostCapabilities caps, Http[] http, Recording transport) throws Exception {
        Identity identity = new Identity();
        NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989),
                47984, "test", new StreamConfiguration.Builder().build(), identity, identity.certificate);
        connection.getHostDetails().caps = caps;
        connection.setNativeApiTransport(transport);
        http[0] = new Http(caps, identity);
        Field field = NvConnection.class.getDeclaredField("http");
        field.setAccessible(true);
        field.set(connection, http[0]);
        return connection;
    }

    @Test
    public void aNativeHostClosesTheGameThroughTheNativeApiAndNeverQuitsTheApp() throws Exception {
        Http[] http = new Http[1];
        Recording transport = new Recording();
        NvConnection connection = connection(NATIVE, http, transport);
        assertTrue(connection.hasNativeSessionActions());
        connection.closeGame();
        assertEquals(List.of("close_game"), transport.arguments);
        assertEquals(0, http[0].quits);
        connection.nativeSessionAction(com.limelight.nativeapi.NativeSession.ActionId.END);
        assertEquals(List.of("close_game", "end"), transport.arguments);
    }

    @Test
    public void anotherHostKeepsQuitAppAndTheNativeActionsRefuse() throws Exception {
        Http[] http = new Http[1];
        Recording transport = new Recording();
        NvConnection connection = connection(TestHosts.fixture("sunshine-serverinfo.xml"), http, transport);
        assertFalse(connection.hasNativeSessionActions());
        connection.closeGame();
        assertEquals(1, http[0].quits);
        try {
            connection.nativeSessionAction(com.limelight.nativeapi.NativeSession.ActionId.END);
            fail("A host without the native API has no End session");
        } catch (IOException expected) {
            // The menu never offers it either.
        }
        connection.announceDisconnect();
        assertTrue(transport.arguments.isEmpty());
    }

    @Test
    public void leavingAStreamAnnouncesTheDisconnectOnlyToANativeHost() throws Exception {
        Http[] http = new Http[1];
        Recording transport = new Recording();
        NvConnection connection = connection(NATIVE, http, transport);
        connection.announceDisconnect();
        assertTrue(transport.called.await(5, TimeUnit.SECONDS));
        assertEquals(List.of("disconnect"), transport.arguments);
        assertEquals(0, http[0].quits);
    }
}
