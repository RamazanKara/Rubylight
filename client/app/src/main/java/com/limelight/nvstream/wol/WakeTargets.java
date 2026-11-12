package com.limelight.nvstream.wol;

import android.content.Context;
import android.content.SharedPreferences;

import com.limelight.LimeLog;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nativeapi.NativeWake;
import com.limelight.nvstream.http.ComputerDetails;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;

/**
 * The wake targets a host's native API lists besides serverinfo's MAC (GET /wake): the MACs of its other adapters
 * and the subnet broadcasts that reach them. They live in their own preferences file keyed by host UUID, so a PC that
 * is asleep can still be woken through them; a host without {@link NativeCapabilities#WAKE} has none.
 *
 * <p>The user can add MACs of their own for a PC (an adapter that wakes it while the one paired over cannot), kept
 * under {@code manual:<uuid>}: what the host lists replaces only its own entry, and only removing the PC drops them.
 */
public final class WakeTargets {
    static final String PREFS = "wake_targets";
    /** A host lists one target per physical adapter; more than this is not a PC's worth. */
    static final int MAX_MACS = 8;
    static final int MAX_BROADCASTS = 16;
    static final String MANUAL = "manual:";

    /** What is stored for one host. */
    public static final class Targets {
        public static final Targets NONE = new Targets(Collections.emptyList(), Collections.emptyList());

        /** Lowercase, colon-separated. */
        public final List<String> macs;
        /** IPv4 literals. */
        public final List<String> broadcasts;

        Targets(List<String> macs, List<String> broadcasts) {
            this.macs = Collections.unmodifiableList(macs);
            this.broadcasts = Collections.unmodifiableList(broadcasts);
        }
    }

    private final SharedPreferences prefs;

    public WakeTargets(Context context) {
        this.prefs = context.getApplicationContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    /**
     * Whether there is a MAC to wake the PC by: serverinfo's, or one its {@code /wake} listed. Every wake gate asks
     * this, so a PC whose serverinfo has no MAC still wakes through its other adapters.
     */
    public static boolean hasMac(Context context, ComputerDetails computer) {
        return computer != null && (computer.macAddress != null
                || !new WakeTargets(context).load(computer.uuid).macs.isEmpty());
    }

    /** The host's targets with the user's own MACs after them. */
    public synchronized Targets load(String hostUuid) {
        List<String> manual = manual(hostUuid);
        String stored = hostUuid == null ? null : prefs.getString(hostUuid, null);
        if (stored == null) {
            return manual.isEmpty() ? Targets.NONE : new Targets(manual, new ArrayList<>());
        }
        try {
            JSONObject json = new JSONObject(stored);
            List<String> macs = strings(json.optJSONArray("macs"));
            for (String mac : manual) {
                if (!macs.contains(mac)) macs.add(mac);
            }
            return new Targets(macs, strings(json.optJSONArray("broadcasts")));
        } catch (JSONException e) {
            LimeLog.warning("Unreadable wake targets of " + hostUuid + ": " + e.getMessage());
            return manual.isEmpty() ? Targets.NONE : new Targets(manual, new ArrayList<>());
        }
    }

    /** The MACs the user added for the PC, lowercase and colon-separated. */
    public synchronized List<String> manual(String hostUuid) {
        String stored = hostUuid == null ? null : prefs.getString(MANUAL + hostUuid, null);
        if (stored == null) {
            return new ArrayList<>();
        }
        try {
            List<String> macs = new ArrayList<>();
            for (String mac : strings(new JSONArray(stored))) addMac(macs, mac);
            return macs;
        } catch (JSONException e) {
            LimeLog.warning("Unreadable wake addresses of " + hostUuid + ": " + e.getMessage());
            return new ArrayList<>();
        }
    }

    /** Keeps the user's MACs for the PC (already checked by {@link #parseMacs}); an empty list removes them. */
    public synchronized void storeManual(String hostUuid, List<String> macs) {
        if (macs.isEmpty()) {
            prefs.edit().remove(MANUAL + hostUuid).apply();
        } else {
            prefs.edit().putString(MANUAL + hostUuid, new JSONArray(macs).toString()).apply();
        }
    }

    /**
     * The MACs typed into the PC's wake addresses, separated by commas, spaces or lines, as {@code 00:11:22:33:44:55},
     * {@code 00-11-22-33-44-55} or {@code 001122334455}; each once. Null when one of them is not a MAC address, is all
     * zeros, or there are more than {@link #MAX_MACS}.
     */
    public static List<String> parseMacs(String text) {
        List<String> macs = new ArrayList<>();
        for (String token : text.trim().split("[\\s,;]+")) {
            if (token.isEmpty()) continue;
            String mac = token.matches("(?i)[0-9a-f]{12}") ? token.replaceAll("(..)(?!$)", "$1:") : token;
            if (!isMac(mac)) return null;
            String normal = normal(mac);
            if (macs.contains(normal)) continue;
            if (macs.size() >= MAX_MACS || normal.equals("00:00:00:00:00:00")) return null;
            macs.add(normal);
        }
        return macs;
    }

    /** Keeps the host's targets in place of the stored ones; MACs and broadcasts that are not well formed are left out. */
    public synchronized void store(String hostUuid, NativeWake wake) {
        List<String> macs = new ArrayList<>();
        List<String> broadcasts = new ArrayList<>();
        if (wake.primary != null) addMac(macs, wake.primary);
        for (NativeWake.Target target : wake.targets) {
            addMac(macs, target.mac);
            for (String broadcast : target.broadcasts) {
                byte[] address = ipv4(broadcast);
                String normal = address == null ? null : (address[0] & 0xff) + "." + (address[1] & 0xff) + "."
                        + (address[2] & 0xff) + "." + (address[3] & 0xff);
                if (normal != null && broadcasts.size() < MAX_BROADCASTS && !broadcasts.contains(normal)) {
                    broadcasts.add(normal);
                }
            }
        }
        try {
            prefs.edit().putString(hostUuid, new JSONObject().put("macs", new JSONArray(macs))
                    .put("broadcasts", new JSONArray(broadcasts)).toString()).apply();
        } catch (JSONException e) {
            throw new IllegalStateException(e);
        }
    }

    /** Drops what the host listed; the user's own MACs stay (see {@link #forgetHost}). */
    public synchronized void forget(String hostUuid) {
        prefs.edit().remove(hostUuid).apply();
    }

    /** Drops everything kept for a removed PC, the user's own MACs too. */
    public synchronized void forgetHost(String hostUuid) {
        prefs.edit().remove(hostUuid).remove(MANUAL + hostUuid).apply();
    }

    /**
     * Reads the targets of a host that lists {@link NativeCapabilities#WAKE}, and forgets them for one that does not
     * (blocks on HTTPS). A failed read keeps what is stored: those adapters still belong to the PC.
     */
    public void refresh(String hostUuid, NativeCapabilities capabilities, NativeApiClient client) {
        if (hostUuid == null) {
            return;
        }
        if (!capabilities.has(NativeCapabilities.WAKE)) {
            forget(hostUuid);
            return;
        }
        try {
            store(hostUuid, client.wake());
        } catch (NativeApiException e) {
            LimeLog.info("Wake targets of " + hostUuid + " not read: " + e.getMessage());
        }
    }

    private static boolean isMac(String mac) {
        return mac.matches("(?i)[0-9a-f]{2}([:-])[0-9a-f]{2}(\\1[0-9a-f]{2}){4}");
    }

    private static String normal(String mac) {
        return mac.replace('-', ':').toLowerCase(Locale.ROOT);
    }

    private static void addMac(List<String> macs, String mac) {
        if (mac == null || macs.size() >= MAX_MACS || !isMac(mac)) {
            return;
        }
        String normal = normal(mac);
        // A zero MAC is a host that could not tell; it wakes nothing.
        if (!normal.equals("00:00:00:00:00:00") && !macs.contains(normal)) {
            macs.add(normal);
        }
    }

    /** The four bytes of a dotted-quad IPv4 literal, or null: a name here would need a DNS lookup. */
    static byte[] ipv4(String literal) {
        String[] parts = literal.split("\\.", -1);
        if (parts.length != 4) {
            return null;
        }
        byte[] bytes = new byte[4];
        for (int i = 0; i < 4; i++) {
            if (!parts[i].matches("[0-9]{1,3}")) {
                return null;
            }
            int value = Integer.parseInt(parts[i]);
            if (value > 255) {
                return null;
            }
            bytes[i] = (byte) value;
        }
        return bytes;
    }

    private static List<String> strings(JSONArray array) {
        List<String> values = new ArrayList<>();
        for (int i = 0; array != null && i < array.length(); i++) {
            Object value = array.opt(i);
            if (value instanceof String) values.add((String) value);
        }
        return values;
    }
}
