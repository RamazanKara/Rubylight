package com.limelight.nvstream.wol;

import android.content.Context;

import java.io.IOException;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.InterfaceAddress;
import java.net.NetworkInterface;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Enumeration;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;

import com.limelight.LimeLog;
import com.limelight.nvstream.http.ComputerDetails;

public class WakeOnLanSender {
    // Echo and discard, where magic packets classically go, and the port Moonlight hosts listen on.
    private static final int[] STATIC_PORTS_TO_TRY = {7, 9, 47009};
    private static final int[] DYNAMIC_PORTS_TO_TRY = {47998, 47999, 48000, 48002, 48010};
    // ff02::1, every node on the link: an IPv6-only network has no broadcast.
    private static final byte[] ALL_NODES = {(byte) 0xff, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1};

    /** What one wake sends: each payload (one per MAC) to every destination, and to the best-effort ones. */
    static final class Plan {
        final List<byte[]> payloads;
        final Set<InetSocketAddress> destinations;
        /** IPv6 all-nodes on each link; sends there may all fail without failing the wake. */
        final Set<InetSocketAddress> bestEffort;

        Plan(List<byte[]> payloads, Set<InetSocketAddress> destinations, Set<InetSocketAddress> bestEffort) {
            this.payloads = payloads;
            this.destinations = destinations;
            this.bestEffort = bestEffort;
        }
    }

    static Set<InetSocketAddress> destinations(InetAddress address, int httpPort) {
        Set<InetSocketAddress> destinations = new LinkedHashSet<>();
        for (int port : STATIC_PORTS_TO_TRY) {
            destinations.add(new InetSocketAddress(address, port));
        }
        for (int port : DYNAMIC_PORTS_TO_TRY) {
            int mappedPort = port - 47989 + httpPort;
            if (mappedPort > 0 && mappedPort <= 65535) {
                destinations.add(new InetSocketAddress(address, mappedPort));
            }
        }
        return destinations;
    }

    /**
     * The packets for the PC: one payload for serverinfo's MAC and each MAC its {@code /wake} listed, to the PC's
     * addresses, to 255.255.255.255, to the subnet broadcasts of this device and of the PC, and to {@code ff02::1}.
     * Resolving the PC's addresses may block on DNS.
     */
    static Plan plan(ComputerDetails computer, WakeTargets.Targets extra, Set<InetAddress> localBroadcasts,
                     Set<InetAddress> allNodes) throws IOException {
        Set<String> macs = new LinkedHashSet<>();
        if (computer.macAddress != null) macs.add(computer.macAddress.replace('-', ':').toLowerCase(Locale.ROOT));
        macs.addAll(extra.macs);
        List<byte[]> payloads = new ArrayList<>();
        IOException invalid = null;
        for (String mac : macs) {
            try {
                payloads.add(createWolPayload(mac));
            } catch (IOException e) {
                invalid = e;
            }
        }
        if (payloads.isEmpty()) {
            throw invalid != null ? invalid : new IOException("No Wake-on-LAN MAC address");
        }

        Set<InetSocketAddress> targets = new LinkedHashSet<>();
        Set<Integer> ports = new LinkedHashSet<>();
        for (ComputerDetails.AddressTuple address : new ComputerDetails.AddressTuple[] {
                computer.localAddress, computer.remoteAddress, computer.manualAddress, computer.ipv6Address,
        }) {
            if (address == null) continue;
            ports.add(address.port);
            try {
                for (InetAddress resolvedAddress : InetAddress.getAllByName(address.address)) {
                    targets.addAll(destinations(resolvedAddress, address.port));
                }
            } catch (IOException e) {
                LimeLog.info("Wake-on-LAN skips " + address + ": " + e.getMessage());
            }
        }
        if (ports.isEmpty()) ports.add(47989);
        Set<InetAddress> broadcasts = new LinkedHashSet<>();
        broadcasts.add(InetAddress.getByAddress(new byte[] {(byte) 255, (byte) 255, (byte) 255, (byte) 255}));
        broadcasts.addAll(localBroadcasts);
        for (String broadcast : extra.broadcasts) {
            byte[] address = WakeTargets.ipv4(broadcast);
            if (address != null) broadcasts.add(InetAddress.getByAddress(address));
        }
        // Directed broadcasts reach each subnet and avoid stale ARP entries for sleeping PCs.
        for (InetAddress broadcast : broadcasts) {
            for (int port : ports) targets.addAll(destinations(broadcast, port));
        }
        Set<InetSocketAddress> bestEffort = new LinkedHashSet<>();
        for (InetAddress link : allNodes) {
            for (int port : ports) bestEffort.addAll(destinations(link, port));
        }
        return new Plan(payloads, targets, bestEffort);
    }

    static void send(Plan plan, DatagramSocket sock) throws IOException {
        IOException lastException = null;
        boolean sentWolPacket = false;
        int bestEffortFailures = 0;
        // A magic packet has no acknowledgement; a short burst tolerates an isolated UDP loss.
        for (int attempt = 0; attempt < 3; attempt++) {
            for (byte[] payload : plan.payloads) {
                for (InetSocketAddress destination : plan.destinations) {
                    try {
                        sock.send(new DatagramPacket(payload, payload.length, destination));
                        sentWolPacket = true;
                    } catch (IOException e) {
                        lastException = e;
                    }
                }
                for (InetSocketAddress destination : plan.bestEffort) {
                    try {
                        sock.send(new DatagramPacket(payload, payload.length, destination));
                    } catch (IOException e) {
                        bestEffortFailures++;
                    }
                }
            }
        }
        if (bestEffortFailures > 0) {
            LimeLog.info("Wake-on-LAN: " + bestEffortFailures + " IPv6 all-nodes sends failed");
        }
        if (!sentWolPacket) {
            throw lastException != null ? lastException : new IOException("No Wake-on-LAN destinations");
        }
    }

    /**
     * Sends the magic packets for the PC (blocks): see {@link #plan}. Throws when no packet could be sent at all, or
     * when the PC has no usable MAC address.
     */
    public static void sendWolPacket(Context context, ComputerDetails computer) throws IOException {
        WakeTargets.Targets extra = new WakeTargets(context).load(computer.uuid);
        Set<InetAddress> broadcasts = new LinkedHashSet<>();
        Set<InetAddress> allNodes = new LinkedHashSet<>();
        try {
            Enumeration<NetworkInterface> interfaces = NetworkInterface.getNetworkInterfaces();
            for (NetworkInterface nic : interfaces == null ? Collections.<NetworkInterface>emptyList() : Collections.list(interfaces)) {
                if (!nic.isUp() || nic.isLoopback()) continue;
                boolean ipv6 = false;
                for (InterfaceAddress address : nic.getInterfaceAddresses()) {
                    if (address.getBroadcast() instanceof Inet4Address) {
                        broadcasts.add(address.getBroadcast());
                    }
                    ipv6 |= address.getAddress() instanceof Inet6Address;
                }
                if (ipv6 && nic.supportsMulticast()) {
                    allNodes.add(Inet6Address.getByAddress(null, ALL_NODES, nic));
                }
            }
        } catch (IOException | SecurityException e) {
            // Restricted interface enumeration must not prevent ordinary broadcast/unicast wake.
            LimeLog.warning("Wake-on-LAN cannot list the network interfaces: " + e);
        }
        Plan plan = plan(computer, extra, broadcasts, allNodes);
        try (DatagramSocket sock = new DatagramSocket(0)) {
            sock.setBroadcast(true);
            send(plan, sock);
        }
    }

    static byte[] createWolPayload(String macAddress) throws IOException {
        if (macAddress == null || !macAddress.matches("(?i)[0-9a-f]{2}([:-])[0-9a-f]{2}(\\1[0-9a-f]{2}){4}")) {
            throw new IOException("Invalid Wake-on-LAN MAC address");
        }
        byte[] macBytes = new byte[6];
        String[] octets = macAddress.split("[:-]");
        for (int i = 0; i < macBytes.length; i++) {
            macBytes[i] = (byte) Integer.parseInt(octets[i], 16);
        }
        byte[] payload = new byte[102];
        for (int i = 0; i < 6; i++) payload[i] = (byte) 0xFF;
        for (int i = 6; i < payload.length; i += 6) {
            System.arraycopy(macBytes, 0, payload, i, macBytes.length);
        }
        return payload;
    }
}
