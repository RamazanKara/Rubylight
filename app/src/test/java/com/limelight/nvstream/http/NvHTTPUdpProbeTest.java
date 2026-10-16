package com.limelight.nvstream.http;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import com.limelight.nvstream.PyroWavePace;

import org.junit.Test;

import java.io.IOException;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Map;
import java.util.Set;

import okhttp3.Headers;

/** The UDP handshake and receiver against a local stand-in for Vibeshine's /pyrowave-udp-probe. */
public class NvHTTPUdpProbeTest {
    private static final InetAddress LOOPBACK = InetAddress.getLoopbackAddress();
    private static final int KBPS = 5000, PACKET = 1392, FPS = 60, PACE = 50000;

    private static Map<String, String> query(String query) {
        Map<String, String> args = new HashMap<>();
        for (String pair : query.split("&")) {
            int eq = pair.indexOf('=');
            args.put(pair.substring(0, eq), pair.substring(eq + 1));
        }
        return args;
    }

    /** Sends the probe's packets the way the host does once the token arrived, skipping drop. */
    private static NvHTTP.ProbeRequest host(Set<Integer> drop, boolean strangerTraffic) {
        return (query, onHeaders) -> {
            Map<String, String> args = query(query);
            assertEquals("1", args.get("handshake"));
            assertEquals(String.valueOf(FPS), args.get("fps"));
            assertEquals(String.valueOf(PACE), args.get("pacekbps"));
            byte[] token = args.get("token").getBytes(StandardCharsets.US_ASCII);
            assertEquals(32, token.length);
            InetSocketAddress client = new InetSocketAddress(LOOPBACK, Integer.parseInt(args.get("port")));
            long expected = PyroWavePace.expectedPackets(Integer.parseInt(args.get("kbps")), Integer.parseInt(args.get("packetsize")));
            long sent = 0;
            try (DatagramSocket socket = new DatagramSocket(new InetSocketAddress(LOOPBACK, 0));
                 DatagramSocket stranger = new DatagramSocket(new InetSocketAddress(LOOPBACK, 0))) {
                socket.setSoTimeout(1500);
                onHeaders.onHeaders(Headers.of("X-PyroWave-Udp-Port", String.valueOf(socket.getLocalPort())));
                byte[] hello = new byte[64];
                DatagramPacket in = new DatagramPacket(hello, hello.length);
                socket.receive(in);
                assertEquals(32, in.getLength());
                byte[] packet = new byte[PACKET + PyroWavePace.PROBE_EXTRA_BYTES];
                System.arraycopy(token, 0, packet, 0, 32);
                for (int seq = 0; seq < expected; seq++) {
                    packet[32] = (byte) (seq >>> 24);
                    packet[33] = (byte) (seq >>> 16);
                    packet[34] = (byte) (seq >>> 8);
                    packet[35] = (byte) seq;
                    sent++;
                    if (strangerTraffic) {
                        // Right token and size from another port: a connected receiver never sees it.
                        stranger.send(new DatagramPacket(packet, packet.length, client));
                    }
                    if (!drop.contains(seq)) {
                        socket.send(new DatagramPacket(packet, packet.length, client));
                    }
                    if (seq % 64 == 0) Thread.sleep(1);
                }
            } catch (InterruptedException e) {
                throw new IOException(e);
            }
            return "<root status_code=\"200\"><expected>" + expected + "</expected><sent>" + sent +
                    "</sent><elapsedMs>2000.4</elapsedMs><sendRetries>0</sendRetries><lastSendError>0</lastSendError></root>";
        };
    }

    @Test
    public void completeProbeArrivesAndQualifies() throws Exception {
        PyroWavePace.Result result = NvHTTP.runUdpProbe(LOOPBACK, KBPS, PACKET, FPS, PACE, host(Set.of(), false));
        assertEquals(PyroWavePace.expectedPackets(KBPS, PACKET), result.expected);
        assertEquals(result.expected, result.received);
        assertEquals(120, result.frames);
        assertTrue(PyroWavePace.qualified(result));
    }

    @Test
    public void lostFrameTailsAreCounted() throws Exception {
        long expected = PyroWavePace.expectedPackets(KBPS, PACKET);
        // The last packet of the first ten frames.
        java.util.HashSet<Integer> drop = new java.util.HashSet<>();
        for (int frame = 0; frame < 10; frame++) {
            drop.add((int) (expected * (frame + 1) / 120 - 1));
        }
        PyroWavePace.Result result = NvHTTP.runUdpProbe(LOOPBACK, KBPS, PACKET, FPS, PACE, host(drop, false));
        assertEquals(expected - 10, result.received);
        assertEquals(10, result.missingTailFrames);
        assertFalse(PyroWavePace.qualified(result));
    }

    @Test
    public void packetsFromAnotherPortAreIgnored() throws Exception {
        PyroWavePace.Result result = NvHTTP.runUdpProbe(LOOPBACK, KBPS, PACKET, FPS, PACE,
                host(Set.of(0, 1, 2, 3, 4, 5, 6, 7, 8, 9), true));
        assertEquals(result.expected - 10, result.received);
    }

    @Test
    public void hostWithoutAnAnnouncedPortFails() {
        assertThrows(IOException.class, () -> NvHTTP.runUdpProbe(LOOPBACK, KBPS, PACKET, FPS, PACE,
                (query, onHeaders) -> {
                    onHeaders.onHeaders(Headers.of());
                    return "<root status_code=\"200\"/>";
                }));
    }

    @Test
    public void mismatchedPacketCountIsRejected() {
        assertThrows(IOException.class, () -> NvHTTP.runUdpProbe(LOOPBACK, KBPS, PACKET, FPS, PACE,
                (query, onHeaders) -> {
                    host(Set.of(), false).send(query, onHeaders);
                    return "<root status_code=\"200\"><expected>1</expected><sent>1</sent><elapsedMs>2000</elapsedMs></root>";
                }));
    }
}
