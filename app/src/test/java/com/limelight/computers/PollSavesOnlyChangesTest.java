package com.limelight.computers;

import android.app.Application;
import android.content.Context;
import android.database.Cursor;
import android.database.sqlite.SQLiteDatabase;

import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;

import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ServiceController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.security.KeyStore;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.function.Consumer;

import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import javax.net.ssl.X509TrustManager;

import static org.junit.Assert.*;

/**
 * A poll that finds a PC as it is saved writes nothing to the database, and one that finds
 * anything the database keeps of a PC changed writes it: the name, each address, the MAC address
 * and the certificate, also when the certificate goes.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class PollSavesOnlyChangesTest {
    private static final String UUID = "polled-host";
    private static final ComputerDetails.AddressTuple REMOTE = new ComputerDetails.AddressTuple("203.0.113.9", 47989);
    private static final ComputerDetails.AddressTuple LOCAL = new ComputerDetails.AddressTuple("192.168.1.9", 47989);

    /** A PC that answers every poll; what it says of itself is the test's to change. */
    public static class AnsweringManager extends ComputerManagerLifecycleTest.Manager {
        Consumer<ComputerDetails> answer = polled -> {};

        @Override boolean pollComputer(ComputerDetails details) {
            // As the service's own: what the PC answered is merged into the PC being polled.
            ComputerDetails polled = new ComputerDetails();
            polled.uuid = details.uuid;
            polled.name = "PC";
            polled.state = ComputerDetails.State.ONLINE;
            polled.activeAddress = LOCAL;
            polled.localAddress = LOCAL;
            polled.macAddress = "AA:BB:CC:DD:EE:01";
            polled.pairState = PairingManager.PairState.PAIRED;
            answer.accept(polled);
            details.update(polled);
            return true;
        }
    }

    /** The database of the service, counting the rows written and letting a test in after a read. */
    private static final class CountingDatabase extends ComputerDatabaseManager {
        final List<String> written = new ArrayList<>();
        Runnable afterNextRead;

        CountingDatabase(Context context) {
            super(context);
        }

        @Override public boolean updateComputer(ComputerDetails details) {
            // Null only while the constructor moves old databases over, which no test here has.
            if (written != null) {
                written.add(details.uuid);
            }
            return super.updateComputer(details);
        }

        @Override public ComputerDetails getComputerByUUID(String uuid) {
            ComputerDetails read = super.getComputerByUUID(uuid);
            Runnable then = afterNextRead;
            afterNextRead = null;
            if (then != null) {
                then.run();
            }
            return read;
        }
    }

    /** The service with one saved PC, paired or not. */
    private static final class Polls implements AutoCloseable {
        final Context context = RuntimeEnvironment.getApplication();
        final ServiceController<AnsweringManager> controller;
        final AnsweringManager service;
        final CountingDatabase database;
        final PollingTuple tuple;

        Polls(X509Certificate certificate) {
            ComputerDetails stored = new ComputerDetails();
            stored.uuid = UUID;
            stored.name = "PC";
            stored.localAddress = LOCAL;
            stored.remoteAddress = REMOTE;
            stored.macAddress = "AA:BB:CC:DD:EE:01";
            stored.serverCert = certificate;
            ComputerDatabaseManager saved = new ComputerDatabaseManager(context);
            saved.updateComputer(stored);
            saved.close();

            controller = Robolectric.buildService(AnsweringManager.class).create();
            service = controller.get();
            ComputerDatabaseManager own = ReflectionHelpers.getField(service, "dbManager");
            own.close();
            database = new CountingDatabase(service);
            ReflectionHelpers.setField(service, "dbManager", database);
            List<PollingTuple> tuples = ReflectionHelpers.getField(service, "pollingTuples");
            tuple = tuples.get(0);
            assertEquals(UUID, tuple.computer.uuid);
        }

        void poll() {
            ReflectionHelpers.callInstanceMethod(service, "runPoll",
                    ReflectionHelpers.ClassParameter.from(ComputerDetails.class, tuple.computer),
                    ReflectionHelpers.ClassParameter.from(PollingTuple.class, tuple),
                    ReflectionHelpers.ClassParameter.from(int.class, 0));
        }

        ComputerDetails saved() {
            return database.getComputerByUUID(UUID);
        }

        @Override public void close() {
            controller.destroy();
        }
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
    public void aPollThatFindsThePcAsItIsSavedWritesNothing() {
        try (Polls polls = new Polls(NativeApiFixtures.certificate())) {
            Object[] before = ComputerDatabaseManager.columns(polls.saved());
            for (int i = 0; i < 5; i++) {
                polls.poll();
            }
            assertEquals(Collections.emptyList(), polls.database.written);
            assertTrue(Arrays.deepEquals(before, ComputerDatabaseManager.columns(polls.saved())));
            assertEquals(ComputerDetails.State.ONLINE, polls.tuple.computer.state);
        }
    }

    // Each thing the database keeps of a PC, changed one after the other. The poll that finds the
    // change writes the PC once, and the polls after it find the PC as saved again.
    @Test
    public void aPollThatFindsAnythingSavedChangedWritesIt() throws Exception {
        X509Certificate pairedAgain = otherCertificate();
        assertNotEquals(NativeApiFixtures.certificate(), pairedAgain);
        try (Polls polls = new Polls(NativeApiFixtures.certificate())) {
            int writes = 0;

            polls.service.answer = polled -> polled.name = "DEN-PC";
            polls.poll();
            assertEquals("name", ++writes, polls.database.written.size());
            assertEquals("DEN-PC", polls.saved().name);

            polls.service.answer = polls.service.answer.andThen(polled ->
                    polled.localAddress = new ComputerDetails.AddressTuple("192.168.1.10", 47989));
            polls.poll();
            assertEquals("local address", ++writes, polls.database.written.size());
            assertEquals(new ComputerDetails.AddressTuple("192.168.1.10", 47989), polls.saved().localAddress);

            // The host's external port goes into the remote address that was saved.
            polls.service.answer = polls.service.answer.andThen(polled -> polled.externalPort = 48989);
            polls.poll();
            assertEquals("remote port", ++writes, polls.database.written.size());
            assertEquals(new ComputerDetails.AddressTuple("203.0.113.9", 48989), polls.saved().remoteAddress);

            polls.service.answer = polls.service.answer.andThen(polled ->
                    polled.ipv6Address = new ComputerDetails.AddressTuple("2001:db8::9", 47989));
            polls.poll();
            assertEquals("IPv6 address", ++writes, polls.database.written.size());
            assertEquals(new ComputerDetails.AddressTuple("2001:db8::9", 47989), polls.saved().ipv6Address);

            polls.service.answer = polls.service.answer.andThen(polled -> polled.macAddress = "AA:BB:CC:DD:EE:02");
            polls.poll();
            assertEquals("MAC address", ++writes, polls.database.written.size());
            assertEquals("AA:BB:CC:DD:EE:02", polls.saved().macAddress);

            // An address the user typed and a certificate from a pairing are set on the PC that is
            // polled, and the next poll is what saves them.
            polls.tuple.computer.manualAddress = new ComputerDetails.AddressTuple("den.example", 47989);
            polls.poll();
            assertEquals("manual address", ++writes, polls.database.written.size());
            assertEquals(new ComputerDetails.AddressTuple("den.example", 47989), polls.saved().manualAddress);

            polls.tuple.computer.serverCert = pairedAgain;
            polls.poll();
            assertEquals("certificate", ++writes, polls.database.written.size());
            assertEquals(pairedAgain, polls.saved().serverCert);

            // Every change is saved and the PC answers the same from here on.
            for (int i = 0; i < 3; i++) {
                polls.poll();
            }
            assertEquals(writes, polls.database.written.size());
            ComputerDetails saved = polls.saved();
            assertEquals("DEN-PC", saved.name);
            assertEquals(new ComputerDetails.AddressTuple("192.168.1.10", 47989), saved.localAddress);
            assertEquals(new ComputerDetails.AddressTuple("203.0.113.9", 48989), saved.remoteAddress);
            assertEquals(new ComputerDetails.AddressTuple("den.example", 47989), saved.manualAddress);
            assertEquals(new ComputerDetails.AddressTuple("2001:db8::9", 47989), saved.ipv6Address);
            assertEquals("AA:BB:CC:DD:EE:02", saved.macAddress);
            assertEquals(pairedAgain, saved.serverCert);
        }
    }

    @Test
    public void aPairingMadeSinceTheLastPollIsSavedByTheNextOne() {
        try (Polls polls = new Polls(null)) {
            polls.poll();
            assertEquals(Collections.emptyList(), polls.database.written);
            assertNull(polls.saved().serverCert);

            polls.tuple.computer.serverCert = NativeApiFixtures.certificate();
            polls.poll();
            assertEquals(1, polls.database.written.size());
            assertEquals(NativeApiFixtures.certificate(), polls.saved().serverCert);
        }
    }

    @Test
    public void aPcThatSaysNotPairedHasItsCertificateForgottenOnceAndIsThenLeftAlone() {
        try (Polls polls = new Polls(NativeApiFixtures.certificate())) {
            polls.service.answer = polled -> polled.pairState = PairingManager.PairState.NOT_PAIRED;
            polls.poll();
            assertEquals(1, polls.database.written.size());
            assertNull(polls.saved().serverCert);
            assertNull(polls.tuple.computer.serverCert);

            polls.poll();
            polls.poll();
            assertEquals(1, polls.database.written.size());
            assertEquals("the rest of the PC stays", REMOTE, polls.saved().remoteAddress);
        }
    }

    // The poll reads the PC, then the user unpairs it, then the poll decides what to save. It learned
    // nothing new, so it leaves the row as the unpairing wrote it and no longer puts back what it read.
    @Test
    public void aPairingForgottenWhileAPollRunsStaysForgotten() {
        try (Polls polls = new Polls(NativeApiFixtures.certificate())) {
            polls.database.afterNextRead = () -> polls.service.forgetCertificate(UUID);
            polls.poll();
            assertNull(polls.saved().serverCert);
            assertEquals("Only the unpairing wrote", 1, polls.database.written.size());
        }
    }

    @Test
    public void aPcSeenForTheFirstTimeIsSaved() {
        try (Polls polls = new Polls(null)) {
            ComputerDetails found = new ComputerDetails();
            found.uuid = "new-host";
            found.remoteAddress = REMOTE;
            ReflectionHelpers.callInstanceMethod(polls.service, "runPoll",
                    ReflectionHelpers.ClassParameter.from(ComputerDetails.class, found),
                    ReflectionHelpers.ClassParameter.from(PollingTuple.class, null),
                    ReflectionHelpers.ClassParameter.from(int.class, 0));
            assertEquals(Collections.singletonList("new-host"), polls.database.written);
            ComputerDetails saved = polls.database.getComputerByUUID("new-host");
            assertEquals("PC", saved.name);
            assertEquals(LOCAL, saved.localAddress);
        }
    }

    // What decides whether a poll writes is ComputerDatabaseManager.columns(). A column the table
    // gains has to be compared there too, or a poll that changes only that column saves nothing.
    @Test
    public void everyColumnAndAddressOfTheTableIsCompared() throws Exception {
        try (Polls polls = new Polls(NativeApiFixtures.certificate())) {
            polls.tuple.computer.manualAddress = new ComputerDetails.AddressTuple("den.example", 47989);
            polls.service.answer = polled -> polled.ipv6Address = new ComputerDetails.AddressTuple("2001:db8::9", 47989);
            polls.poll();

            List<String> columns = new ArrayList<>();
            List<String> addresses = new ArrayList<>();
            try (SQLiteDatabase database = polls.context.openOrCreateDatabase("computers4.db", 0, null)) {
                try (Cursor table = database.rawQuery("PRAGMA table_info(Computers)", null)) {
                    while (table.moveToNext()) {
                        columns.add(table.getString(table.getColumnIndexOrThrow("name")));
                    }
                }
                try (Cursor row = database.rawQuery("SELECT Addresses FROM Computers WHERE UUID=?", new String[] {UUID})) {
                    assertTrue(row.moveToFirst());
                    JSONObject saved = new JSONObject(row.getString(0));
                    for (java.util.Iterator<String> keys = saved.keys(); keys.hasNext(); ) {
                        addresses.add(keys.next());
                    }
                }
            }
            Collections.sort(columns);
            Collections.sort(addresses);
            String hint = "ComputerDatabaseManager.columns() has to compare what is new here";
            assertEquals(hint, Arrays.asList("Addresses", "ComputerName", "MacAddress", "ServerCert", "UUID"), columns);
            assertEquals(hint, Arrays.asList("ipv6", "local", "manual", "remote"), addresses);
            // The UUID, the name, the four addresses, the MAC address and the certificate.
            assertEquals(hint, 8, ComputerDatabaseManager.columns(polls.saved()).length);
        }
    }
}
