package com.limelight.preferences;

import android.Manifest;
import android.content.Context;
import android.content.pm.PackageManager;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.NetworkInfo;
import android.net.wifi.WifiInfo;
import android.net.wifi.WifiManager;
import android.os.Build;

/**
 * The network this device streams over, as profiles see it. The Wi-Fi name is read only when a
 * home Wi-Fi is set and location access was granted; it stays in memory and is never logged.
 */
public final class StreamNetwork {
    public final boolean cellular, wifi, metered;
    /** The Wi-Fi name, or null when not on Wi-Fi, not asked for or not readable. */
    public final String wifiName;
    /** True when on Wi-Fi and its name was asked for but could not be read. */
    public final boolean wifiNameMissing;

    StreamNetwork(boolean cellular, boolean wifi, boolean metered, String wifiName, boolean wifiNameMissing) {
        this.cellular = cellular;
        this.wifi = wifi;
        this.metered = metered;
        this.wifiName = wifiName;
        this.wifiNameMissing = wifiNameMissing;
    }

    public static boolean canReadWifiName(Context context) {
        return Build.VERSION.SDK_INT < Build.VERSION_CODES.M || context.checkSelfPermission(
                Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED;
    }

    /** Reads the network now; asks Android for the Wi-Fi name only when {@code wantWifiName}. */
    @SuppressWarnings("deprecation")
    public static StreamNetwork read(Context context, boolean wantWifiName) {
        boolean cellular = false, wifi = false, metered = false;
        ConnectivityManager connectivity = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        if (connectivity != null) {
            try {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                    Network active = connectivity.getActiveNetwork();
                    NetworkCapabilities caps = active == null ? null : connectivity.getNetworkCapabilities(active);
                    if (caps != null) {
                        cellular = caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR);
                        wifi = caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI);
                    }
                } else {
                    NetworkInfo info = connectivity.getActiveNetworkInfo();
                    if (info != null) {
                        cellular = info.getType() == ConnectivityManager.TYPE_MOBILE;
                        wifi = info.getType() == ConnectivityManager.TYPE_WIFI;
                    }
                }
                metered = connectivity.isActiveNetworkMetered();
            } catch (SecurityException | IllegalStateException e) {
                // Leave the network unknown; the profile in use applies.
            }
        }
        String wifiName = null;
        boolean missing = false;
        if (wifi && wantWifiName) {
            if (canReadWifiName(context)) {
                try {
                    WifiManager manager = (WifiManager) context.getApplicationContext().getSystemService(Context.WIFI_SERVICE);
                    WifiInfo info = manager == null ? null : manager.getConnectionInfo();
                    wifiName = info == null ? null : StreamLayers.cleanWifiName(info.getSSID());
                } catch (SecurityException e) {
                    wifiName = null;
                }
            }
            missing = wifiName == null;
        }
        return new StreamNetwork(cellular, wifi, metered, wifiName, missing);
    }

    /** Reads the network and fills the network part of {@code input}. */
    public static void fill(Context context, StreamLayers.Input input) {
        boolean wantName = input.profiles.auto && !input.profiles.homeWifi.isEmpty();
        StreamNetwork network = read(context, wantName);
        input.network = StreamLayers.classify(network.cellular, network.wifi, network.metered,
                network.wifiName, input.profiles.homeWifi);
        input.wifiNameKnown = !network.wifiNameMissing;
    }
}
