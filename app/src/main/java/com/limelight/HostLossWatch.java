package com.limelight;

import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.RouteInfo;
import android.os.SystemClock;

import com.limelight.nvstream.jni.MoonBridge;

import java.io.IOException;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.UnknownHostException;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.FutureTask;

/**
 * After this session's host said goodbye: tells a PC that shuts down (it stops answering pings)
 * from a host that only ended the stream or restarts, so the first ends with a message at once
 * and a restart reconnects. Runs over both engines; nightjar_core::host_loss decides, from rounds
 * of connection attempts and a ping of the PC, and says why a silence without a goodbye is never
 * decided here. A PC off this device's link, or one that did not answer a ping as the stream
 * started, is left to the old behaviour.
 */
final class HostLossWatch {
    /** nightjar_core::host_loss::ROUND_TIMEOUT and FIRST_PING_TIMEOUT. */
    static final int PING_TIMEOUT_MS = 750;
    static final int FIRST_PING_TIMEOUT_MS = 1000;
    /** nightjar_core::host_loss::GOODBYE_ROUND_GAP. */
    static final long ROUND_GAP_MS = 250;
    /** A round's ping: not asked. */
    static final int NOT_PINGED = -1;
    /** nightjar_core::host_loss::Goodbye's codes. */
    static final int GOODBYE_WAIT = 0;
    static final int GOODBYE_GONE = 1;
    static final int GOODBYE_BACK = 2;
    static final int GOODBYE_UNKNOWN = 3;

    /** The engine and network, replaced in tests. */
    interface Probes {
        /** nightjar_core::host_loss: one round of connection attempts, as its code; blocks for about 750 ms. */
        int round(String address, int port, String gateway);
        /** nightjar_core::host_loss::after_goodbye: one round's verdict, as its code. */
        int afterGoodbye(boolean pingedAtStart, boolean onLink, int round, int pings, boolean closedBefore, long waitedMs);
        /** 1 when the PC answered a ping, 0 when not, NOT_PINGED when this device can't ping it. */
        int ping(String address, int timeoutMs);
        /** This session's host's own disconnect ended the connection, not a silence. */
        boolean saidGoodbye();
        /** This device's gateway while the PC is on its link, else null. */
        String gatewayToward(InetAddress host);
        long now();
        void sleep(long ms) throws InterruptedException;
    }

    private final Probes probes;
    private final InetAddress host;
    private final String literal;
    private final int port;
    private volatile boolean stopped;
    /** The PC answered the ping at the start of the stream: only then can it be told gone. */
    private volatile boolean pingedAtStart;
    private Thread thread;

    /** {@code host}: the address the stream connected to, never looked up again. */
    HostLossWatch(Probes probes, InetAddress host, int port) {
        this.probes = probes;
        this.host = host;
        this.literal = literal(host);
        this.port = port;
    }

    /**
     * The engine's address as text, parsed without DNS: null for anything that is no IP literal (a
     * name would need a lookup, which may give another address or block).
     */
    static InetAddress parse(String address) {
        if (address == null || address.isEmpty()) return null;
        boolean ipv6 = address.indexOf(':') >= 0;
        for (int i = 0; i < address.length(); i++) {
            char c = address.charAt(i);
            boolean allowed = Character.isDigit(c) || c == '.' || (ipv6 && (Character.digit(c, 16) >= 0 || c == ':' || c == '%'));
            if (!allowed) return null;
        }
        try {
            return InetAddress.getByName(address);
        } catch (UnknownHostException e) {
            return null;
        }
    }

    /** Asks the first ping, off the caller's thread. */
    void start() {
        thread = new Thread(this::firstPing, "Host watch");
        thread.setDaemon(true);
        thread.start();
    }

    /** Never waits: a ping or round under way ends on its own and its answer is dropped. */
    void stop() {
        stopped = true;
        if (thread != null) thread.interrupt();
    }

    private void firstPing() {
        // Only a PC on this device's link that answers a ping now can be told gone later.
        boolean pings = probes.gatewayToward(host) != null && probes.ping(literal, FIRST_PING_TIMEOUT_MS) == 1;
        if (stopped) return;
        pingedAtStart = pings;
        LimeLog.info("Host watch: the PC " + (pings ? "answers a ping" : "does not answer a ping or is off this link"));
    }

    /**
     * After the connection ended without a reason: GOODBYE_GONE when this session's host said
     * goodbye and its PC is shutting down or the host did not come back within the restart grace,
     * GOODBYE_BACK when it restarted (the recovery reconnects), and GOODBYE_UNKNOWN for the old
     * behaviour: a silence, a server that still listens, a PC off this link. On the caller's
     * thread, which may wait through the restart grace and a round; stop() ends it early.
     */
    int afterGoodbye() {
        if (stopped || !probes.saidGoodbye()) return GOODBYE_UNKNOWN;
        if (!pingedAtStart) {
            LimeLog.info("Host check after the goodbye: the PC did not answer a ping at the start, nothing to tell");
            return GOODBYE_UNKNOWN;
        }
        long started = probes.now();
        boolean closedBefore = false;
        try {
            while (!stopped) {
                String gateway = probes.gatewayToward(host);
                FutureTask<Integer> ping = null;
                if (gateway != null) {
                    ping = new FutureTask<>(() -> probes.ping(literal, PING_TIMEOUT_MS));
                    Thread pinger = new Thread(ping, "Host ping");
                    pinger.setDaemon(true);
                    pinger.start();
                }
                int round = probes.round(literal, port, gateway);
                int answered = ping == null ? NOT_PINGED : answer(ping);
                if (stopped) break;
                long waited = probes.now() - started;
                int verdict = probes.afterGoodbye(true, gateway != null, round, answered, closedBefore, waited);
                LimeLog.info("Host check after the goodbye, " + waited + " ms in: round " + round + ", ping " + answered +
                        ", verdict " + verdict);
                if (verdict != GOODBYE_WAIT) {
                    if (verdict == GOODBYE_GONE) LimeLog.info("The PC is gone: it said goodbye and its host did not come back");
                    return verdict;
                }
                closedBefore = true;
                probes.sleep(ROUND_GAP_MS);
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        return GOODBYE_UNKNOWN;
    }

    private static int answer(FutureTask<Integer> ping) throws InterruptedException {
        try {
            return ping.get();
        } catch (ExecutionException e) {
            return NOT_PINGED;
        }
    }

    /** The address as text, without a zone: a link-local one then proves nothing, as it should. */
    private static String literal(InetAddress host) {
        String text = host.getHostAddress();
        int zone = text.indexOf('%');
        return zone < 0 ? text : text.substring(0, zone);
    }

    /**
     * The default gateway when {@code host} is on a link of these routes, else null: a route that
     * goes through a gateway, an IPv6 or a tunnel's says nothing about this device's own link.
     */
    static String gatewayToward(Iterable<RouteInfo> routes, InetAddress host) {
        boolean onLink = false;
        InetAddress gateway = null;
        for (RouteInfo route : routes) {
            if (route.isDefaultRoute()) {
                if (route.hasGateway() && route.getGateway() instanceof Inet4Address) gateway = route.getGateway();
            } else if (!route.hasGateway() && route.matches(host)) {
                onLink = true;
            }
        }
        return onLink && gateway != null ? gateway.getHostAddress() : null;
    }

    /**
     * Where this device may not open ICMP datagram sockets: InetAddress.isReachable, which pings when
     * it can and otherwise tries TCP port 7. A PC that drops both then never answered at the start, so
     * it is never told gone. {@code address} is a literal: no lookup.
     */
    static int reachable(String address, int timeoutMs) {
        try {
            return InetAddress.getByName(address).isReachable(timeoutMs) ? 1 : 0;
        } catch (IOException e) {
            return NOT_PINGED;
        }
    }

    /** The real engine and this device's network. */
    static final class Live implements Probes {
        private final ConnectivityManager connectivity;

        Live(ConnectivityManager connectivity) {
            this.connectivity = connectivity;
        }

        @Override public int round(String address, int port, String gateway) {
            return MoonBridge.hostLossRound(address, port, gateway);
        }
        @Override public int afterGoodbye(boolean pingedAtStart, boolean onLink, int round, int pings, boolean closedBefore, long waitedMs) {
            return MoonBridge.hostLossAfterGoodbye(pingedAtStart, onLink, round, pings, closedBefore, waitedMs);
        }
        @Override public int ping(String address, int timeoutMs) {
            int answered = MoonBridge.hostLossPing(address, timeoutMs);
            return answered != NOT_PINGED ? answered : HostLossWatch.reachable(address, timeoutMs);
        }
        @Override public boolean saidGoodbye() { return MoonBridge.hostSaidGoodbye(); }
        @Override public String gatewayToward(InetAddress host) {
            Network network = connectivity.getActiveNetwork();
            if (network == null) return null;
            NetworkCapabilities capabilities = connectivity.getNetworkCapabilities(network);
            LinkProperties link = connectivity.getLinkProperties(network);
            if (capabilities == null || link == null || capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) return null;
            return HostLossWatch.gatewayToward(link.getRoutes(), host);
        }
        @Override public long now() { return SystemClock.elapsedRealtime(); }
        @Override public void sleep(long ms) throws InterruptedException { Thread.sleep(ms); }
    }
}
