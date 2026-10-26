package com.limelight.nvstream.wol;

import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Test;

import java.io.IOException;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.SocketAddress;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;

import static org.junit.Assert.*;

public class WakeOnLanSenderTest {
    private static final int[] PORTS = {7, 9, 47009, 47998, 47999, 48000, 48002, 48010};

    private static ComputerDetails pc(String mac) {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "pc";
        computer.macAddress = mac;
        computer.localAddress = new ComputerDetails.AddressTuple("192.168.4.20", 47989);
        return computer;
    }

    private static Set<InetAddress> addresses(String... literals) throws Exception {
        Set<InetAddress> addresses = new LinkedHashSet<>();
        for (String literal : literals) addresses.add(InetAddress.getByName(literal));
        return addresses;
    }

    /** A socket that records each packet and fails the sends to {@code failing}. */
    private static final class FakeSocket extends DatagramSocket {
        final List<DatagramPacket> sent = new ArrayList<>();
        final Set<InetAddress> failing;

        FakeSocket(Set<InetAddress> failing) throws IOException {
            super((SocketAddress) null);
            this.failing = failing;
        }

        @Override public void send(DatagramPacket packet) throws IOException {
            sent.add(new DatagramPacket(packet.getData().clone(), packet.getLength(), packet.getSocketAddress()));
            if (failing.contains(packet.getAddress())) throw new IOException("No route");
        }
    }

    @Test
    public void magicPacketContainsAllSixteenCopiesAndAcceptsBothSeparators() throws Exception {
        byte[] payload = WakeOnLanSender.createWolPayload("01:23:45:67:89:aB");
        assertEquals(102, payload.length);
        assertArrayEquals(new byte[] {-1, -1, -1, -1, -1, -1}, Arrays.copyOf(payload, 6));
        for (int i = 6; i < payload.length; i += 6) {
            assertArrayEquals(new byte[] {1, 0x23, 0x45, 0x67, (byte) 0x89, (byte) 0xab},
                    Arrays.copyOfRange(payload, i, i + 6));
        }
        assertArrayEquals(payload, WakeOnLanSender.createWolPayload("01-23-45-67-89-AB"));
    }

    @Test
    public void malformedMacNeverProducesAPartialPacket() {
        for (String mac : new String[] {null, "", "01:23:45", "01:23:45:67:89:zz", "1:23:45:67:89:ab",
                "01:23:45:67:89:abc", "01:23:45:67:89:ab:cd", "01-23:45:67:89:ab"}) {
            assertThrows(IOException.class, () -> WakeOnLanSender.createWolPayload(mac));
        }
    }

    @Test
    public void alternatePortsAreBoundedAndDuplicateSubnetsAreSentOnlyOnce() throws Exception {
        InetAddress wifi = InetAddress.getByName("192.168.1.255");
        InetAddress ethernet = InetAddress.getByName("10.2.255.255");
        Set<InetSocketAddress> targets = WakeOnLanSender.destinations(wifi, 48000);
        assertTrue(targets.contains(new InetSocketAddress(wifi, 7)));
        assertTrue(targets.contains(new InetSocketAddress(wifi, 48009)));
        assertTrue(targets.contains(new InetSocketAddress(wifi, 48021)));
        targets.addAll(WakeOnLanSender.destinations(wifi, 48000));
        targets.addAll(WakeOnLanSender.destinations(ethernet, 48000));
        assertEquals(16, targets.size());
        assertEquals(3, WakeOnLanSender.destinations(wifi, 65535).size());
        assertEquals(8, WakeOnLanSender.destinations(InetAddress.getByName("::1"), 47989).size());
    }

    @Test
    public void twoMacsGoToThePcEveryBroadcastAndEveryPortWithOnePayloadEach() throws Exception {
        WakeTargets.Targets extra = new WakeTargets.Targets(Arrays.asList("02:7c:16:b4:13:4f", "02:e0:4c:68:00:01"),
                Arrays.asList("10.0.0.255", "192.168.1.255"));
        // Serverinfo's MAC is the first of /wake's, written the other way.
        WakeOnLanSender.Plan plan = WakeOnLanSender.plan(pc("02-7C-16-B4-13-4F"), extra,
                addresses("192.168.1.255"), addresses("ff02::1"));

        assertEquals(2, plan.payloads.size());
        assertArrayEquals(WakeOnLanSender.createWolPayload("02:7c:16:b4:13:4f"), plan.payloads.get(0));
        assertArrayEquals(WakeOnLanSender.createWolPayload("02:e0:4c:68:00:01"), plan.payloads.get(1));

        Set<InetSocketAddress> expected = new LinkedHashSet<>();
        for (String address : new String[] {"192.168.4.20", "255.255.255.255", "192.168.1.255", "10.0.0.255"}) {
            for (int port : PORTS) expected.add(new InetSocketAddress(InetAddress.getByName(address), port));
        }
        assertEquals(expected, plan.destinations);
        Set<InetSocketAddress> allNodes = new LinkedHashSet<>();
        for (int port : PORTS) allNodes.add(new InetSocketAddress(InetAddress.getByName("ff02::1"), port));
        assertEquals(allNodes, plan.bestEffort);

        try (FakeSocket socket = new FakeSocket(Collections.emptySet())) {
            WakeOnLanSender.send(plan, socket);
            // Three bursts of each payload to every destination.
            assertEquals(3 * 2 * (expected.size() + allNodes.size()), socket.sent.size());
        }
    }

    @Test
    public void aHostWithoutWakeTargetsGetsServerinfosMacOnly() throws Exception {
        WakeOnLanSender.Plan plan = WakeOnLanSender.plan(pc("02:7c:16:b4:13:4f"), WakeTargets.Targets.NONE,
                Collections.emptySet(), Collections.emptySet());
        assertEquals(1, plan.payloads.size());
        assertArrayEquals(WakeOnLanSender.createWolPayload("02:7c:16:b4:13:4f"), plan.payloads.get(0));
        assertTrue(plan.bestEffort.isEmpty());
        assertTrue(plan.destinations.contains(new InetSocketAddress(InetAddress.getByName("255.255.255.255"), 9)));

        // Serverinfo's MAC is unreadable, but /wake listed another.
        WakeTargets.Targets extra = new WakeTargets.Targets(Collections.singletonList("02:e0:4c:68:00:01"),
                Collections.emptyList());
        assertEquals(1, WakeOnLanSender.plan(pc("garbage"), extra, Collections.emptySet(),
                Collections.emptySet()).payloads.size());
        assertThrows(IOException.class, () -> WakeOnLanSender.plan(pc("garbage"), WakeTargets.Targets.NONE,
                Collections.emptySet(), Collections.emptySet()));
        assertThrows(IOException.class, () -> WakeOnLanSender.plan(pc(null), WakeTargets.Targets.NONE,
                Collections.emptySet(), Collections.emptySet()));
    }

    @Test
    public void allNodesFailuresDoNotFailTheWake() throws Exception {
        WakeOnLanSender.Plan plan = WakeOnLanSender.plan(pc("02:7c:16:b4:13:4f"), WakeTargets.Targets.NONE,
                Collections.emptySet(), addresses("ff02::1"));
        try (FakeSocket socket = new FakeSocket(addresses("ff02::1"))) {
            WakeOnLanSender.send(plan, socket);
        }

        // Only the best-effort sends went out: the PC was not reached.
        Set<InetAddress> ipv4 = addresses("192.168.4.20", "255.255.255.255");
        try (FakeSocket socket = new FakeSocket(ipv4)) {
            assertThrows(IOException.class, () -> WakeOnLanSender.send(plan, socket));
        }
    }

    @Test
    public void failedNicDoesNotPreventRemainingSubnetsAndPorts() throws Exception {
        InetAddress unavailable = InetAddress.getByName("192.168.1.255");
        WakeOnLanSender.Plan plan = WakeOnLanSender.plan(pc("01:23:45:67:89:ab"), WakeTargets.Targets.NONE,
                addresses("192.168.1.255", "10.0.0.255"), Collections.emptySet());
        Set<SocketAddress> attempted = new LinkedHashSet<>();
        try (FakeSocket socket = new FakeSocket(Collections.singleton(unavailable))) {
            WakeOnLanSender.send(plan, socket);
            for (DatagramPacket packet : socket.sent) attempted.add(packet.getSocketAddress());
        }
        assertEquals(plan.destinations, attempted);
    }

    @Test
    public void failureOfEveryDestinationIsReported() throws Exception {
        try (DatagramSocket socket = new DatagramSocket((SocketAddress) null) {
            @Override public void send(DatagramPacket packet) throws IOException {
                throw new IOException("No route");
            }
        }) {
            WakeOnLanSender.Plan plan = new WakeOnLanSender.Plan(Collections.singletonList(new byte[102]),
                    WakeOnLanSender.destinations(InetAddress.getLoopbackAddress(), 47989), Collections.emptySet());
            assertThrows(IOException.class, () -> WakeOnLanSender.send(plan, socket));
        }
    }

    @Test
    public void transientSendFailureIsRetriedWithoutAddingDuplicateDestinations() throws Exception {
        int[] attempts = {0};
        try (DatagramSocket socket = new DatagramSocket((SocketAddress) null) {
            @Override public void send(DatagramPacket packet) throws IOException {
                if (++attempts[0] == 1) throw new IOException("Temporary route failure");
            }
        }) {
            Set<InetSocketAddress> targets = new LinkedHashSet<>();
            targets.add(new InetSocketAddress(InetAddress.getLoopbackAddress(), 9));
            WakeOnLanSender.send(new WakeOnLanSender.Plan(Collections.singletonList(new byte[102]), targets,
                    Collections.emptySet()), socket);
        }
        assertEquals(3, attempts[0]);
    }
}
