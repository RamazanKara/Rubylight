package com.limelight;

import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.RouteInfo;
import android.os.SystemClock;

import com.limelight.nvstream.jni.MoonBridge;

import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.UnknownHostException;

/**
 * After this session's host said goodbye: tells a PC that shut down or stopped its host (its server
 * closes) from a host that only ended the stream, so the first ends with a message at once. Runs
 * over both engines; nightjar_core::host_loss decides, from rounds of connection attempts, and says
 * why a silence without a goodbye is never decided here. A PC off this device's link is left to the
 * old behaviour.
 */
final class HostLossWatch {
    /** nightjar_core::host_loss::GOODBYE_ROUND_GAP. */
    static final long ROUND_GAP_MS = 250;
    /** nightjar_core::host_loss::Goodbye's codes. */
    static final int GOODBYE_WAIT = 0;
    static final int GOODBYE_GONE = 1;
    static final int GOODBYE_UNKNOWN = 2;

    /** The engine and network, replaced in tests. */
    interface Probes {
        /** nightjar_core::host_loss: one round of connection attempts, as its code; blocks for about 750 ms. */
        int round(String address, int port, String gateway);
        /** nightjar_core::host_loss::after_goodbye: one round's verdict, as its code. */
        int afterGoodbye(boolean onLink, int round, long waitedMs);
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

    /** Never waits: a round under way ends on its own and its answer is dropped. */
    void stop() {
        stopped = true;
    }

    /**
     * After the connection ended without a reason: GOODBYE_GONE when this session's host said
     * goodbye and its server closed (the PC shuts down, or its host stopped), and GOODBYE_UNKNOWN for
     * the old behaviour: a silence, a server that still listens, a PC off this link. On the caller's
     * thread, which may wait through a host's exit window (about 2 s, 4 s while this device's link is
     * quiet); stop() ends it early.
     */
    int afterGoodbye() {
        if (stopped || !probes.saidGoodbye()) return GOODBYE_UNKNOWN;
        long started = probes.now();
        try {
            while (!stopped) {
                String gateway = probes.gatewayToward(host);
                if (gateway == null) {
                    LimeLog.info("Host check after the goodbye: the PC is not on this device's link");
                    return GOODBYE_UNKNOWN;
                }
                int round = probes.round(literal, port, gateway);
                if (stopped) break;
                long waited = probes.now() - started;
                int verdict = probes.afterGoodbye(true, round, waited);
                LimeLog.info("Host check after the goodbye, " + waited + " ms in: round " + round + ", verdict " + verdict);
                if (verdict != GOODBYE_WAIT) {
                    if (verdict == GOODBYE_GONE) LimeLog.info("The PC is gone: it said goodbye and closed its server");
                    return verdict;
                }
                probes.sleep(ROUND_GAP_MS);
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        return GOODBYE_UNKNOWN;
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

    /** The real engine and this device's network. */
    static final class Live implements Probes {
        private final ConnectivityManager connectivity;

        Live(ConnectivityManager connectivity) {
            this.connectivity = connectivity;
        }

        @Override public int round(String address, int port, String gateway) {
            return MoonBridge.hostLossRound(address, port, gateway);
        }
        @Override public int afterGoodbye(boolean onLink, int round, long waitedMs) {
            return MoonBridge.hostLossAfterGoodbye(onLink, round, waitedMs);
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
