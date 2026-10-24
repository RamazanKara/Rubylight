package com.limelight.utils;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.RouteInfo;
import android.os.Build;

import com.limelight.LimeLog;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileReader;
import java.io.IOException;
import java.net.InetAddress;
import java.util.ArrayList;
import java.util.List;
import java.util.regex.Pattern;

/**
 * The speed of this device's own wired port on the route to a host, for the PyroWave pacing hint
 * (x-ss-video[0].pyrowaveLinkMbps). A host on a faster port than its client would otherwise burst
 * frames faster than the client's port takes them. Zero means unknown: Wi-Fi, cellular, a VPN, a
 * route that cannot be told or a port that does not report its speed. A wrong low value would
 * throttle the stream, so Android's own bandwidth estimate for Ethernet (a fixed 100 Mbps on many
 * builds) is never used; only the speed the network interface reports counts.
 */
public final class WiredLink {
    private static final Pattern INTERFACE_NAME = Pattern.compile("[A-Za-z0-9._-]{1,15}");
    private static final int MAX_MBPS = 1_000_000;

    /** A network that can reach the host, with how specific its route to the host is. */
    static final class Route {
        final String interfaceName;
        final boolean ethernet, vpn, active;
        /** Wi-Fi or cellular is among the network's transports; a VPN shows its underlying ones. */
        final boolean wifi, cellular;
        /** Prefix length of the best route to the host on this network; 0 is the default route. */
        final int prefixLength;

        Route(String interfaceName, boolean ethernet, boolean vpn, boolean active, int prefixLength) {
            this(interfaceName, ethernet, vpn, active, prefixLength, false, false);
        }

        Route(String interfaceName, boolean ethernet, boolean vpn, boolean active, int prefixLength,
              boolean wifi, boolean cellular) {
            this.interfaceName = interfaceName;
            this.ethernet = ethernet;
            this.vpn = vpn;
            this.active = active;
            this.prefixLength = prefixLength;
            this.wifi = wifi;
            this.cellular = cellular;
        }
    }

    /** Reads the speed in Mbps an interface reports; zero when it reports none. */
    interface SpeedSource {
        int mbps(String interfaceName);
    }

    private WiredLink() {}

    /** The wired link speed in Mbps toward {@code host}, or 0 when unknown. Blocks on name lookup. */
    public static int mbpsToward(Context context, String host) {
        try {
            InetAddress address = InetAddress.getByName(host);
            return mbpsToward(routesTo(context, address), WiredLink::sysfsMbps);
        } catch (IOException | RuntimeException e) {
            // Unresolvable name, no permission or a connectivity service that is gone: no hint.
            return 0;
        }
    }

    /**
     * Whether the route to {@code host} is Wi-Fi or cellular, for the clientWireless hint
     * (x-rl-video[0].clientWireless) that lets a Rubylight host pace for a wireless link from the
     * first frame. False when it is wired, unknown, a VPN whose underlying transport is not
     * visible, or cannot be determined. Blocks on name lookup.
     */
    public static boolean wirelessToward(Context context, String host) {
        try {
            InetAddress address = InetAddress.getByName(host);
            return wirelessToward(routesTo(context, address));
        } catch (IOException | RuntimeException e) {
            return false;
        }
    }

    static boolean wirelessToward(List<Route> routes) {
        Route best = bestRoute(routes);
        // A VPN counts by the transport underneath it when Android shows one; a network with
        // Ethernet among its transports is never wireless, whatever else it carries.
        return best != null && !best.ethernet && (best.wifi || best.cellular);
    }

    private static Route bestRoute(List<Route> routes) {
        Route best = null;
        for (Route route : routes) {
            if (best == null || route.prefixLength > best.prefixLength ||
                    (route.prefixLength == best.prefixLength && route.active && !best.active)) {
                best = route;
            }
        }
        return best;
    }

    static int mbpsToward(List<Route> routes, SpeedSource speeds) {
        Route best = bestRoute(routes);
        // Only the network the traffic takes counts: a wired port that loses to Wi-Fi or a VPN
        // on the route to this host says nothing about the stream.
        if (best == null || !best.ethernet || best.vpn || best.interfaceName == null ||
                !INTERFACE_NAME.matcher(best.interfaceName).matches()) {
            return 0;
        }
        return speeds.mbps(best.interfaceName);
    }

    @SuppressWarnings("deprecation")
    private static List<Route> routesTo(Context context, InetAddress host) {
        List<Route> routes = new ArrayList<>();
        ConnectivityManager connectivity = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        if (connectivity == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            // RouteInfo cannot be asked about a destination before Android 10, so the routes
            // are unknown and so is the link.
            return routes;
        }
        Network active = connectivity.getActiveNetwork();
        for (Network network : connectivity.getAllNetworks()) {
            NetworkCapabilities capabilities = connectivity.getNetworkCapabilities(network);
            LinkProperties properties = connectivity.getLinkProperties(network);
            if (capabilities == null || properties == null) {
                continue;
            }
            int prefix = -1;
            for (RouteInfo route : properties.getRoutes()) {
                if (route.matches(host)) {
                    prefix = Math.max(prefix, route.getDestination().getPrefixLength());
                }
            }
            if (prefix >= 0) {
                routes.add(new Route(properties.getInterfaceName(),
                        capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET),
                        capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN) ||
                                !capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN),
                        network.equals(active), prefix,
                        capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI),
                        capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)));
            }
        }
        return routes;
    }

    private static int sysfsMbps(String interfaceName) {
        File file = new File("/sys/class/net/" + interfaceName + "/speed");
        try (BufferedReader reader = new BufferedReader(new FileReader(file))) {
            int mbps = parseMbps(reader.readLine());
            if (mbps > 0) {
                LimeLog.info("Wired link toward the host: " + mbps + " Mbps");
            }
            return mbps;
        } catch (IOException | SecurityException e) {
            // Unreadable (SELinux), or the port is down: it reports no speed.
            return 0;
        }
    }

    /** The speed in Mbps from the interface's speed file; "-1" (unknown) and nonsense give 0. */
    static int parseMbps(String text) {
        if (text == null) {
            return 0;
        }
        try {
            int mbps = Integer.parseInt(text.trim());
            return mbps > 0 && mbps <= MAX_MBPS ? mbps : 0;
        } catch (NumberFormatException e) {
            return 0;
        }
    }
}
