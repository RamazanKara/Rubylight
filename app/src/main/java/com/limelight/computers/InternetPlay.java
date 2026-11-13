package com.limelight.computers;

import android.content.Context;
import android.content.SharedPreferences;

import androidx.annotation.VisibleForTesting;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;

/**
 * Play over the internet: reaching a PC from outside its network without port forwarding. The PC's
 * internet-play/v1 reply, read on its network while paired, is kept per PC here; the paths, the tunnel and the
 * words come from libnightjar_core (nightjar-core internet.rs, protocol/android/src/internet.rs), the same code
 * desktop Nightjar uses.
 *
 * <p>The punched path binds the PC's ports on a loopback address of this device, so the PC is then polled, paired
 * and streamed to at that address like any other, by either engine.
 */
public final class InternetPlay {
    static final String PREFERENCES = "InternetPlay";
    /** How often a PC's reply is read again while it is reached on its own network. */
    static final long READ_INTERVAL_MS = 10 * 60 * 1000;

    private InternetPlay() {
    }

    /** One JSON request in, one JSON answer out: {@code {"ok": ...}} or {@code {"error": words}}. */
    public interface Calls {
        String call(String request);
    }

    private static Calls calls;

    private static synchronized Calls calls() {
        if (calls == null) {
            System.loadLibrary("nightjar_core");
            calls = InternetPlay::call;
        }
        return calls;
    }

    /** JVM tests cannot load the library. */
    @VisibleForTesting
    public static synchronized void use(Calls replacement) {
        calls = replacement;
    }

    private static native String call(String request);

    /** The answer's "ok" value, or an exception with its words. */
    private static Object ask(JSONObject request) throws JSONException, Unreachable {
        String answer;
        try {
            answer = calls().call(request.toString());
        } catch (UnsatisfiedLinkError e) {
            throw new Unreachable("Play over the internet is not available on this device");
        }
        if (answer == null) throw new Unreachable("Play over the internet did not answer");
        JSONObject root = new JSONObject(answer);
        if (root.has("error")) throw new Unreachable(root.optString("error"));
        return root.opt("ok");
    }

    /** Why no path reached the PC, in words for the user. */
    public static final class Unreachable extends Exception {
        Unreachable(String words) {
            super(words);
        }
    }

    /** The internet addresses to poll after the PC's own, in order. */
    public static final class Addresses {
        public final List<ComputerDetails.AddressTuple> ipv6 = new ArrayList<>();
        public ComputerDetails.AddressTuple mapped;
    }

    /** Which direct paths a poll asked. */
    public static final class Tried {
        public boolean lan, ipv6, mapped;
    }

    // --- The kept replies ---

    private static SharedPreferences preferences(Context context) {
        return context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    }

    /** The PC's kept reply, or null when it never gave one. */
    public static String reply(Context context, String hostUuid) {
        if (context == null || hostUuid == null) return null;
        return preferences(context).getString(hostUuid, null);
    }

    /** Keeps the PC's reply; an unchanged one is not written again. */
    public static void save(Context context, String hostUuid, String reply) {
        if (context == null || hostUuid == null || reply == null) return;
        SharedPreferences preferences = preferences(context);
        if (!reply.equals(preferences.getString(hostUuid, null))) {
            preferences.edit().putString(hostUuid, reply).apply();
        }
    }

    /** The PC was removed or forgot this device: its key goes too. */
    public static void forget(Context context, String hostUuid) {
        if (context == null || hostUuid == null) return;
        preferences(context).edit().remove(hostUuid).apply();
        try {
            ask(new JSONObject().put("op", "stop").put("host", hostUuid));
        } catch (JSONException | Unreachable e) {
            LimeLog.warning("Internet play: stopping the tunnel failed: " + e.getMessage());
        }
    }

    // --- The shared calls ---

    /** Takes in this device's "Play over the internet" setting; off ends every tunnel. */
    public static void setEnabled(boolean on) {
        try {
            ask(new JSONObject().put("op", "enable").put("on", on));
        } catch (JSONException | Unreachable e) {
            LimeLog.warning("Internet play: " + e.getMessage());
        }
    }

    /** The PC's IPv6 addresses and forwarded IPv4 address from its reply, or none. */
    public static Addresses addresses(String reply) {
        Addresses out = new Addresses();
        if (reply == null) return out;
        try {
            JSONObject ok = (JSONObject) ask(new JSONObject().put("op", "addresses").put("reply", reply));
            int port = ok.getInt("port");
            JSONArray ipv6 = ok.getJSONArray("ipv6");
            for (int i = 0; i < ipv6.length(); i++) {
                out.ipv6.add(new ComputerDetails.AddressTuple(ipv6.getString(i), port));
            }
            if (!ok.isNull("mapped")) out.mapped = new ComputerDetails.AddressTuple(ok.getString("mapped"), port);
        } catch (JSONException | ClassCastException | Unreachable e) {
            LimeLog.warning("Internet play: the kept reply could not be read: " + e.getMessage());
        }
        return out;
    }

    private static JSONObject tried(Tried tried) throws JSONException {
        return new JSONObject().put("lan", tried.lan).put("ipv6", tried.ipv6).put("mapped", tried.mapped);
    }

    /**
     * The punched path: blocks up to 50 seconds for the tunnel, then gives its loopback end, the address the PC is
     * reached at from now on. Call it off the main thread.
     */
    public static ComputerDetails.AddressTuple connect(String hostUuid, String reply, Tried tried) throws Unreachable {
        try {
            JSONObject ok = (JSONObject) ask(new JSONObject().put("op", "connect").put("host", hostUuid)
                    .put("reply", reply).put("tried", tried(tried)));
            return new ComputerDetails.AddressTuple(ok.getString("address"), ok.getInt("port"));
        } catch (JSONException | ClassCastException e) {
            throw new Unreachable("Play over the internet gave an unreadable answer");
        }
    }

    /** The words for every path when the PC is not reached, without punching. */
    public static String unreachable(String reply, Tried tried) {
        try {
            return (String) ask(new JSONObject().put("op", "unreachable").put("reply", reply).put("tried", tried(tried)));
        } catch (JSONException | ClassCastException | Unreachable e) {
            return e.getMessage();
        }
    }

    /** "lan", "ipv6", "mapped" or "punched": how {@code address} reaches the PC. */
    public static String path(ComputerDetails details, String reply) {
        if (details == null || details.activeAddress == null) return null;
        try {
            JSONArray lan = new JSONArray();
            for (ComputerDetails.AddressTuple own : new ComputerDetails.AddressTuple[] {details.localAddress, details.manualAddress}) {
                if (own != null) lan.put(own.address);
            }
            String key = (String) ask(new JSONObject().put("op", "path").put("address", details.activeAddress.address)
                    .put("lan", lan).put("reply", reply == null ? JSONObject.NULL : reply));
            // The remote address is a forwarded port even without a reply naming it.
            if ("lan".equals(key) && details.activeAddress.equals(details.remoteAddress)) return "mapped";
            if ("lan".equals(key) && details.activeAddress.equals(details.ipv6Address)) return "ipv6";
            return key;
        } catch (JSONException | ClassCastException | Unreachable e) {
            return null;
        }
    }

    /** The words for a path key, or null for the local network, which needs none. */
    public static String pathLabel(Context context, String key) {
        if (key == null) return null;
        switch (key) {
            case "ipv6": return context.getString(R.string.internet_path_ipv6);
            case "mapped": return context.getString(R.string.internet_path_mapped);
            case "punched": return context.getString(R.string.internet_path_punched);
            default: return null;
        }
    }

    /** Whether {@code address} is the loopback end of a punched path (127.78.x.y, nightjar-core internet.rs). */
    public static boolean isTunnelAddress(String address) {
        return address != null && address.startsWith("127.78.");
    }

    /** Whether the PC's reply is due to be read again. */
    static boolean due(Long lastRead, long now) {
        return lastRead == null || now - lastRead >= READ_INTERVAL_MS;
    }
}
