package com.limelight.nvstream;

import android.annotation.TargetApi;
import android.content.Context;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.Build;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;

/**
 * While a stream runs, follows the default network. It tells the engine about each
 * default-network change and whether the new network is metered: with client roaming
 * (Experimental) on both sides the engine can move the stream (docs/protocol/client-roaming.md),
 * and never moves onto a network not reported here; otherwise it only answers whether the
 * connection carries on. It tells the listener when the stream's network is lost and when one
 * returns, so a stream whose connection cannot carry on reconnects at once.
 * The callbacks run on ConnectivityManager's thread, never the stream's.
 */
@TargetApi(Build.VERSION_CODES.N)
final class RoamingNetworkWatch {
    private final ConnectivityManager connMgr;
    private final RoamingNetworkTracker tracker;
    private final ConnectivityManager.NetworkCallback callback;

    private RoamingNetworkWatch(ConnectivityManager connMgr, Network streamNetwork,
                                NvConnectionListener listener) {
        this.connMgr = connMgr;
        tracker = new RoamingNetworkTracker(streamNetwork, new RoamingNetworkTracker.Reporter() {
            @Override
            public int networkChanged(boolean metered) {
                // Without roaming on both sides the engine only tells whether the connection
                // carries on; the C engine cannot tell, and carries on.
                int change = MoonBridge.networkChanged(metered);
                LimeLog.info("Roaming: the default network changed (" + (metered ? "metered" : "not metered") +
                        "), " + (change == MoonBridge.NETWORK_ROAMED ? "roaming" :
                        change == MoonBridge.NETWORK_CANNOT_CONTINUE ? "not roaming, the connection cannot carry on" :
                        "not roaming, the connection carries on"));
                return change;
            }

            @Override
            public void streamNetworkLost() {
                LimeLog.warning("The stream's network was lost");
                listener.streamNetworkLost();
            }

            @Override
            public void streamNetworkReturned(int change, boolean metered) {
                listener.streamNetworkReturned(change, metered);
            }
        });
        callback = new ConnectivityManager.NetworkCallback() {
            @Override
            public void onAvailable(Network network) {
                // From Android 8 onCapabilitiesChanged always follows; before that, ask.
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
                    NetworkCapabilities caps = connMgr.getNetworkCapabilities(network);
                    if (caps != null) {
                        tracker.capabilities(network, caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED));
                    }
                }
            }

            @Override
            public void onCapabilitiesChanged(Network network, NetworkCapabilities caps) {
                tracker.capabilities(network, caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED));
            }

            @Override
            public void onLost(Network network) {
                tracker.lost(network);
            }
        };
    }

    /** The default network now, to pass to start() as the one the stream starts on; may be null. */
    static Network defaultNetwork(Context context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) {
            return null;
        }
        ConnectivityManager connMgr = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        return connMgr == null ? null : connMgr.getActiveNetwork();
    }

    /**
     * Starts reporting network changes, or returns null (with a warning) where it cannot: then
     * the stream does not roam onto another network.
     */
    static RoamingNetworkWatch start(Context context, Network streamNetwork, NvConnectionListener listener) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) {
            LimeLog.warning("Roaming: needs Android 7 to follow network changes; not roaming");
            return null;
        }
        ConnectivityManager connMgr = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        if (connMgr == null) {
            LimeLog.warning("Roaming: no ConnectivityManager; not roaming");
            return null;
        }
        RoamingNetworkWatch watch = new RoamingNetworkWatch(connMgr, streamNetwork, listener);
        try {
            connMgr.registerDefaultNetworkCallback(watch.callback);
        } catch (RuntimeException e) {
            LimeLog.warning("Roaming: could not follow network changes, not roaming: " + e);
            return null;
        }
        return watch;
    }

    /** The stream ends: no report reaches the engine after this returns. */
    void stop() {
        tracker.stop();
        try {
            connMgr.unregisterNetworkCallback(callback);
        } catch (RuntimeException e) {
            LimeLog.warning("Roaming: could not stop following network changes: " + e);
        }
    }
}
