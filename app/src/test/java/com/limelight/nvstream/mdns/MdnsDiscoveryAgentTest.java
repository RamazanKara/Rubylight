package com.limelight.nvstream.mdns;

import org.junit.Test;

import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

public class MdnsDiscoveryAgentTest {
    @Test
    public void blockingHostProbeDoesNotLockTheDiscoveredComputerSet() throws Exception {
        CountDownLatch probing = new CountDownLatch(1);
        CountDownLatch finish = new CountDownLatch(1);
        MdnsDiscoveryAgent agent = new MdnsDiscoveryAgent(new MdnsDiscoveryListener() {
            public boolean notifyComputerAdded(MdnsComputer computer) {
                probing.countDown();
                try { finish.await(); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
                return true;
            }
            public void notifyDiscoveryFailure(Exception e) { throw new AssertionError(e); }
        }) {
            public void startDiscovery(int interval) { }
            public void stopDiscovery() { }
        };
        Inet4Address address = (Inet4Address) InetAddress.getByName("127.0.0.1");
        ExecutorService workers = Executors.newFixedThreadPool(2);
        try {
            Future<?> probe = workers.submit(() -> agent.reportNewComputer("PC", 47989,
                    new Inet4Address[] {address}, new Inet6Address[0]));
            assertTrue(probing.await(2, TimeUnit.SECONDS));
            assertEquals(1, workers.submit(agent::getComputerSet).get(2, TimeUnit.SECONDS).size());
            finish.countDown();
            probe.get(2, TimeUnit.SECONDS);
        } finally {
            finish.countDown();
            workers.shutdownNow();
        }
    }

    @Test
    public void aHostThatMovesBackToAnEarlierAddressIsProbedThere() throws Exception {
        List<String> probes = new ArrayList<>();
        MdnsDiscoveryAgent agent = new MdnsDiscoveryAgent(new MdnsDiscoveryListener() {
            public boolean notifyComputerAdded(MdnsComputer computer) {
                probes.add(computer.getName() + " " + computer.getLocalAddress().getHostAddress());
                return true;
            }
            public void notifyDiscoveryFailure(Exception e) { throw new AssertionError(e); }
        }) {
            public void startDiscovery(int interval) { resetDiscovery(); }
            public void stopDiscovery() { resetDiscovery(); }
        };
        String[][] sessions = {
                {"PC", "192.0.2.10"}, // answers at A
                {"PC", "192.0.2.20"}, // moves to B; the probe there points polling at B
                {"PC", "192.0.2.10"}, // back at A, which polling no longer tries
                {"PC", "192.0.2.10"}, // answered at A again, no new probe
        };
        for (String[] session : sessions) {
            agent.startDiscovery(1000);
            announce(agent, session[0], session[1]);
            announce(agent, "Other", "192.0.2.30");
            agent.stopDiscovery();
        }
        assertEquals(Arrays.asList("PC 192.0.2.10", "Other 192.0.2.30", "PC 192.0.2.20", "PC 192.0.2.10"), probes);
    }

    private static void announce(MdnsDiscoveryAgent agent, String name, String address) throws Exception {
        agent.reportNewComputer(name, 47989, new Inet4Address[] {(Inet4Address) InetAddress.getByName(address)},
                new Inet6Address[0]);
    }

    @Test
    public void unspecifiedAndMulticastAddressesAreNotGlobalHosts() throws Exception {
        assertNull(MdnsDiscoveryAgent.getBestIpv6Address(new Inet6Address[] {
                (Inet6Address) InetAddress.getByName("::"), (Inet6Address) InetAddress.getByName("ff02::1")}));
    }

}
