package com.limelight.computers;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;

import com.limelight.AppView;
import com.limelight.discovery.DiscoveryService;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.nvstream.mdns.MdnsComputer;
import com.limelight.nvstream.mdns.MdnsDiscoveryAgent;
import com.limelight.nvstream.mdns.MdnsDiscoveryListener;
import com.limelight.utils.CacheHelper;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ServiceController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class ComputerManagerLifecycleTest {
    public static class Manager extends ComputerManagerService {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }
    }

    @Test
    public void deletingComputerRemovesItsCachedLibraryArtworkAndHiddenAppsOnly() throws Exception {
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "removed-host";
        try {
            for (String host : new String[] {computer.uuid, "another-host"}) {
                Files.writeString(CacheHelper.openPath(true, service.getCacheDir(), "applist", host).toPath(), "library");
                Files.writeString(CacheHelper.openPath(true, service.getCacheDir(), "boxart", host, "42.png").toPath(), "cover");
                service.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, 0).edit()
                        .putStringSet(host, Collections.singleton("42")).commit();
            }

            service.removeComputer(computer);

            assertFalse(CacheHelper.openPath(false, service.getCacheDir(), "applist", computer.uuid).exists());
            assertFalse(CacheHelper.openPath(false, service.getCacheDir(), "boxart", computer.uuid).exists());
            assertFalse(service.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, 0).contains(computer.uuid));
            assertTrue(CacheHelper.openPath(false, service.getCacheDir(), "applist", "another-host").isFile());
            assertTrue(CacheHelper.openPath(false, service.getCacheDir(), "boxart", "another-host", "42.png").isFile());
            assertTrue(service.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, 0).contains("another-host"));
        } finally {
            controller.destroy();
        }
    }

    @Test
    public void deletingComputerRemovesOnlyItsSavedStreamSettings() {
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "removed-host";
        String[] files = {"HostStreamProfiles", "HostDisplayChoices", "AdaptiveBitrate", "PyroWaveBitrate",
                HostCapabilityCache.PREFERENCES, HostNotices.PREFERENCES, HostNames.PREFERENCES};
        try {
            for (String file : files) {
                service.getSharedPreferences(file, 0).edit()
                        .putString(computer.uuid, "saved").putString("another-host", "retained").commit();
            }
            service.removeComputer(computer);
            for (String file : files) {
                assertFalse(service.getSharedPreferences(file, 0).contains(computer.uuid));
                assertEquals("retained", service.getSharedPreferences(file, 0).getString("another-host", null));
            }
        } finally {
            controller.destroy();
        }
    }

    @SuppressWarnings("unchecked")
    private static ComputerDetails tupleOf(ComputerManagerService service, String uuid) {
        List<Object> tuples = ReflectionHelpers.getField(service, "pollingTuples");
        for (Object tuple : tuples) {
            ComputerDetails computer = ReflectionHelpers.getField(tuple, "computer");
            if (uuid.equals(computer.uuid)) return computer;
        }
        return null;
    }

    private static void addTuple(ComputerManagerService service, ComputerDetails computer) {
        ReflectionHelpers.callInstanceMethod(service, "addTuple",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, computer));
    }

    @Test
    public void aNameGivenToAPcSurvivesPollsThatRenameTheHost() {
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        try {
            ComputerDetails stored = new ComputerDetails();
            stored.uuid = "named-host";
            stored.name = "DESKTOP-1";
            addTuple(service, stored);
            assertNull(tupleOf(service, "named-host").alias);

            service.renameComputer("named-host", "Den PC");
            ComputerDetails tuple = tupleOf(service, "named-host");
            assertEquals("Den PC", tuple.alias);
            assertEquals("Den PC", HostNames.get(service, "named-host"));

            // A poll reads a new host name; the alias, not the name, is shown and it stays.
            ComputerDetails polled = new ComputerDetails();
            polled.uuid = "named-host";
            polled.name = "DESKTOP-2";
            tuple.update(polled);
            assertEquals("DESKTOP-2", tuple.name);
            assertEquals("Den PC", tuple.alias);
            assertEquals("Den PC", tuple.displayName());
            assertEquals("Den PC", new ComputerDetails(tuple).displayName());

            // Discovery adds the PC again, with no alias of its own; the stored one applies.
            ComputerDetails rediscovered = new ComputerDetails();
            rediscovered.uuid = "named-host";
            rediscovered.name = "DESKTOP-3";
            addTuple(service, rediscovered);
            assertEquals("Den PC", rediscovered.alias);
            assertEquals("Den PC", tupleOf(service, "named-host").alias);
            assertEquals("DESKTOP-3", tupleOf(service, "named-host").name);

            // The name shown goes back to the PC's own when the alias is cleared.
            service.renameComputer("named-host", null);
            assertEquals("DESKTOP-3", tupleOf(service, "named-host").displayName());
            assertNull(HostNames.get(service, "named-host"));
        } finally {
            controller.destroy();
        }
    }

    public static class OnlineManager extends Manager {
        @Override boolean pollComputer(ComputerDetails details) {
            details.state = ComputerDetails.State.ONLINE;
            return true;
        }
    }

    @Test
    public void aRediscoveredPcIsFirstShownWithItsStoredName() throws Exception {
        ServiceController<OnlineManager> controller = Robolectric.buildService(OnlineManager.class).create();
        ComputerManagerService service = controller.get();
        try {
            HostNames.set(service, "named-host", "Den PC");
            List<String> shown = new ArrayList<>();
            ReflectionHelpers.setField(service, "listener", (ComputerManagerListener) details ->
                    shown.add(details.displayName()));

            ComputerDetails found = new ComputerDetails();
            found.uuid = "named-host";
            found.name = "DESKTOP-9";
            ComputerDetails.AddressTuple address = new ComputerDetails.AddressTuple("192.168.1.9", 47989);
            found.activeAddress = address;
            found.remoteAddress = address;
            ReflectionHelpers.callInstanceMethod(service, "runPoll",
                    ReflectionHelpers.ClassParameter.from(ComputerDetails.class, found),
                    ReflectionHelpers.ClassParameter.from(boolean.class, true),
                    ReflectionHelpers.ClassParameter.from(int.class, 0));

            assertEquals(Collections.singletonList("Den PC"), shown);
            assertEquals("DESKTOP-9", found.name);
        } finally {
            controller.destroy();
        }
    }

    @Test
    public void deletingComputerForgetsItsNameAndKeepsOthers() {
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        try {
            HostNames.set(service, "removed-host", "Den PC");
            HostNames.set(service, "another-host", "Kitchen");
            ComputerDetails computer = new ComputerDetails();
            computer.uuid = "removed-host";
            service.removeComputer(computer);
            assertNull(HostNames.get(service, "removed-host"));
            assertEquals("Kitchen", HostNames.get(service, "another-host"));
        } finally {
            controller.destroy();
        }
    }

    // Stands in for the platform resolver so the test can announce a host at an address.
    private static final class Announcer extends MdnsDiscoveryAgent {
        Announcer(MdnsDiscoveryListener listener) { super(listener); }
        @Override public void startDiscovery(int interval) { resetDiscovery(); }
        @Override public void stopDiscovery() { resetDiscovery(); }
        void announce(String name, String address) throws Exception {
            reportNewComputer(name, 47989, new Inet4Address[] {(Inet4Address) InetAddress.getByName(address)},
                    new Inet6Address[0]);
        }
    }

    @Test
    public void aDeletedComputerIsProbedAgainWhenDiscoveryFindsIt() throws Exception {
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        ServiceController<DiscoveryService> discoveryController = Robolectric.buildService(DiscoveryService.class).create();
        ComputerManagerService service = controller.get();
        DiscoveryService discovery = discoveryController.get();
        MdnsDiscoveryAgent platformAgent = ReflectionHelpers.getField(discovery, "discoveryAgent");
        Announcer agent = new Announcer(ReflectionHelpers.getField(platformAgent, "listener"));
        ReflectionHelpers.setField(discovery, "discoveryAgent", agent);
        DiscoveryService.DiscoveryBinder binder = (DiscoveryService.DiscoveryBinder) discovery.onBind(new Intent());
        List<String> probes = new ArrayList<>();
        binder.setListener(new MdnsDiscoveryListener() {
            @Override public boolean notifyComputerAdded(MdnsComputer computer) {
                probes.add(computer.getName());
                return true;
            }
            @Override public void notifyDiscoveryFailure(Exception e) { throw new AssertionError(e); }
        });
        ReflectionHelpers.setField(service, "discoveryBinder", binder);
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "removed-host";
        try {
            for (int session = 0; session < 2; session++) {
                binder.startDiscovery(1000);
                agent.announce("PC", "192.0.2.10");
                binder.stopDiscovery();
            }
            assertEquals("An answered host is not probed on every restart", Collections.singletonList("PC"), probes);

            service.removeComputer(computer);
            binder.startDiscovery(1000);
            agent.announce("PC", "192.0.2.10");
            assertEquals("The deleted PC must be able to come back through mDNS",
                    Arrays.asList("PC", "PC"), probes);
        } finally {
            discoveryController.destroy();
            controller.destroy();
        }
    }

    @Test
    public void offlinePcsKeepTheCapabilitiesTheyLastReported() {
        android.content.Context context = org.robolectric.RuntimeEnvironment.getApplication();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "cached-host";
        computer.name = "Cached";
        ComputerDatabaseManager database = new ComputerDatabaseManager(context);
        database.updateComputer(computer);
        database.close();
        HostCapabilityCache.save(context, computer.uuid, com.limelight.nvstream.http.TestHosts.fixture("vibepollo-serverinfo.xml"));
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        try {
            java.util.List<?> tuples = ReflectionHelpers.getField(controller.get(), "pollingTuples");
            ComputerDetails restored = ReflectionHelpers.getField(tuples.get(0), "computer");
            assertEquals(computer.uuid, restored.uuid);
            assertEquals(com.limelight.nvstream.http.HostCapabilities.Family.VIBEPOLLO, restored.caps.family());
            assertTrue(restored.caps.has(com.limelight.nvstream.http.HostCapabilities.Feature.VIRTUAL_DISPLAY));
        } finally {
            controller.destroy();
        }
    }

    /** Answers every poll and has a scripted native API. */
    public static class NativeManager extends OnlineManager {
        final NativeApiFixtures.FakeTransport transport = new NativeApiFixtures.FakeTransport();

        @Override NativeApiClient nativeClient(ComputerDetails computer) {
            return new NativeApiClient(transport, HostEndpoint.of(computer, "unique", NativeApiFixtures.crypto()));
        }
    }

    @Test
    public void aPollOfAPcThatListsWakeKeepsItsTargetsForThePcListAndShortcuts() {
        ServiceController<NativeManager> controller = Robolectric.buildService(NativeManager.class).create();
        NativeManager service = controller.get();
        try {
            service.transport.reply("tower", "capabilities", NativeApiFixtures.ok("{\"api_version\":1,"
                    + "\"features\":[\"wake\"],\"permissions\":{\"list\":true,\"view\":true,\"launch\":true,"
                    + "\"host_control\":false}}"));
            service.transport.reply("tower", "wake", NativeApiFixtures.ok("{\"revision\":1,\"targets\":["
                    + "{\"mac\":\"02:e0:4c:68:00:01\",\"broadcasts\":[\"10.0.0.255\"]}]}"));
            ComputerDetails found = NativeApiFixtures.nativeHost("tower", "Tower", false);
            found.remoteAddress = found.activeAddress;
            ReflectionHelpers.callInstanceMethod(service, "runPoll",
                    ReflectionHelpers.ClassParameter.from(ComputerDetails.class, found),
                    ReflectionHelpers.ClassParameter.from(boolean.class, true),
                    ReflectionHelpers.ClassParameter.from(int.class, 0));

            WakeTargets.Targets targets = new WakeTargets(service).load("tower");
            assertEquals(Collections.singletonList("02:e0:4c:68:00:01"), targets.macs);
            assertEquals(Collections.singletonList("10.0.0.255"), targets.broadcasts);
        } finally {
            controller.destroy();
        }
    }

    @Test
    public void failingPollListenerStillReleasesTheDatabaseReference() throws Exception {
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "pc";
        computer.state = ComputerDetails.State.OFFLINE;
        ReflectionHelpers.setField(service, "listener", (ComputerManagerListener) details -> {
            throw new IllegalStateException("Listener failed");
        });
        Method poll = ComputerManagerService.class.getDeclaredMethod("runPoll", ComputerDetails.class, boolean.class, int.class);
        poll.setAccessible(true);
        try {
            InvocationTargetException failure = assertThrows(InvocationTargetException.class,
                    () -> poll.invoke(service, computer, false, 3));
            assertEquals("Listener failed", failure.getCause().getMessage());
            AtomicInteger references = ReflectionHelpers.getField(service, "dbRefCount");
            assertEquals(1, references.get());
        } finally {
            controller.destroy();
        }
    }
}
