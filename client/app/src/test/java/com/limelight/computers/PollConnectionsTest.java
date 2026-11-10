package com.limelight.computers;

import android.app.Application;

import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvHTTP;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ServiceController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import javax.net.ssl.X509TrustManager;

import static org.junit.Assert.*;

/**
 * A poll gets the client the last poll of the same address and certificate used, and only then:
 * never one another poll still uses, one built for another certificate, or one from before the
 * polls stopped.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class PollConnectionsTest {
    private static final ComputerDetails.AddressTuple DEN = new ComputerDetails.AddressTuple("192.0.2.1", 47989);

    /** Builds real clients, as the service does, and lists what it was asked for. */
    private static final class Built implements PollConnections.Opener {
        final List<String> asked = new ArrayList<>();
        boolean fails;

        @Override
        public NvHTTP open(ComputerDetails.AddressTuple address, int httpsPort, X509Certificate serverCert) throws IOException {
            asked.add(address.address + ":" + address.port + " https " + httpsPort + (serverCert != null ? " pinned" : ""));
            if (fails) {
                throw new IOException("No client");
            }
            return new NvHTTP(address, httpsPort, "device", serverCert, new LimelightCryptoProvider() {
                public X509Certificate getClientCertificate() { return null; }
                public PrivateKey getClientPrivateKey() { return null; }
                public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
                public String encodeBase64String(byte[] data) { return ""; }
            });
        }
    }

    private static int httpsPort(NvHTTP http) {
        return ReflectionHelpers.getField(http, "httpsPort");
    }

    /** A certificate that is not the fixture's: the first one this JVM trusts. */
    private static X509Certificate otherCertificate() throws Exception {
        TrustManagerFactory factory = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
        factory.init((KeyStore) null);
        for (TrustManager manager : factory.getTrustManagers()) {
            if (manager instanceof X509TrustManager) {
                return ((X509TrustManager) manager).getAcceptedIssuers()[0];
            }
        }
        throw new AssertionError("No trusted certificate");
    }

    @Test
    public void aPollGetsTheClientOfTheLastPollOfThatAddress() throws Exception {
        Built built = new Built();
        PollConnections connections = new PollConnections(built);

        PollConnections.Lease first = connections.take(DEN, 47984, null);
        assertEquals("Out on its poll", 0, connections.kept());
        connections.giveBack(first, 0);
        assertEquals(1, connections.kept());

        // The address of the next poll is another object with the same text and port.
        PollConnections.Lease second = connections.take(new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, null);
        assertSame(first.http, second.http);
        connections.giveBack(second, 1500);
        assertSame(first.http, connections.take(DEN, 47984, null).http);
        assertEquals(List.of("192.0.2.1:47989 https 47984"), built.asked);
    }

    // A poll gives a new client the HTTPS port it knows, or 0 for the client to ask the host. A kept
    // client learned a port on its last poll and must not carry it into a poll that would ask again.
    @Test
    public void aKeptClientStartsAtTheHttpsPortANewOneWouldBeGiven() throws Exception {
        PollConnections connections = new PollConnections(new Built());
        PollConnections.Lease lease = connections.take(DEN, 0, null);
        assertEquals(0, httpsPort(lease.http));
        // What the client does when the host answers its question.
        lease.http.setHttpsPort(47984);
        connections.giveBack(lease, 0);

        lease = connections.take(DEN, 0, null);
        assertEquals("The port of the last poll stayed", 0, httpsPort(lease.http));
        connections.giveBack(lease, 0);

        lease = connections.take(DEN, 48984, null);
        assertEquals(48984, httpsPort(lease.http));
    }

    @Test
    public void pollsThatOverlapEachHaveTheirOwnClient() throws Exception {
        Built built = new Built();
        PollConnections connections = new PollConnections(built);
        PollConnections.Lease slow = connections.take(DEN, 47984, null);
        PollConnections.Lease next = connections.take(DEN, 47984, null);
        assertNotSame(slow.http, next.http);
        assertEquals(2, built.asked.size());

        // Both come back; one is enough for the next poll.
        connections.giveBack(next, 0);
        connections.giveBack(slow, 10);
        assertEquals(1, connections.kept());
        assertSame(slow.http, connections.take(DEN, 47984, null).http);
        assertEquals(2, built.asked.size());
    }

    // The client checks the host against the certificate it was built with, so a PC that was paired,
    // paired again or forgotten since the last poll is asked with another client.
    @Test
    public void anotherCertificateAddressOrPortIsAnotherClient() throws Exception {
        Built built = new Built();
        PollConnections connections = new PollConnections(built);
        X509Certificate paired = NativeApiFixtures.certificate();
        X509Certificate pairedAgain = otherCertificate();
        assertNotEquals(paired, pairedAgain);

        PollConnections.Lease unpaired = connections.take(DEN, 47984, null);
        connections.giveBack(unpaired, 0);
        PollConnections.Lease withCertificate = connections.take(DEN, 47984, paired);
        assertNotSame(unpaired.http, withCertificate.http);
        assertSame(paired, ReflectionHelpers.getField(withCertificate.http, "serverCert"));
        connections.giveBack(withCertificate, 0);

        PollConnections.Lease withNewCertificate = connections.take(DEN, 47984, pairedAgain);
        assertNotSame(withCertificate.http, withNewCertificate.http);
        assertSame(pairedAgain, ReflectionHelpers.getField(withNewCertificate.http, "serverCert"));
        connections.giveBack(withNewCertificate, 0);

        // The same certificate read from storage again is the same certificate.
        X509Certificate reread = (X509Certificate) CertificateFactory.getInstance("X.509")
                .generateCertificate(new ByteArrayInputStream(paired.getEncoded()));
        assertNotSame(paired, reread);
        assertSame(withCertificate.http, connections.take(DEN, 47984, reread).http);

        int before = built.asked.size();
        NvHTTP otherPort = connections.take(new ComputerDetails.AddressTuple("192.0.2.1", 48989), 47984, pairedAgain).http;
        NvHTTP otherAddress = connections.take(new ComputerDetails.AddressTuple("192.0.2.2", 47989), 47984, pairedAgain).http;
        assertNotSame(withNewCertificate.http, otherPort);
        assertNotSame(withNewCertificate.http, otherAddress);
        assertEquals(before + 2, built.asked.size());
    }

    @Test
    public void aClientNoPollUsedForAWhileIsDropped() throws Exception {
        Built built = new Built();
        PollConnections connections = new PollConnections(built);
        ComputerDetails.AddressTuple study = new ComputerDetails.AddressTuple("192.0.2.2", 47989);
        PollConnections.Lease den = connections.take(DEN, 47984, null);
        connections.giveBack(den, 1000);
        connections.giveBack(connections.take(study, 47984, null), 1000 + PollConnections.IDLE_MS - 1);
        assertEquals(2, connections.kept());

        // The study is polled on; the den's address is no longer asked (the PC moved, or was removed).
        connections.giveBack(connections.take(study, 47984, null), 1000 + PollConnections.IDLE_MS);
        assertEquals(1, connections.kept());
        assertNotSame(den.http, connections.take(DEN, 47984, null).http);
        assertEquals(3, built.asked.size());
    }

    @Test
    public void whenThePollsStopNoClientIsKeptNotEvenOneStillOut() throws Exception {
        Built built = new Built();
        PollConnections connections = new PollConnections(built);
        PollConnections.Lease out = connections.take(DEN, 47984, null);
        connections.giveBack(connections.take(new ComputerDetails.AddressTuple("192.0.2.2", 47989), 47984, null), 0);
        assertEquals(1, connections.kept());

        connections.clear();
        assertEquals(0, connections.kept());
        // Its request ended after the polls stopped.
        connections.giveBack(out, 10);
        assertEquals(0, connections.kept());

        // Polls that start again keep theirs.
        PollConnections.Lease again = connections.take(DEN, 47984, null);
        assertNotSame(out.http, again.http);
        connections.giveBack(again, 20);
        assertEquals(1, connections.kept());
        assertSame(again.http, connections.take(DEN, 47984, null).http);
    }

    @Test
    public void aClientThatCannotBeBuiltFailsThePollAsBefore() {
        Built built = new Built();
        built.fails = true;
        PollConnections connections = new PollConnections(built);
        assertThrows(IOException.class, () -> connections.take(DEN, 47984, null));
        assertEquals(0, connections.kept());
    }

    private static ComputerDetails poll(ComputerManagerService service, ComputerDetails pc,
                                        ComputerDetails.AddressTuple address) {
        return ReflectionHelpers.callInstanceMethod(service, "tryPollIp",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, pc),
                ReflectionHelpers.ClassParameter.from(ComputerDetails.AddressTuple.class, address));
    }

    // The service's own polls, against a port that takes the connection and hangs up: each poll asks
    // over the network with the one client the service keeps for that address, also after a poll
    // that failed, and the service keeps none once nobody is bound to it.
    @Test
    public void theServiceAsksAnAddressWithTheClientItKeptAndKeepsNoneOnceUnbound() throws Exception {
        try (ServerSocket host = new ServerSocket(0, 50, InetAddress.getLoopbackAddress())) {
            AtomicInteger connections = new AtomicInteger();
            Thread hangsUp = new Thread(() -> {
                while (true) {
                    try (Socket ignored = host.accept()) {
                        connections.incrementAndGet();
                    } catch (IOException e) {
                        return;
                    }
                }
            }, "Fixture host");
            hangsUp.setDaemon(true);
            hangsUp.start();

            ServiceController<ComputerManagerLifecycleTest.Manager> controller =
                    Robolectric.buildService(ComputerManagerLifecycleTest.Manager.class).create();
            ComputerManagerService service = controller.get();
            try {
                PollConnections kept = ReflectionHelpers.getField(service, "pollConnections");
                ComputerDetails.AddressTuple address = new ComputerDetails.AddressTuple("127.0.0.1", host.getLocalPort());
                ComputerDetails pc = new ComputerDetails();
                pc.uuid = "den";

                assertNull(poll(service, pc, address));
                assertEquals(1, kept.kept());
                int afterFirst = connections.get();
                assertTrue(afterFirst > 0);
                PollConnections.Lease first = kept.take(address, 0, null);
                kept.giveBack(first, 0);

                assertNull(poll(service, pc, address));
                assertTrue("The kept client asked the host again", connections.get() > afterFirst);
                assertEquals(1, kept.kept());
                PollConnections.Lease second = kept.take(address, 0, null);
                assertSame(first.http, second.http);
                kept.giveBack(second, 0);

                controller.unbind();
                assertEquals(0, kept.kept());
            } finally {
                controller.destroy();
            }
        }
    }
}
