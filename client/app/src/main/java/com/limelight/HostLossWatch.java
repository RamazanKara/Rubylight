package com.limelight;

import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.RouteInfo;

import com.limelight.nvstream.jni.MoonBridge;

import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.UnknownHostException;
import java.util.function.BooleanSupplier;

/**
 * Ends a stream as soon as its PC is gone (shut down, crashed), instead of holding a frozen picture
 * until the connection's timeout. Runs over both engines; nightjar_core::host_loss decides, with
 * connection attempts during a silence of the control stream, and says why a dropout or a stalled
 * host never counts. Does nothing while the host is heard.
 */
final class HostLossWatch {
    static final long TICK_MS = 250;
    /** nightjar_core::host_loss::PROBE_AFTER: below it a step does nothing, so nothing is looked up for it. */
    private static final long PROBE_AFTER_MS = 2000;
    /** After the connection ended by itself there is no silence to read: long enough for every verdict. */
    private static final long ENDED_SILENCE_MS = 60_000;
    /** Rounds a check after the connection ended may take: a verdict needs three. */
    private static final int CONFIRM_ROUNDS = 4;

    /** The engine and network, replaced in tests. */
    interface Probes {
        long silenceMs();
        long mediaBytes();
        /** nightjar_core::host_loss::Detector's next state, -1 when the PC is gone; blocks for one round. */
        int step(long silenceMs, String address, int port, String gateway, int state);
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
        int state = 0;
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
                    state = 0;
                    continue;
                }
                state = probes.step(silence, literal, port, probes.gatewayToward(host), state);
                if (state >= 0) continue;
                // The round took a while: the host may have been heard meanwhile.
                if (stopped || probes.silenceMs() < silence || probes.mediaBytes() != bytes) {
                    state = 0;
                    continue;
                }
                LimeLog.info("The PC is gone: nothing from it for " + silence + " ms and it does not answer");
                probes.endStream();
                return;
            }
        } catch (InterruptedException e) {
            // Stopped.
        }
    }

    /**
     * After the connection ended without a reason: whether that was the PC going away. It proves
     * nothing on its own (a PC that answers just closed this stream). On the caller's thread, which
     * may wait for about two seconds.
     */
    boolean confirmGone() {
        InetAddress host;
        try {
            host = probes.resolve(address);
        } catch (UnknownHostException e) {
            return false;
        }
        String literal = literal(host);
        int state = 0;
        try {
            for (int round = 0; round < CONFIRM_ROUNDS && !stopped; round++) {
                state = probes.step(ENDED_SILENCE_MS, literal, port, probes.gatewayToward(host), state);
                if (state < 0) {
                    LimeLog.info("The PC is gone: the connection ended and it does not answer");
                    return true;
                }
                if (state == 0) return false;
                probes.sleep(TICK_MS);
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        return false;
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
        private final BooleanSupplier networkLost;

        Live(ConnectivityManager connectivity, BooleanSupplier networkLost) {
            this.connectivity = connectivity;
            this.networkLost = networkLost;
        }

        @Override public long silenceMs() { return MoonBridge.getControlSilenceMs(); }
        @Override public long mediaBytes() { return MoonBridge.getWireBytesReceived(); }
        @Override public int step(long silenceMs, String address, int port, String gateway, int state) {
            return MoonBridge.hostLossStep(silenceMs, address, port, gateway, state);
        }
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
