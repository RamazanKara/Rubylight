package com.limelight;

import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.RouteInfo;

import com.limelight.nvstream.jni.MoonBridge;

import java.io.IOException;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.UnknownHostException;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.FutureTask;
import java.util.function.BooleanSupplier;

/**
 * Ends a stream as soon as its PC is gone (shut down, lost power), instead of holding a frozen
 * picture until the connection's timeout. Runs over both engines; nightjar_core::host_loss decides,
 * with connection attempts and a ping of the PC during a silence of the control stream, and says
 * why a dropout, a stalled host or a host that restarts never counts. Does nothing while the host
 * is heard.
 */
final class HostLossWatch {
    static final long TICK_MS = 250;
    /** nightjar_core::host_loss::PROBE_AFTER: below it a step does nothing, so nothing is looked up for it. */
    private static final long PROBE_AFTER_MS = 2000;
    /** nightjar_core::host_loss::ROUND_TIMEOUT and FIRST_PING_TIMEOUT. */
    static final int PING_TIMEOUT_MS = 750;
    static final int FIRST_PING_TIMEOUT_MS = 1000;
    /** A round's ping: not asked. */
    static final int NOT_PINGED = -1;
    /** nightjar_core::host_loss::Round::Answered's code: the PC accepted a connection. */
    static final int ROUND_ANSWERED = 0;

    /** The engine and network, replaced in tests. */
    interface Probes {
        long silenceMs();
        long mediaBytes();
        /** nightjar_core::host_loss: one round of connection attempts, as its code; blocks for about 750 ms. */
        int round(String address, int port, String gateway);
        /** The detector's next state after a round, -1 when the PC is gone. */
        int decide(long silenceMs, int round, int pings, int state);
        /** The detector's first state, from the ping at the start. */
        int firstState(boolean pings);
        /** 1 when the PC answered a ping, 0 when not, NOT_PINGED when this device can't ping it. */
        int ping(String address, int timeoutMs);
        /** The connection ended with the host's disconnect, not a silence. */
        boolean saidGoodbye();
        /** This device's gateway while the PC is on its link, else null. */
        String gatewayToward(InetAddress host);
        boolean networkLost();
        void endStream();
        InetAddress resolve(String address) throws UnknownHostException;
        void sleep(long ms) throws InterruptedException;
    }

    private final Probes probes;
    private final String address;
    private final int port;
    private volatile boolean stopped;
    private Thread thread;

    HostLossWatch(Probes probes, String address, int port) {
        this.probes = probes;
        this.address = address;
        this.port = port;
    }

    void start() {
        thread = new Thread(this::run, "Host watch");
        thread.setDaemon(true);
        thread.start();
    }

    /** Never waits: a round under way ends on its own and its answer is dropped. */
    void stop() {
        stopped = true;
        if (thread != null) thread.interrupt();
    }

    private void run() {
        InetAddress host;
        try {
            host = probes.resolve(address);
        } catch (UnknownHostException e) {
            LimeLog.warning("A PC that is gone is noticed only by the connection's timeout: " + e);
            return;
        }
        String literal = literal(host);
        // Only a PC on this device's link that answers a ping now can be told gone later.
        boolean pings = probes.gatewayToward(host) != null && probes.ping(literal, FIRST_PING_TIMEOUT_MS) == 1;
        LimeLog.info("Host watch: the PC " + (pings ? "answers a ping" : "does not answer a ping or is off this link"));
        final int first = probes.firstState(pings);
        int state = first;
        long bytes = probes.mediaBytes();
        try {
            while (!stopped) {
                probes.sleep(TICK_MS);
                long silence = probes.silenceMs();
                long seen = probes.mediaBytes();
                // Media still arriving is a host too, and a device without a network can't tell anything.
                boolean heard = seen != bytes || probes.networkLost();
                bytes = seen;
                if (heard || silence < PROBE_AFTER_MS) {
                    state = first;
                    continue;
                }
                String gateway = probes.gatewayToward(host);
                FutureTask<Integer> ping = null;
                if (pings && gateway != null) {
                    ping = new FutureTask<>(() -> probes.ping(literal, PING_TIMEOUT_MS));
                    Thread pinger = new Thread(ping, "Host ping");
                    pinger.setDaemon(true);
                    pinger.start();
                }
                int round = probes.round(literal, port, gateway);
                int answered = ping == null ? NOT_PINGED : answer(ping);
                long after = probes.silenceMs();
                state = probes.decide(Math.max(silence, after), round, answered, state);
                LimeLog.info("Host check after " + after + " ms of silence: round " + round + ", ping " + answered);
                if (state >= 0) continue;
                // The round took a while: the host may have been heard meanwhile.
                if (stopped || probes.silenceMs() < Math.max(silence, after) || probes.mediaBytes() != bytes) {
                    state = first;
                    continue;
                }
                LimeLog.info("The PC is gone: nothing from it for " + after + " ms and it stopped answering pings");
                probes.endStream();
                return;
            }
        } catch (InterruptedException e) {
            // Stopped.
        }
    }

    private static int answer(FutureTask<Integer> ping) throws InterruptedException {
        try {
            return ping.get();
        } catch (ExecutionException e) {
            return NOT_PINGED;
        }
    }

    /**
     * After the connection ended without a reason: whether the host disconnected and its server went
     * with it (shutting down), so there is nothing to recover. A host that went silent is left to the
     * recovery, and one that still accepts connections only ended this stream. On the caller's
     * thread, which may wait for one round (about 750 ms).
     */
    boolean goneAfterGoodbye() {
        if (stopped || !probes.saidGoodbye()) return false;
        InetAddress host;
        try {
            host = probes.resolve(address);
        } catch (UnknownHostException e) {
            return false;
        }
        boolean gone = probes.round(literal(host), port, probes.gatewayToward(host)) != ROUND_ANSWERED;
        if (gone) LimeLog.info("The PC is gone: it disconnected and its server went with it");
        return gone;
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
     * it is never told gone.
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
        private final BooleanSupplier networkLost;

        Live(ConnectivityManager connectivity, BooleanSupplier networkLost) {
            this.connectivity = connectivity;
            this.networkLost = networkLost;
        }

        @Override public long silenceMs() { return MoonBridge.getControlSilenceMs(); }
        @Override public long mediaBytes() { return MoonBridge.getWireBytesReceived(); }
        @Override public int round(String address, int port, String gateway) {
            return MoonBridge.hostLossRound(address, port, gateway);
        }
        @Override public int decide(long silenceMs, int round, int pings, int state) {
            return MoonBridge.hostLossDecide(silenceMs, round, pings, state);
        }
        @Override public int firstState(boolean pings) { return MoonBridge.hostLossFirstState(pings); }
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
        @Override public boolean networkLost() { return networkLost.getAsBoolean(); }
        @Override public void endStream() { MoonBridge.endStreamHostGone(); }
        @Override public InetAddress resolve(String address) throws UnknownHostException { return InetAddress.getByName(address); }
        @Override public void sleep(long ms) throws InterruptedException { Thread.sleep(ms); }
    }
}
