package com.limelight.utils;

import com.limelight.nvstream.http.ComputerDetails;

import java.io.IOException;
import java.io.InputStream;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.Set;

public final class RedactedLog {
    private static final int MAX_ENTRIES = 400;
    static final int MAX_TRACE_BYTES = 64 * 1024;
    static final int MAX_TRACE_CHARS = 4096;
    private static final Set<String> NATIVE_LIBRARIES = Set.of("libc.so", "libart.so", "libandroid_runtime.so",
            "libmoonlight-core.so", "libnightjar_core.so", "libpyrowave-renderer.so", "libpyrowave-shared.so", "libbinder.so", "libutils.so", "libhwui.so",
            "liblog.so", "libc++.so", "libc++_shared.so", "libbase.so", "libunwindstack.so", "base.apk");
    private static final Set<String> EVENTS = Set.of(
            "Stream starting", "Stream connected", "Stream stopping", "Stream failed",
            "Stream disconnected", "Stream activity destroyed", "Stream restored after process death",
            "Pairing started", "Pairing succeeded", "Pairing failed", "Pairing cancelled",
            "USB controller attached", "USB controller detached", "USB permission denied",
            "Bluetooth or Android controller detached", "Connection test started", "Connection test finished");
    private final ArrayDeque<String> entries = new ArrayDeque<>();
    // Names and addresses known to belong to the user's PCs; replaced wherever they appear.
    private static final Set<String> SECRETS = java.util.concurrent.ConcurrentHashMap.newKeySet();
    static final int MAX_MESSAGE_CHARS = 300;
    private static final java.util.regex.Pattern[] SENSITIVE = {
            // URLs, e-mail addresses, IPv4/IPv6 and MAC addresses, UUIDs, keys and paths.
            java.util.regex.Pattern.compile("[a-zA-Z][a-zA-Z0-9+.-]*://\\S+"),
            // Any character past ASCII too: Android's \w is Unicode, the JVM's (and Rust's copy) not.
            java.util.regex.Pattern.compile("[\\w\\x{80}-\\x{10FFFF}.+-]+@[\\w\\x{80}-\\x{10FFFF}-]+(?:\\.[\\w\\x{80}-\\x{10FFFF}-]+)+"),
            java.util.regex.Pattern.compile("\\b\\d{1,3}(?:\\.\\d{1,3}){3}(?::\\d+)?\\b"),
            java.util.regex.Pattern.compile("[0-9a-fA-F]{0,4}(?::[0-9a-fA-F]{0,4}){2,7}(?:%\\w+)?"),
            java.util.regex.Pattern.compile("\\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\\b"),
            java.util.regex.Pattern.compile("\\b[0-9a-fA-F]{16,}\\b"),
            java.util.regex.Pattern.compile("[A-Za-z0-9+/=_-]{32,}"),
            java.util.regex.Pattern.compile("(?:/storage|/sdcard|/data/user|/data/data|content:)\\S*"),
    };
    private static final java.util.regex.Pattern PIN = java.util.regex.Pattern.compile("(?i)\\b(pin|password|passphrase|passcode|secret|token|key)\\b(\\W{0,3})\\S+");
    private static final java.util.regex.Pattern SHORT_NUMBER = java.util.regex.Pattern.compile("\\b\\d{4,8}\\b");
    // Informational lines worth keeping; the rest is too frequent and would push out a session.
    private static final String[] INFO_PREFIXES = {
            "Stream request", "Connection terminated", "Selected ", "Decoder", "Reconfigure", "Stream now",
            "Display luminance", "Display modes changed", "Display refresh policy:", "Microphone", "PC stream profiles", "Present mode",
            "PyroWave", "Phase lock", "Low latency", "Automatic codec", "Streaming engine",
            "DualSense USB ", "USB controller announcement ", "Audio output:", "Audio track configuration:"};

    /** A PC name or address to keep out of problem reports. */
    public static void addSecret(String secret) {
        if (secret != null && secret.trim().length() >= 2) {
            SECRETS.add(secret.trim());
        }
    }

    public static void addComputerSecrets(ComputerDetails computer) {
        addSecret(computer.name);
        addSecret(computer.uuid);
        addSecret(computer.macAddress);
        for (ComputerDetails.AddressTuple address : new ComputerDetails.AddressTuple[] {
                computer.localAddress, computer.remoteAddress, computer.manualAddress,
                computer.ipv6Address, computer.activeAddress}) {
            if (address != null) addSecret(address.address);
        }
    }

    public synchronized void add(String level, String message) {
        // Only fixed event text crosses the export boundary. Host replies can contain arbitrary secrets.
        boolean important = "WARNING".equals(level) || "SEVERE".equals(level);
        if (!important && (message == null || (!EVENTS.contains(message) && !keptInfo(message)))) {
            return;
        }
        String safeMessage = redact(message);
        if (safeMessage == null) {
            safeMessage = "Diagnostic details omitted";
        }
        if (entries.size() == MAX_ENTRIES) {
            entries.removeFirst();
        }
        entries.addLast(("SEVERE".equals(level) ? "ERROR" : "WARNING".equals(level) ? "WARNING" : "INFO")
                + " " + safeMessage);
    }

    static String redact(String message) {
        if (message == null) {
            return null;
        }
        if (EVENTS.contains(message)) {
            return message;
        }
        // Our own log text crosses the boundary with names, addresses, IDs, PINs and keys taken out.
        String text = message.replace('\n', ' ').replace('\r', ' ');
        ArrayList<String> secrets = new ArrayList<>(SECRETS);
        secrets.sort(Comparator.comparingInt(String::length).reversed());
        for (String secret : secrets) {
            text = text.replace(secret, "[PC]");
        }
        String[] marks = {"[url]", "[email]", "[address]", "[address]", "[id]", "[hex]", "[data]", "[path]"};
        for (int i = 0; i < SENSITIVE.length; i++) {
            text = SENSITIVE[i].matcher(text).replaceAll(java.util.regex.Matcher.quoteReplacement(marks[i]));
        }
        text = PIN.matcher(text).replaceAll("$1$2[omitted]");
        if (text.toLowerCase(java.util.Locale.ROOT).contains("pair")) {
            // Pairing messages may carry the PIN as a bare number.
            text = SHORT_NUMBER.matcher(text).replaceAll("[omitted]");
        }
        return text.length() > MAX_MESSAGE_CHARS ? text.substring(0, MAX_MESSAGE_CHARS) + "…" : text;
    }

    static boolean keptInfo(String message) {
        for (String prefix : INFO_PREFIXES) {
            if (message.startsWith(prefix)) {
                return true;
            }
        }
        return false;
    }

    public synchronized String snapshot() {
        return String.join("\n", entries);
    }

    static String crashThread(String name) {
        // Networking libraries sometimes put the remote URL in the thread name.
        return name != null && name.length() <= 80 &&
                (name.equals("main") || name.equals("Crash history") || name.equals("FinalizerDaemon") ||
                        name.equals("FinalizerWatchdogDaemon") || name.equals("ReferenceQueueDaemon") ||
                        name.matches("(?:Thread-\\d+|pool-\\d+-thread-\\d+|AsyncTask #\\d+|GLThread \\d+)"))
                ? name : "[name omitted]";
    }

    static String nativeTrace(InputStream input) throws IOException {
        byte[] bytes = new byte[MAX_TRACE_BYTES];
        int length = 0;
        int count;
        while (length < bytes.length && (count = input.read(bytes, length, bytes.length - length)) != -1) {
            length += count;
        }
        StringBuilder text = new StringBuilder("Tombstone prefix (redacted; up to 64 KiB input):\n");
        try {
            appendTombstone(ByteBuffer.wrap(bytes, 0, length), 0, text);
        } catch (IOException e) {
            text.append("[trace incomplete]\n");
        }
        text.append("[messages, names, paths, registers, memory and logs omitted]\n");
        return text.substring(0, Math.min(text.length(), MAX_TRACE_CHARS));
    }

    private static void appendTombstone(ByteBuffer data, int section, StringBuilder text) throws IOException {
        // Only structural fields from debuggerd/proto/tombstone.proto cross the export boundary.
        // Sections: Tombstone, Signal, thread map entry, Thread, BacktraceFrame.
        long relativePc = 0;
        String library = "[library omitted]";
        while (data.hasRemaining() && text.length() < MAX_TRACE_CHARS) {
            long tag = readVarint(data);
            int field = (int) (tag >>> 3);
            int wire = (int) (tag & 7);
            if (field == 0) {
                throw new IOException("Invalid tombstone field");
            }
            if (wire == 0) {
                long value = readVarint(data);
                if (section == 0 && field == 6) {
                    text.append("Crashing thread ID: ").append(value).append('\n');
                } else if (section == 1 && (field == 1 || field == 3)) {
                    text.append(field == 1 ? "Signal: " : "Signal code: ").append((int) value).append('\n');
                } else if (section == 3 && field == 1) {
                    text.append("Thread ID: ").append(value).append('\n');
                } else if (section == 4 && field == 1) {
                    relativePc = value;
                }
            } else if (wire == 2) {
                long size = readVarint(data);
                if (size < 0 || size > Integer.MAX_VALUE) {
                    throw new IOException("Invalid tombstone length");
                }
                ByteBuffer child = data.slice();
                child.limit((int) Math.min(size, data.remaining()));
                data.position(data.position() + child.remaining());
                if (section == 0 && field == 10) {
                    appendTombstone(child, 1, text);
                } else if (section == 0 && field == 16) {
                    appendTombstone(child, 2, text);
                } else if (section == 2 && field == 2) {
                    appendTombstone(child, 3, text);
                } else if (section == 3 && field == 4) {
                    appendTombstone(child, 4, text);
                } else if (section == 4 && field == 6 && size == child.remaining() && size <= 512) {
                    String path = StandardCharsets.UTF_8.decode(child).toString();
                    String basename = path.substring(path.lastIndexOf('/') + 1);
                    if (NATIVE_LIBRARIES.contains(basename)) {
                        library = basename;
                    }
                }
            } else if ((wire == 1 && data.remaining() >= 8) || (wire == 5 && data.remaining() >= 4)) {
                data.position(data.position() + (wire == 1 ? 8 : 4));
            } else {
                throw new IOException("Invalid tombstone wire type");
            }
        }
        if (section == 4) {
            text.append("  pc ").append(Long.toHexString(relativePc)).append(' ').append(library).append('\n');
        }
    }

    private static long readVarint(ByteBuffer data) throws IOException {
        long result = 0;
        for (int shift = 0; shift < 64 && data.hasRemaining(); shift += 7) {
            int value = data.get() & 0xff;
            result |= (long) (value & 0x7f) << shift;
            if ((value & 0x80) == 0) {
                return result;
            }
        }
        throw new IOException("Incomplete tombstone varint");
    }
}
