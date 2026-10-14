package com.limelight.nvstream.mdns;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.lang.reflect.Proxy;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.util.HashSet;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

import javax.jmdns.JmDNS;
import javax.jmdns.JmmDNS;
import javax.jmdns.ServiceEvent;
import javax.jmdns.ServiceInfo;
import javax.jmdns.ServiceListener;
import javax.jmdns.impl.ServiceInfoImpl;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class JmDNSDiscoveryLifecycleTest {
    private static final class Resolution {
        final CountDownLatch entered = new CountDownLatch(1);
        final CountDownLatch release = new CountDownLatch(1);

        void awaitRelease() {
            entered.countDown();
            boolean interrupted = false;
            try {
                while (true) {
                    try {
                        assertTrue(release.await(5, TimeUnit.SECONDS));
                        return;
                    } catch (InterruptedException e) {
                        // A resolver result may arrive just after discovery is stopped.
                        interrupted = true;
                    }
                }
            } finally {
                if (interrupted) Thread.currentThread().interrupt();
            }
        }
    }

    @Test
    public void anOldResolverCannotSuppressTheRestartedSessionsHost() throws Exception {
        Inet4Address address = (Inet4Address) InetAddress.getByName("192.0.2.1");
        ServiceInfo info = new ServiceInfoImpl("_nvstream._tcp.local.", "PC", "", 47989,
                0, 0, false, "") {
            @Override public Inet4Address[] getInet4Addresses() { return new Inet4Address[] {address}; }
        };
        Resolution previous = new Resolution();
        Resolution current = new Resolution();
        LinkedBlockingQueue<Resolution> requests = new LinkedBlockingQueue<>();
        requests.add(previous);
        requests.add(current);
        AtomicReference<JmDNSDiscoveryAgent> agentReference = new AtomicReference<>();
        JmmDNS resolver = (JmmDNS) Proxy.newProxyInstance(JmmDNS.class.getClassLoader(),
                new Class<?>[] {JmmDNS.class}, (proxy, method, args) -> {
                    if (method.getName().equals("requestServiceInfo")) {
                        // Each worker reaches this before copying its own pending set. Seed the
                        // session here so the race does not depend on scheduling after start().
                        Object session = ReflectionHelpers.getField(agentReference.get(), "discoverySession");
                        HashSet<String> pending = ReflectionHelpers.getField(session, "pendingResolution");
                        synchronized (pending) {
                            pending.add("PC");
                        }
                    }
                    if (method.getName().equals("getServiceInfos")) {
                        Resolution request = requests.poll(2, TimeUnit.SECONDS);
                        assertNotNull("Unexpected resolver request", request);
                        request.awaitRelease();
                        return new ServiceInfo[] {info};
                    }
                    return null;
                });
        JmmDNS.Factory.ClassDelegate originalFactory = JmmDNS.Factory.classDelegate();
        JmmDNS.Factory.setClassDelegate(() -> resolver);
        AtomicInteger hostProbes = new AtomicInteger();
        CountDownLatch notified = new CountDownLatch(1);
        JmDNSDiscoveryAgent agent = new JmDNSDiscoveryAgent(RuntimeEnvironment.getApplication(),
                new MdnsDiscoveryListener() {
                    @Override public boolean notifyComputerAdded(MdnsComputer computer) {
                        hostProbes.incrementAndGet();
                        notified.countDown();
                        return true;
                    }
                    @Override public void notifyDiscoveryFailure(Exception e) { throw new AssertionError(e); }
                });
        agentReference.set(agent);
        Thread previousThread = null;
        Thread currentThread = null;
        try {
            agent.startDiscovery(10000);
            previousThread = ReflectionHelpers.getField(agent, "discoveryThread");
            assertTrue(previous.entered.await(2, TimeUnit.SECONDS));
            agent.stopDiscovery();
            agent.startDiscovery(10000);
            currentThread = ReflectionHelpers.getField(agent, "discoveryThread");
            assertTrue(current.entered.await(2, TimeUnit.SECONDS));
            previous.release.countDown();
            previousThread.join(2000);
            assertFalse(previousThread.isAlive());
            assertEquals("The stopped resolver cannot populate the new session", 0, hostProbes.get());
            assertTrue(agent.getComputerSet().isEmpty());

            current.release.countDown();
            assertTrue(notified.await(2, TimeUnit.SECONDS));
            assertEquals(1, hostProbes.get());
        } finally {
            previous.release.countDown();
            current.release.countDown();
            agent.stopDiscovery();
            if (previousThread != null) previousThread.join(2000);
            if (currentThread != null) currentThread.join(2000);
            JmmDNS.Factory.close();
            JmmDNS.Factory.setClassDelegate(originalFactory);
        }
    }

    @Test
    public void aListenerSnapshotFromBeforeRestartCannotResolveIntoTheNewSession() throws Exception {
        LinkedBlockingQueue<Thread> startedWorkers = new LinkedBlockingQueue<>();
        JmmDNS resolver = (JmmDNS) Proxy.newProxyInstance(JmmDNS.class.getClassLoader(),
                new Class<?>[] {JmmDNS.class}, (proxy, method, args) -> {
                    if (method.getName().equals("requestServiceInfo")) {
                        startedWorkers.add(Thread.currentThread());
                    }
                    return null;
                });
        JmmDNS.Factory.ClassDelegate originalFactory = JmmDNS.Factory.classDelegate();
        JmmDNS.Factory.setClassDelegate(() -> resolver);
        AtomicInteger hostProbes = new AtomicInteger();
        JmDNSDiscoveryAgent agent = new JmDNSDiscoveryAgent(RuntimeEnvironment.getApplication(),
                new MdnsDiscoveryListener() {
                    @Override public boolean notifyComputerAdded(MdnsComputer computer) {
                        hostProbes.incrementAndGet();
                        return true;
                    }
                    @Override public void notifyDiscoveryFailure(Exception e) { throw new AssertionError(e); }
                });
        Thread previousThread = null;
        Thread currentThread = null;
        try {
            agent.startDiscovery(10000);
            previousThread = ReflectionHelpers.getField(agent, "discoveryThread");
            assertSame(previousThread, startedWorkers.poll(2, TimeUnit.SECONDS));
            // The static dispatcher can retain this listener in a snapshot across stop/start.
            ServiceListener previousListener = ReflectionHelpers.getField(agent, "discoverySession");
            agent.stopDiscovery();
            previousThread.join(2000);
            assertFalse(previousThread.isAlive());
            agent.startDiscovery(10000);
            currentThread = ReflectionHelpers.getField(agent, "discoveryThread");
            assertSame(currentThread, startedWorkers.poll(2, TimeUnit.SECONDS));
            ServiceListener currentListener = ReflectionHelpers.getField(agent, "discoverySession");
            assertNotSame(previousListener, currentListener);
            HashSet<String> currentPending = ReflectionHelpers.getField(currentListener, "pendingResolution");
            synchronized (currentPending) {
                currentPending.add("PC");
            }

            ServiceInfo info = ServiceInfo.create("_nvstream._tcp.local.", "PC", 47989, "");
            AtomicInteger resolverEntries = new AtomicInteger();
            RuntimeException resolverEntered = new IllegalStateException("Resolver entered");
            ServiceEvent event = new ServiceEvent(agent) {
                @Override public JmDNS getDNS() {
                    resolverEntries.incrementAndGet();
                    throw resolverEntered;
                }
                @Override public String getType() { return info.getType(); }
                @Override public String getName() { return info.getName(); }
                @Override public ServiceInfo getInfo() { return info; }
            };

            previousListener.serviceAdded(event);
            assertEquals("Stopped listener callbacks must not enter the resolver", 0, resolverEntries.get());
            assertEquals(0, hostProbes.get());
            assertTrue(agent.getComputerSet().isEmpty());
            synchronized (currentPending) {
                assertEquals(java.util.Collections.singleton("PC"), currentPending);
            }
            // The current wrapper must still accept callbacks. Stop at the resolver boundary
            // without starting network I/O, proving the preceding no-op was session-specific.
            try {
                currentListener.serviceAdded(event);
                fail("The active session must reach the resolver");
            } catch (IllegalStateException expected) {
                assertSame(resolverEntered, expected);
            }
            assertEquals(1, resolverEntries.get());
        } finally {
            agent.stopDiscovery();
            if (previousThread != null) previousThread.join(2000);
            if (currentThread != null) currentThread.join(2000);
            JmmDNS.Factory.close();
            JmmDNS.Factory.setClassDelegate(originalFactory);
        }
    }
}
