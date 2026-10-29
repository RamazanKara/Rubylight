package com.limelight.computers;

import static org.junit.Assert.*;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.nio.ByteBuffer;
import java.util.Arrays;
import java.util.concurrent.atomic.AtomicInteger;

import org.junit.Test;

public class StunClientTest {
    private static final int COOKIE = 0x2112a442;

    /** A binding success carrying 203.0.113.7:4500 as XOR-MAPPED-ADDRESS, after one unknown attribute. */
    private static byte[] success(byte[] transaction) {
        ByteBuffer b = ByteBuffer.allocate(20 + 8 + 12);
        b.putShort((short) 0x0101).putShort((short) 20).putInt(COOKIE).put(transaction);
        b.putShort((short) 0x8022).putShort((short) 3).put(new byte[] {'r', 'l', '!', 0});
        b.putShort((short) 0x0020).putShort((short) 8).put((byte) 0).put((byte) 1);
        b.putShort((short) (4500 ^ (COOKIE >>> 16)));
        b.putInt(((203 << 24) | (113 << 8) | 7) ^ COOKIE);
        return b.array();
    }

    @Test
    public void theMappedAddressComesBackOnlyForThisTransaction() {
        byte[] transaction = new byte[12];
        Arrays.fill(transaction, (byte) 9);
        assertEquals("203.0.113.7", StunClient.decode(success(transaction), transaction));

        byte[] other = transaction.clone();
        other[0] = 1;
        assertNull(StunClient.decode(success(transaction), other));
        byte[] error = success(transaction);
        error[1] = 0x11;
        assertNull("a binding error", StunClient.decode(error, transaction));
        assertNull("truncated", StunClient.decode(Arrays.copyOf(success(transaction), 30), transaction));
    }

    // The query goes out on the socket the caller bound (Network.bindSocket), and resolves on
    // the caller's resolver (Network.getAllByName): nothing else is bound.
    @Test
    public void theQueryUsesTheSocketTheCallerBinds() throws Exception {
        try (DatagramSocket server = new DatagramSocket(0, InetAddress.getLoopbackAddress())) {
            Thread answer = new Thread(() -> {
                try {
                    DatagramPacket request = new DatagramPacket(new byte[64], 64);
                    server.receive(request);
                    ByteBuffer r = ByteBuffer.wrap(request.getData(), 0, request.getLength());
                    assertEquals(20, request.getLength());
                    assertEquals(0x0001, r.getShort());
                    assertEquals(0, r.getShort());
                    assertEquals(COOKIE, r.getInt());
                    byte[] transaction = new byte[12];
                    r.get(transaction);
                    byte[] reply = success(transaction);
                    server.send(new DatagramPacket(reply, reply.length, request.getSocketAddress()));
                } catch (Exception e) {
                    throw new RuntimeException(e);
                }
            });
            answer.start();
            AtomicInteger bound = new AtomicInteger();
            String address = StunClient.query("stun.test", server.getLocalPort(), 3000,
                    host -> {
                        assertEquals("stun.test", host);
                        return new InetAddress[] {InetAddress.getLoopbackAddress()};
                    },
                    socket -> bound.incrementAndGet());
            answer.join(3000);
            assertEquals("203.0.113.7", address);
            assertEquals(1, bound.get());
        }
    }

    @Test
    public void noAnswerIsNoAddress() throws Exception {
        try (DatagramSocket silent = new DatagramSocket(0, InetAddress.getLoopbackAddress())) {
            assertNull(StunClient.query("stun.test", silent.getLocalPort(), 300,
                    host -> new InetAddress[] {InetAddress.getLoopbackAddress()}, socket -> { }));
        }
        assertNull("no IPv4 address", StunClient.query("stun.test", 3478, 300,
                host -> new InetAddress[] {InetAddress.getByName("::1")}, socket -> { }));
    }
}
