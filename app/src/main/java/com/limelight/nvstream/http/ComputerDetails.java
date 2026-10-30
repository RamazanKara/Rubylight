package com.limelight.nvstream.http;

import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;


public class ComputerDetails {
    public static final int PERMISSION_CONTROLLER = 1 << 8;
    public static final int PERMISSION_INPUT = 0x00001F00;
    public static final int PERMISSION_CLIPBOARD_SET = 1 << 16;
    public static final int PERMISSION_CLIPBOARD_READ = 1 << 17;
    public static final int PERMISSION_SERVER_COMMAND = 1 << 20;
    public static final int PERMISSION_LIST = 1 << 24;
    public static final int PERMISSION_VIEW = 1 << 25;
    public static final int PERMISSION_LAUNCH = 1 << 26;
    // Masks in the order of the host_permission_names string array
    public static final int[] PERMISSION_DISPLAY_MASKS = {1 << 8, 1 << 9, 1 << 10, 1 << 11, 1 << 12,
            PERMISSION_CLIPBOARD_SET, PERMISSION_CLIPBOARD_READ, PERMISSION_SERVER_COMMAND, PERMISSION_LIST,
            PERMISSION_VIEW | PERMISSION_LAUNCH, PERMISSION_LAUNCH};

    public enum State {
        ONLINE, OFFLINE, UNKNOWN
    }

    public static class AddressTuple {
        public String address;
        public int port;

        public AddressTuple(String address, int port) {
            if (address == null) {
                throw new IllegalArgumentException("Address cannot be null");
            }
            if (port <= 0 || port > 65535) {
                throw new IllegalArgumentException("Invalid port");
            }

            // If this was an escaped IPv6 address, remove the brackets
            if (address.startsWith("[") && address.endsWith("]")) {
                address = address.substring(1, address.length() - 1);
            }

            this.address = address;
            this.port = port;
        }

        @Override
        public int hashCode() {
            return Objects.hash(address, port);
        }

        @Override
        public boolean equals(Object obj) {
            if (!(obj instanceof AddressTuple)) {
                return false;
            }

            AddressTuple that = (AddressTuple) obj;
            return address.equals(that.address) && port == that.port;
        }

        public String toString() {
            if (address.contains(":")) {
                // IPv6
                return "[" + address + "]:" + port;
            }
            else {
                // IPv4 and hostnames
                return address + ":" + port;
            }
        }
    }

    // Persistent attributes
    public String uuid;
    public String name;
    /**
     * The name this device shows instead of {@link #name}, set by the user and kept in HostNames. It is
     * not persistent in the database and {@link #update} leaves it alone, so a poll that renames the
     * PC cannot replace it.
     */
    public String alias;
    public AddressTuple localAddress;
    public AddressTuple remoteAddress;
    public AddressTuple manualAddress;
    public AddressTuple ipv6Address;
    /**
     * The known address the user chose to always connect through, or null for the automatic pick
     * (Moonlight V+ PC's pinned address). Kept beside the database ({@code PinnedAddresses}).
     */
    public AddressTuple pinnedAddress;
    public String macAddress;
    public X509Certificate serverCert;

    // Transient attributes
    public State state;
    public AddressTuple activeAddress;
    public int httpsPort;
    public int externalPort;
    public PairingManager.PairState pairState;
    public int runningGameId;
    public boolean sessionIdle;
    public String runningGameUuid;
    public String rawAppList;
    public boolean nvidiaServer;
    public long permission = -1;
    // What the host can do, from its last serverinfo reply; offline PCs keep their cached copy.
    public HostCapabilities caps = HostCapabilities.UNKNOWN;
    public boolean encoderBitrateSupported;
    public int encoderBitrateKbps;
    public int initialEncoderBitrateKbps;
    public ArrayList<String> serverCommands = new ArrayList<>();
    public boolean frameLimiterEnabled;
    public boolean virtualDisplayFrameLimiterEnabled;
    public long frameLimiterFpsLimitMilliHz;
    public long pyroWaveHostLinkMbps;
    public int pyroWaveBandwidthProbeBytes;
    /** Frame-shaped UDP probes with the handshake (Vibeshine), and the FEC its wire budget counts. */
    public boolean pyroWaveBurstProbe;
    public int pyroWaveCriticalFecPercentage;
    public int pyroWaveMinParityShards;
    /**
     * The packed full-chroma modes the host offers (Rubylight's {@code Packed444Modes}: HEVC 8-bit 1,
     * HEVC 10-bit 2, AV1 8-bit 4, AV1 10-bit 8), or -1 when its serverinfo does not say.
     */
    public int packed444Modes = -1;

    /**
     * Whether the host reports a running game. Apollo hides the game while its input-only mode is
     * on: it sends game ID 0 and a free state, but still the game's UUID.
     */
    public boolean hasRunningGame() {
        return runningGameId != 0 || (runningGameUuid != null && !runningGameUuid.isEmpty());
    }

    public boolean supportsHardwareMetrics() {
        return caps.has(HostCapabilities.Feature.HARDWARE_METRICS) && pairState == PairingManager.PairState.PAIRED &&
                hasPermission(PERMISSION_VIEW | PERMISSION_LAUNCH);
    }

    public boolean supportsPyroWaveBandwidthProbe() {
        return caps.has(HostCapabilities.Feature.PYROWAVE_BANDWIDTH_PROBE) &&
                pairState == PairingManager.PairState.PAIRED &&
                pyroWaveBandwidthProbeBytes == NvHTTP.PYROWAVE_BANDWIDTH_PROBE_BYTES;
    }

    public boolean supportsPyroWavePaceCalibration() {
        return caps.has(HostCapabilities.Feature.PYROWAVE) && pairState == PairingManager.PairState.PAIRED &&
                pyroWaveBurstProbe;
    }

    public boolean hasPermission(int mask) {
        // Hosts without Apollo permissions retain their existing behavior.
        return permission == -1 || (permission & mask) != 0;
    }

    public boolean canReadClipboard() {
        return permission != -1 && hasPermission(PERMISSION_CLIPBOARD_READ) &&
                hasPermission(PERMISSION_VIEW | PERMISSION_LAUNCH);
    }

    public boolean canWriteClipboard() {
        return permission != -1 && hasPermission(PERMISSION_CLIPBOARD_SET) &&
                hasPermission(PERMISSION_VIEW | PERMISSION_LAUNCH);
    }

    public boolean canRunServerCommand(int index) {
        return index >= 0 && index < Math.min(serverCommands.size(), 256) &&
                hasPermission(PERMISSION_SERVER_COMMAND);
    }

    public byte[] serverCommandPayload(int index) {
        if (!canRunServerCommand(index)) {
            throw new IllegalArgumentException("Server command unavailable or permission denied");
        }
        // Rubylight accepts one byte; Apollo/Artemis uses three reserved trailing bytes.
        return caps.family() == HostCapabilities.Family.RUBYLIGHT ? new byte[] {(byte) index} :
                new byte[] {(byte) index, 0, 0, 0};
    }

    public ComputerDetails() {
        // Use defaults
        state = State.UNKNOWN;
    }

    public ComputerDetails(ComputerDetails details) {
        // Copy details from the other computer
        this.pinnedAddress = details.pinnedAddress;
        update(details);
        this.alias = details.alias;
    }

    /** The PC's known addresses without repeats, in the automatic pick's order: local, manual, remote, IPv6. */
    public List<AddressTuple> knownAddresses() {
        List<AddressTuple> known = new ArrayList<>();
        for (AddressTuple address : new AddressTuple[] {localAddress, manualAddress, remoteAddress, ipv6Address}) {
            if (address != null && !known.contains(address)) {
                known.add(address);
            }
        }
        return known;
    }

    /** The addresses a poll tries: only the pinned one while the user pinned it, else every known one in order. */
    public List<AddressTuple> pollAddresses() {
        if (pinnedAddress != null) {
            List<AddressTuple> pinned = new ArrayList<>();
            pinned.add(pinnedAddress);
            return pinned;
        }
        return knownAddresses();
    }

    /** What this device calls the PC: the name the user gave it, else the PC's own. */
    public String displayName() {
        return alias != null && !alias.isEmpty() ? alias : name;
    }

    public int guessExternalPort() {
        if (externalPort != 0) {
            return externalPort;
        }
        else if (remoteAddress != null) {
            return remoteAddress.port;
        }
        else if (activeAddress != null) {
            return activeAddress.port;
        }
        else if (ipv6Address != null) {
            return ipv6Address.port;
        }
        else if (localAddress != null) {
            return localAddress.port;
        }
        else {
            return NvHTTP.DEFAULT_HTTP_PORT;
        }
    }

    public void update(ComputerDetails details) {
        this.state = details.state;
        this.name = details.name;
        this.uuid = details.uuid;
        if (details.activeAddress != null) {
            this.activeAddress = details.activeAddress;
        }
        // A pinned PC keeps its address whatever answered (an mDNS rediscovery, another poll).
        if (pinnedAddress != null) {
            this.activeAddress = pinnedAddress;
        }
        // We can get IPv4 loopback addresses with GS IPv6 Forwarder
        if (details.localAddress != null && !details.localAddress.address.startsWith("127.")) {
            this.localAddress = details.localAddress;
        }
        if (details.remoteAddress != null) {
            this.remoteAddress = details.remoteAddress;
        }
        else if (this.remoteAddress != null && details.externalPort != 0) {
            // If we have a remote address already (perhaps via STUN) but our updated details
            // don't have a new one (because GFE doesn't send one), propagate the external
            // port to the current remote address. We may have tried to guess it previously.
            this.remoteAddress = new AddressTuple(this.remoteAddress.address, details.externalPort);
        }
        if (details.manualAddress != null) {
            this.manualAddress = details.manualAddress;
        }
        if (details.ipv6Address != null) {
            this.ipv6Address = details.ipv6Address;
        }
        if (details.macAddress != null && !details.macAddress.equals("00:00:00:00:00:00")) {
            this.macAddress = details.macAddress;
        }
        if (details.serverCert != null) {
            this.serverCert = details.serverCert;
        }
        this.externalPort = details.externalPort;
        this.httpsPort = details.httpsPort;
        this.pairState = details.pairState;
        this.runningGameId = details.runningGameId;
        this.sessionIdle = details.sessionIdle;
        this.runningGameUuid = details.runningGameUuid;
        this.nvidiaServer = details.nvidiaServer;
        this.rawAppList = details.rawAppList;
        this.permission = details.permission;
        // Details that never reached the host (mDNS, a manual address) keep what is known.
        if (details.caps.isKnown() || !this.caps.isKnown()) {
            this.caps = details.caps;
        }
        this.encoderBitrateSupported = details.encoderBitrateSupported;
        this.encoderBitrateKbps = details.encoderBitrateKbps;
        this.initialEncoderBitrateKbps = details.initialEncoderBitrateKbps;
        this.serverCommands = new ArrayList<>(details.serverCommands);
        this.frameLimiterEnabled = details.frameLimiterEnabled;
        this.virtualDisplayFrameLimiterEnabled = details.virtualDisplayFrameLimiterEnabled;
        this.frameLimiterFpsLimitMilliHz = details.frameLimiterFpsLimitMilliHz;
        this.pyroWaveHostLinkMbps = details.pyroWaveHostLinkMbps;
        this.pyroWaveBandwidthProbeBytes = details.pyroWaveBandwidthProbeBytes;
        this.pyroWaveBurstProbe = details.pyroWaveBurstProbe;
        this.pyroWaveCriticalFecPercentage = details.pyroWaveCriticalFecPercentage;
        this.pyroWaveMinParityShards = details.pyroWaveMinParityShards;
        this.packed444Modes = details.packed444Modes;
    }

    private static void appendIfKnown(StringBuilder str, String label, Object value) {
        if (value != null) {
            str.append(label).append(value).append("\n");
        }
    }

    @Override
    public String toString() {
        StringBuilder str = new StringBuilder();
        str.append("Name: ").append(name).append("\n");
        str.append("State: ").append(state).append("\n");
        str.append("Active Address: ").append(activeAddress).append("\n");
        str.append("UUID: ").append(uuid).append("\n");
        // Skip addresses the host never reported instead of printing "null"
        appendIfKnown(str, "Local Address: ", localAddress);
        appendIfKnown(str, "Remote Address: ", remoteAddress);
        appendIfKnown(str, "IPv6 Address: ", ipv6Address);
        appendIfKnown(str, "Manual Address: ", manualAddress);
        appendIfKnown(str, "MAC Address: ", macAddress);
        str.append("Pair State: ").append(pairState).append("\n");
        str.append("Running Game ID: ").append(runningGameId).append("\n");
        str.append("HTTPS Port: ").append(httpsPort).append("\n");
        str.append("Host: ").append(caps).append("\n");
        str.append("Permissions: ").append(permission == -1 ? "Not advertised" : "0x" + Long.toHexString(permission)).append("\n");
        if (permission != -1) {
            str.append("List applications: ").append(hasPermission(PERMISSION_LIST)).append("\n");
            str.append("View streams: ").append(hasPermission(PERMISSION_VIEW | PERMISSION_LAUNCH)).append("\n");
            str.append("Launch/quit applications: ").append(hasPermission(PERMISSION_LAUNCH)).append("\n");
            str.append("Controller / touch / pen / mouse / keyboard: ");
            for (int bit = 8; bit <= 12; bit++) {
                str.append(hasPermission(1 << bit)).append(bit == 12 ? "\n" : " / ");
            }
            str.append("Clipboard send/read: ").append(canWriteClipboard()).append(" / ").append(canReadClipboard()).append("\n");
            str.append("Server commands: ").append(hasPermission(PERMISSION_SERVER_COMMAND)).append("\n");
        }
        return str.toString();
    }
}
