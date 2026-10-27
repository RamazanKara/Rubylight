package com.limelight.nvstream.mdns;

import android.app.Application;
import android.net.nsd.NsdManager;
import android.net.nsd.NsdServiceInfo;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.annotation.RealObject;
import org.robolectric.shadow.api.Shadow;

import java.net.InetAddress;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.Executor;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, shadows = {
        NsdDiscoveryLifecycleTest.RecordingNsdManager.class, NsdDiscoveryLifecycleTest.DelayedServiceInfo.class})
public class NsdDiscoveryLifecycleTest {
    @Implements(NsdServiceInfo.class)
    public static class DelayedServiceInfo {
        @RealObject private NsdServiceInfo serviceInfo;
        CountDownLatch entered;
        CountDownLatch release;

        @Implementation protected List<InetAddress> getHostAddresses() {
            if (entered != null) {
                entered.countDown();
                try {
                    assertTrue(release.await(3, TimeUnit.SECONDS));
                } catch (InterruptedException e) {
                    throw new AssertionError(e);
                }
            }
            return Shadow.directlyOn(serviceInfo, NsdServiceInfo.class, "getHostAddresses");
        }
    }

    @Implements(NsdManager.class)
    public static class RecordingNsdManager {
        NsdManager.DiscoveryListener discovery;
        NsdManager.ServiceInfoCallback callback;

        @Implementation protected void discoverServices(String type, int protocol, NsdManager.DiscoveryListener listener) {
            discovery = listener;
        }

        @Implementation protected void stopServiceDiscovery(NsdManager.DiscoveryListener listener) { }

        @Implementation protected void registerServiceInfoCallback(NsdServiceInfo info, Executor executor,
                                                                    NsdManager.ServiceInfoCallback callback) {
            this.callback = callback;
        }

        @Implementation protected void unregisterServiceInfoCallback(NsdManager.ServiceInfoCallback callback) { }
    }

    @Test
    public void anOldResolutionCannotFillTheRestartedDiscoverysCache() throws Exception {
        Application application = RuntimeEnvironment.getApplication();
        RecordingNsdManager manager = Shadow.extract(application.getSystemService(NsdManager.class));
        AtomicInteger hostProbes = new AtomicInteger();
        NsdManagerDiscoveryAgent agent = new NsdManagerDiscoveryAgent(application, new MdnsDiscoveryListener() {
            @Override public boolean notifyComputerAdded(MdnsComputer computer) {
                hostProbes.incrementAndGet();
                return true;
            }
            @Override public void notifyDiscoveryFailure(Exception e) { throw new AssertionError(e); }
        });
        NsdServiceInfo info = new NsdServiceInfo();
        info.setServiceName("PC");
        info.setPort(47989);
        info.setHostAddresses(Collections.singletonList(InetAddress.getByName("192.0.2.1")));
        DelayedServiceInfo delayed = Shadow.extract(info);
        delayed.entered = new CountDownLatch(1);
        delayed.release = new CountDownLatch(1);
        ExecutorService thread = Executors.newSingleThreadExecutor();
        try {
            agent.startDiscovery(1000);
            manager.discovery.onDiscoveryStarted("_nvstream._tcp");
            manager.discovery.onServiceFound(info);
            NsdManager.ServiceInfoCallback previous = manager.callback;
            Future<?> resolution = thread.submit(() -> previous.onServiceUpdated(info));
            assertTrue(delayed.entered.await(2, TimeUnit.SECONDS));
            agent.stopDiscovery();
            agent.startDiscovery(1000);
            manager.discovery.onDiscoveryStarted("_nvstream._tcp");
            delayed.release.countDown();
            resolution.get(2, TimeUnit.SECONDS);
            assertEquals("A resolution from a stopped session cannot probe or cache a host", 0, hostProbes.get());
            assertTrue(agent.getComputerSet().isEmpty());

            manager.discovery.onServiceFound(info);
            manager.callback.onServiceUpdated(info);
            assertEquals(1, hostProbes.get());
        } finally {
            delayed.release.countDown();
            thread.shutdownNow();
            assertTrue(thread.awaitTermination(2, TimeUnit.SECONDS));
            agent.stopDiscovery();
        }
    }

    @Test
    public void restartingDiscoveryRetriesHostsWhoseFirstProbeFailed() throws Exception {
        Application application = RuntimeEnvironment.getApplication();
        RecordingNsdManager manager = Shadow.extract(application.getSystemService(NsdManager.class));
        AtomicInteger hostProbes = new AtomicInteger();
        NsdManagerDiscoveryAgent agent = new NsdManagerDiscoveryAgent(application, new MdnsDiscoveryListener() {
            @Override public boolean notifyComputerAdded(MdnsComputer computer) {
                // ComputerManager's first serverinfo request may fail while the host is starting.
                // It does not save that host, so a new discovery session must report it again.
                hostProbes.incrementAndGet();
                return false;
            }
            @Override public void notifyDiscoveryFailure(Exception e) { throw new AssertionError(e); }
        });
        NsdServiceInfo info = new NsdServiceInfo();
        info.setServiceName("PC");
        info.setPort(47989);
        info.setHostAddresses(Collections.singletonList(InetAddress.getByName("192.0.2.1")));
        try {
            agent.startDiscovery(1000);
            manager.discovery.onDiscoveryStarted("_nvstream._tcp");
            manager.discovery.onServiceFound(info);
            NsdManager.ServiceInfoCallback previous = manager.callback;
            previous.onServiceUpdated(info);
            previous.onServiceUpdated(info);
            assertEquals("One probe per host within a discovery session", 1, hostProbes.get());

            agent.stopDiscovery();
            agent.startDiscovery(1000);
            manager.discovery.onDiscoveryStarted("_nvstream._tcp");
            previous.onServiceUpdated(info);
            assertEquals("Stopped callbacks cannot seed the new session's cache", 1, hostProbes.get());
            manager.discovery.onServiceFound(info);
            manager.callback.onServiceUpdated(info);
            assertEquals("Returning to PCs must retry the previously unreachable host", 2, hostProbes.get());

            agent.startDiscovery(1000);
            manager.callback.onServiceUpdated(info);
            assertEquals("Starting an already active discovery must retain deduplication", 2, hostProbes.get());
        } finally {
            agent.stopDiscovery();
        }
    }

    @Test
    public void restartingDiscoveryDoesNotProbeAHostThatAlreadyAnswered() throws Exception {
        Application application = RuntimeEnvironment.getApplication();
        RecordingNsdManager manager = Shadow.extract(application.getSystemService(NsdManager.class));
        List<String> probes = new ArrayList<>();
        NsdManagerDiscoveryAgent agent = new NsdManagerDiscoveryAgent(application, new MdnsDiscoveryListener() {
            @Override public boolean notifyComputerAdded(MdnsComputer computer) {
                // Each probe is a STUN query and a serverinfo request; this host answers.
                probes.add(computer.getLocalAddress().getHostAddress());
                return true;
            }
            @Override public void notifyDiscoveryFailure(Exception e) { throw new AssertionError(e); }
        });
        NsdServiceInfo info = new NsdServiceInfo();
        info.setServiceName("PC");
        info.setPort(47989);
        info.setHostAddresses(Collections.singletonList(InetAddress.getByName("192.0.2.1")));
        try {
            for (int resume = 0; resume < 3; resume++) {
                // The PC list restarts discovery on every resume and around each PC menu.
                agent.stopDiscovery();
                agent.startDiscovery(1000);
                manager.discovery.onDiscoveryStarted("_nvstream._tcp");
                manager.discovery.onServiceFound(info);
                manager.callback.onServiceUpdated(info);
                assertEquals(1, agent.getComputerSet().size());
            }
            assertEquals(Collections.singletonList("192.0.2.1"), probes);

            // A host that moved to another address is new and is probed there.
            info.setHostAddresses(Collections.singletonList(InetAddress.getByName("192.0.2.2")));
            manager.callback.onServiceUpdated(info);
            assertEquals(Arrays.asList("192.0.2.1", "192.0.2.2"), probes);
        } finally {
            agent.stopDiscovery();
        }
    }

    @Test
    public void stoppingDiscoveryDoesNotWaitForTheHostNetworkRequest() throws Exception {
        Application application = RuntimeEnvironment.getApplication();
        RecordingNsdManager manager = Shadow.extract(application.getSystemService(NsdManager.class));
        CountDownLatch entered = new CountDownLatch(1);
        CountDownLatch release = new CountDownLatch(1);
        NsdManagerDiscoveryAgent agent = new NsdManagerDiscoveryAgent(application, new MdnsDiscoveryListener() {
            @Override public boolean notifyComputerAdded(MdnsComputer computer) {
                entered.countDown();
                try {
                    release.await();
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
                return true;
            }
            @Override public void notifyDiscoveryFailure(Exception e) { throw new AssertionError(e); }
        });
        agent.startDiscovery(1000);
        manager.discovery.onDiscoveryStarted("_nvstream._tcp");
        NsdServiceInfo info = new NsdServiceInfo();
        info.setServiceName("PC");
        info.setPort(47989);
        info.setHostAddresses(Collections.singletonList(InetAddress.getByName("192.0.2.1")));
        manager.discovery.onServiceFound(info);
        ExecutorService threads = Executors.newFixedThreadPool(2);
        try {
            Future<?> update = threads.submit(() -> manager.callback.onServiceUpdated(info));
            assertTrue(entered.await(2, TimeUnit.SECONDS));
            threads.submit(agent::stopDiscovery).get(1, TimeUnit.SECONDS);
            release.countDown();
            update.get(2, TimeUnit.SECONDS);
        } finally {
            release.countDown();
            threads.shutdownNow();
            assertTrue(threads.awaitTermination(2, TimeUnit.SECONDS));
            agent.stopDiscovery();
        }
    }
}
