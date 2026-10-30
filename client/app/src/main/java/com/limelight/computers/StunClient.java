package com.limelight.computers;

import java.io.IOException;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.SocketTimeoutException;
import java.net.UnknownHostException;
import java.nio.ByteBuffer;
import java.security.SecureRandom;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * IPv4 STUN binding discovery on a socket the caller binds, so the query can go out on one chosen
 * network (Network.bindSocket) without moving the process's other sockets, such as a running
 * stream, off the VPN. Same wire form as the native LiFindExternalAddressIP4 (android_stun.c).
 * Ported from Moonlight V+'s StunClient.kt (qiin2333/moonlight-vplus).
 */
final class StunClient {
    private static final int COOKIE = 0x2112a442;
    private static final int HEADER_SIZE = 20;
    private static final int BINDING_REQUEST = 0x0001;
    private static final int BINDING_SUCCESS = 0x0101;
    private static final int XOR_MAPPED_ADDRESS = 0x0020;

    interface Resolver {
        InetAddress[] resolve(String host) throws UnknownHostException;
    }

    interface SocketBinder {
        void bind(DatagramSocket socket) throws IOException;
    }

    private StunClient() {
    }

    /**
     * The WAN IPv4 address the STUN server sees, or null without an answer before the timeout.
     * The request goes to up to three IPv4 addresses of the server, again every second.
     */
    static String query(String hostname, int port, long timeoutMs, Resolver resolver, SocketBinder binder)
            throws IOException {
        long deadline = System.nanoTime() + timeoutMs * 1_000_000L;
        List<InetAddress> servers = new ArrayList<>();
        for (InetAddress address : resolver.resolve(hostname)) {
            if (address instanceof Inet4Address && servers.size() < 3) servers.add(address);
        }
        if (servers.isEmpty()) return null;
        byte[] transaction = new byte[12];
        new SecureRandom().nextBytes(transaction);
        byte[] request = ByteBuffer.allocate(HEADER_SIZE).putShort((short) BINDING_REQUEST).putShort((short) 0)
                .putInt(COOKIE).put(transaction).array();
        try (DatagramSocket socket = new DatagramSocket()) {
            binder.bind(socket);
            long nextSend = System.nanoTime();
            byte[] buffer = new byte[1024];
            while (System.nanoTime() < deadline) {
                long now = System.nanoTime();
                if (now >= nextSend) {
                    for (InetAddress server : servers) {
                        socket.send(new DatagramPacket(request, request.length, server, port));
                    }
                    nextSend = now + 1_000_000_000L;
                }
                long left = (deadline - System.nanoTime()) / 1_000_000L;
                socket.setSoTimeout((int) Math.max(1, Math.min(200, left)));
                DatagramPacket response = new DatagramPacket(buffer, buffer.length);
                try {
                    socket.receive(response);
                } catch (SocketTimeoutException e) {
                    continue;
                }
                if (response.getPort() != port || !servers.contains(response.getAddress())) continue;
                String address = decode(Arrays.copyOf(buffer, response.getLength()), transaction);
                if (address != null) return address;
            }
        }
        return null;
    }

    /** The XOR-MAPPED-ADDRESS of a binding success for this transaction, else null. */
    static String decode(byte[] data, byte[] transaction) {
        if (data.length < HEADER_SIZE || transaction.length != 12) return null;
        ByteBuffer buffer = ByteBuffer.wrap(data);
        if ((buffer.getShort() & 0xffff) != BINDING_SUCCESS) return null;
        int length = buffer.getShort() & 0xffff;
        if (length % 4 != 0 || length > data.length - HEADER_SIZE || buffer.getInt() != COOKIE) return null;
        byte[] received = new byte[12];
        buffer.get(received);
        if (!Arrays.equals(received, transaction)) return null;
        int end = HEADER_SIZE + length;
        while (buffer.position() < end) {
            if (end - buffer.position() < 4) return null;
            int type = buffer.getShort() & 0xffff;
            int size = buffer.getShort() & 0xffff;
            int padded = (size + 3) & -4;
            if (padded > end - buffer.position()) return null;
            if (type == XOR_MAPPED_ADDRESS && size == 8) {
                buffer.get();
                if (buffer.get() != 1) return null;
                buffer.getShort();
                int address = buffer.getInt() ^ COOKIE;
                try {
                    return InetAddress.getByAddress(ByteBuffer.allocate(4).putInt(address).array()).getHostAddress();
                } catch (UnknownHostException e) {
                    return null;
                }
            }
            buffer.position(buffer.position() + padded);
        }
        return null;
    }
}
