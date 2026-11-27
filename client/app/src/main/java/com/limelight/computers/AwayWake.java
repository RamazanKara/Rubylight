package com.limelight.computers;

import android.content.Context;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.wol.WakeRelayKeys;
import com.limelight.nvstream.wol.WakeTargets;

import java.net.Inet4Address;
import java.net.InterfaceAddress;
import java.net.NetworkInterface;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Enumeration;
import java.util.List;

/**
 * Waking a PC from away (Away wake relay, docs/designs/15-away-wake-relay.md): whether Wake on an offline PC asks one
 * of its wake relays, and the words for the request's steps and outcome (from protocol/android/src/wake_relay.rs).
 * Decided here without Android, so a JVM test covers it; {@link AwayWakeFlow} runs the request.
 */
final class AwayWake {
    private AwayWake() {
    }

    enum Path {
        /** Today's wake: the magic packets from this device, then the wait for the PC. */
        LAN,
        /** The magic packets, and a request to one of the PC's relays. */
        RELAY,
        /** The PC offers waking from away, but this device has no relay to ask. */
        NO_RELAY,
    }

    /**
     * Only an offline PC whose host offers waking from away ({@code kept} non-null), seen from outside its network,
     * leaves today's wake.
     */
    static Path path(boolean offline, boolean onPcNetwork, WakeRelayKeys.Kept kept) {
        if (!offline || onPcNetwork || kept == null) {
            return Path.LAN;
        }
        return kept.hasRelay() ? Path.RELAY : Path.NO_RELAY;
    }

    /** One IPv4 subnet this device is on: its address and prefix length. */
    static final class Subnet {
        final int address;
        final int prefix;

        Subnet(int address, int prefix) {
            this.address = address;
            this.prefix = prefix;
        }

        boolean contains(int other) {
            if (prefix < 1 || prefix > 32) {
                return false;
            }
            int mask = prefix == 32 ? -1 : ~(-1 >>> prefix);
            return (address & mask) == (other & mask);
        }

        int broadcast() {
            return prefix >= 32 ? address : address | (-1 >>> prefix);
        }
    }

    /**
     * Whether this device is on the PC's network, as a wake from it reaches the PC: one of its subnets holds the
     * address the PC had there, or is a subnet the PC listed for waking (GET /wake's broadcasts). A device on another
     * network, or none, is away.
     */
    static boolean onPcNetwork(List<Subnet> own, List<String> pcAddresses, List<String> pcBroadcasts) {
        for (Subnet subnet : own) {
            for (String address : pcAddresses) {
                Integer ip = ipv4(address);
                if (ip != null && subnet.contains(ip)) {
                    return true;
                }
            }
            for (String broadcast : pcBroadcasts) {
                Integer ip = ipv4(broadcast);
                if (ip != null && subnet.prefix >= 8 && subnet.prefix <= 30 && subnet.broadcast() == ip) {
                    return true;
                }
            }
        }
        return false;
    }

    /** What chooses an offline PC's wake path: its away wake keys (null without), and whether this device is home. */
    static final class Seen {
        final WakeRelayKeys.Kept kept;
        final boolean onPcNetwork;

        Seen(WakeRelayKeys.Kept kept, boolean onPcNetwork) {
            this.kept = kept;
            this.onPcNetwork = onPcNetwork;
        }
    }

    /** Reads {@link Seen} for a PC: from a file, preferences and the network stack, so off the main thread. */
    interface Look {
        Seen at(ComputerDetails computer);
    }

    /** {@link #onPcNetwork(List, List, List)} with this device's interfaces and what it knows of the PC. */
    static boolean onPcNetwork(Context context, ComputerDetails computer) {
        List<String> addresses = new ArrayList<>();
        for (ComputerDetails.AddressTuple address : new ComputerDetails.AddressTuple[] {
                computer.localAddress, computer.manualAddress}) {
            if (address != null) {
                addresses.add(address.address);
            }
        }
        return onPcNetwork(ownSubnets(), addresses, new WakeTargets(context).load(computer.uuid).broadcasts);
    }

    private static List<Subnet> ownSubnets() {
        List<Subnet> subnets = new ArrayList<>();
        try {
            Enumeration<NetworkInterface> interfaces = NetworkInterface.getNetworkInterfaces();
            for (NetworkInterface nic : interfaces == null ? Collections.<NetworkInterface>emptyList()
                    : Collections.list(interfaces)) {
                if (!nic.isUp() || nic.isLoopback()) {
                    continue;
                }
                for (InterfaceAddress address : nic.getInterfaceAddresses()) {
                    if (address.getAddress() instanceof Inet4Address) {
                        byte[] b = address.getAddress().getAddress();
                        int ip = (b[0] & 0xff) << 24 | (b[1] & 0xff) << 16 | (b[2] & 0xff) << 8 | (b[3] & 0xff);
                        subnets.add(new Subnet(ip, address.getNetworkPrefixLength()));
                    }
                }
            }
        } catch (java.io.IOException | SecurityException e) {
            LimeLog.warning("Wake from away cannot list the network interfaces: " + e);
        }
        return subnets;
    }

    /** A dotted-quad IPv4 literal, or null: a name would need a lookup. */
    static Integer ipv4(String literal) {
        if (literal == null) {
            return null;
        }
        String[] parts = literal.trim().split("\\.", -1);
        if (parts.length != 4) {
            return null;
        }
        int ip = 0;
        for (String part : parts) {
            if (part.isEmpty() || part.length() > 3 || !part.chars().allMatch(Character::isDigit)) {
                return null;
            }
            int value = Integer.parseInt(part);
            if (value > 255) {
                return null;
            }
            ip = ip << 8 | value;
        }
        return ip;
    }

    /** A string resource and its arguments. */
    static final class Words {
        final int text;
        final Object[] args;

        Words(int text, Object... args) {
            this.text = text;
            this.args = args;
        }

        String in(Context context) {
            return context.getString(text, args);
        }
    }

    /** What the dialog says when the request starts, before the first step. */
    static Words starting(WakeRelayKeys.Kept kept) {
        return new Words(R.string.wake_relay_contacting, kept.relays.isEmpty() ? "" : kept.relays.get(0));
    }

    /** The words for one step of the request ({@code {"stage", "relay", "status"}}), or null to keep the last. */
    static Words step(String json, String pc) {
        Json.Obj step = parse(json);
        String relay = step.optString("relay", "");
        switch (step.optString("stage", "")) {
            case "looking_up":
            case "punching":
                return new Words(R.string.wake_relay_contacting, relay);
            case "requested":
                return new Words(R.string.wake_relay_asking, relay, pc);
            case "status": {
                String status = step.optString("status", "");
                return "packet_sent".equals(status) || "waiting_for_boot".equals(status)
                        ? new Words(R.string.wake_relay_sent, pc) : null;
            }
            default:
                return null;
        }
    }

    /** The outcome's key ({@code "awake"}, {@code "cancelled"}, ...). */
    static String outcomeKey(String json) {
        return parse(json).optString("outcome", "");
    }

    /** The words for the outcome ({@code {"outcome", "relays", "retry_after"}}), or null when there is nothing to say. */
    static Words outcome(String json, String pc) {
        Json.Obj outcome = parse(json);
        String relays = names(outcome.optArray("relays"));
        switch (outcome.optString("outcome", "")) {
            case "cancelled":
                return null;
            case "awake":
                return new Words(R.string.wake_relay_awake, pc);
            case "sent_no_answer":
                return new Words(R.string.wake_relay_sent_no_answer, pc);
            case "timed_out":
                return new Words(R.string.wake_relay_timed_out, pc);
            case "rate_limited":
                return new Words(R.string.wake_relay_rate_limited, pc, Math.max(1, outcome.optInt("retry_after", 0)));
            case "refused":
            case "invalid":
                return new Words(R.string.wake_relay_refused);
            case "not_on_pc_network":
                return new Words(R.string.wake_relay_elsewhere, relays, pc);
            case "relay_offline":
                return new Words(R.string.wake_relay_offline, relays, pc);
            case "no_relay":
                return new Words(R.string.wake_relay_none, pc);
            case "network":
                return new Words(R.string.wake_relay_network);
            case "relay_unreachable":
            default:
                return new Words(R.string.wake_relay_unreachable, relays);
        }
    }

    /** Whether trying again may end otherwise: not after a refusal, a rate limit or a wake that worked. */
    static boolean offersRetry(String outcomeKey) {
        switch (outcomeKey) {
            case "sent_no_answer":
            case "timed_out":
            case "relay_offline":
            case "relay_unreachable":
            case "network":
                return true;
            default:
                return false;
        }
    }

    /** The words for a PC that offers waking from away without a relay for this device. */
    static Words noRelay(String pc) {
        return new Words(R.string.wake_relay_none, pc);
    }

    private static Json.Obj parse(String json) {
        try {
            return json == null ? Json.obj() : Json.parseObject(json);
        } catch (Json.SyntaxException e) {
            return Json.obj();
        }
    }

    private static String names(Json.Arr relays) {
        StringBuilder out = new StringBuilder();
        for (int i = 0; relays != null && i < relays.length(); i++) {
            String name = relays.optString(i, "");
            if (!name.isEmpty()) {
                out.append(out.length() == 0 ? "" : ", ").append(name);
            }
        }
        return out.toString();
    }
}
