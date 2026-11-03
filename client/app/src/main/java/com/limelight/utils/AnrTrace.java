package com.limelight.utils;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * The thread dump Android keeps for an ANR (ApplicationExitInfo.getTraceInputStream), cut down to
 * where the threads were: frames only, as for crash stacks. The main thread comes first, then the
 * threads holding what it waits for, then other threads running app code.
 */
final class AnrTrace {
    static final int MAX_INPUT_BYTES = 1024 * 1024;
    static final int MAX_CHARS = 12 * 1024;
    private static final int MAIN_FRAMES = 64;
    private static final int OTHER_FRAMES = 24;

    private static final Pattern THREAD = Pattern.compile("^\"(.*)\"(?: daemon)? prio=\\d+ tid=(\\d+) (\\w+)");
    private static final Pattern JAVA_FRAME = Pattern.compile("^\\s+at ([\\w$.]+)\\.([\\w$<>-]+)\\((?:[\\w$.-]+:(\\d+)|(Native method)|.*)\\)$");
    private static final Pattern NATIVE_FRAME = Pattern.compile(
            "^\\s+native: #(\\d+) pc ([0-9a-f]+)\\s+(\\S+)(?: \\((.*?)\\+\\d+\\))?(?: \\(BuildId: [0-9a-f]+\\))?\\s*$");
    private static final Pattern LOCK = Pattern.compile(
            "^\\s+- (waiting to lock|locked|waiting on|sleeping on|parking to wait for) <0x[0-9a-f]+> \\(a ([\\w$.]+)\\)(?: held by thread (\\d+))?");
    // Symbols come from the binaries, not from user data, but anything unusual is still left out.
    private static final Pattern SYMBOL = Pattern.compile("[\\w:~<>,*&()\\[\\] .-]{1,160}");

    private static final class Dump {
        final String name;
        final int tid;
        final String state;
        final List<String> lines = new ArrayList<>();
        final Set<Integer> waitsFor = new LinkedHashSet<>();
        int frames;
        boolean appCode;

        Dump(String name, int tid, String state) {
            this.name = name;
            this.tid = tid;
            this.state = state;
        }
    }

    private AnrTrace() {
    }

    static String redact(InputStream input) throws IOException {
        StringBuilder text = new StringBuilder("ANR trace (redacted; frames only):\n");
        List<Dump> threads = new ArrayList<>();
        Dump current = null;
        BufferedReader reader = new BufferedReader(new InputStreamReader(new BoundedInput(input), StandardCharsets.UTF_8));
        String line;
        while ((line = reader.readLine()) != null) {
            if (line.startsWith("Subject: ")) {
                text.append("Cause: ").append(cause(line)).append('\n');
                continue;
            }
            Matcher thread = THREAD.matcher(line);
            if (thread.find()) {
                current = new Dump(thread.group(1), Integer.parseInt(thread.group(2)), thread.group(3));
                threads.add(current);
                continue;
            }
            if (current == null) {
                continue;
            }
            if (line.isEmpty()) {
                current = null;
                continue;
            }
            int limit = current.tid == 1 ? MAIN_FRAMES : OTHER_FRAMES;
            Matcher java = JAVA_FRAME.matcher(line);
            if (java.matches()) {
                if (current.frames++ >= limit || !CrashRecord.keepsClass(java.group(1))) {
                    continue;
                }
                current.appCode |= java.group(1).startsWith("com.limelight.");
                current.lines.add("  at " + java.group(1) + '.' + java.group(2) +
                        (java.group(3) != null ? "(line " + java.group(3) + ")" : java.group(4) != null ? "(native)" : ""));
                continue;
            }
            Matcher lock = LOCK.matcher(line);
            if (lock.find()) {
                String type = CrashRecord.keepsClass(lock.group(2)) ? lock.group(2) : "[class omitted]";
                String held = lock.group(3);
                if (held != null) {
                    current.waitsFor.add(Integer.parseInt(held));
                }
                if (current.frames <= limit) {
                    current.lines.add("  - " + lock.group(1) + " (a " + type + ")" + (held != null ? " held by tid=" + held : ""));
                }
                continue;
            }
            Matcher frame = NATIVE_FRAME.matcher(line);
            if (frame.matches()) {
                if (current.frames++ >= limit) {
                    continue;
                }
                String path = frame.group(3);
                String library = path.substring(path.lastIndexOf('/') + 1);
                boolean known = RedactedLog.NATIVE_LIBRARIES.contains(library);
                String symbol = frame.group(4);
                current.appCode |= library.equals("libvulkan_renderer.so") || library.equals("libmoonlight-core.so") ||
                        library.equals("libnightjar_core.so") || library.startsWith("libpyrowave");
                current.lines.add("  native #" + frame.group(1) + " pc " + frame.group(2) + ' ' +
                        (known ? library : "[library omitted]") +
                        (known && symbol != null && SYMBOL.matcher(symbol).matches() ? " (" + symbol + ")" : ""));
            }
        }

        Dump main = null;
        for (Dump dump : threads) {
            if (dump.tid == 1) {
                main = dump;
                break;
            }
        }
        if (main == null) {
            text.append("No main thread in the trace.\n");
            return text.toString();
        }
        // The main thread, the threads it waits for (and theirs), then other threads in app code
        List<Dump> order = new ArrayList<>();
        order.add(main);
        for (int i = 0; i < order.size() && i < 8; i++) {
            for (int tid : order.get(i).waitsFor) {
                for (Dump dump : threads) {
                    if (dump.tid == tid && !order.contains(dump)) {
                        order.add(dump);
                    }
                }
            }
        }
        for (Dump dump : threads) {
            if (dump.appCode && !order.contains(dump)) {
                order.add(dump);
            }
        }
        for (Dump dump : order) {
            StringBuilder block = new StringBuilder("Thread tid=").append(dump.tid).append(' ').append(dump.state)
                    .append(": ").append(RedactedLog.crashThread(dump.name)).append('\n');
            for (String frameLine : dump.lines) {
                block.append(frameLine).append('\n');
            }
            if (text.length() + block.length() > MAX_CHARS) {
                text.append("[further threads omitted]\n");
                break;
            }
            text.append(block);
        }
        return text.toString();
    }

    // The kind of ANR, from the subject line, without the window titles and intents it names.
    private static String cause(String subject) {
        String kind;
        if (subject.contains("Input dispatching timed out")) {
            kind = "input dispatching timed out";
        } else if (subject.contains("Broadcast of Intent")) {
            kind = "broadcast timed out";
        } else if (subject.contains("executing service") || subject.contains("startForegroundService")) {
            kind = "service timed out";
        } else if (subject.contains("ContentProvider")) {
            kind = "content provider timed out";
        } else {
            kind = "other";
        }
        Matcher activity = Pattern.compile("com\\.limelight\\.[\\w.$]+").matcher(subject);
        return activity.find() ? kind + " in " + activity.group() : kind;
    }

    private static final class BoundedInput extends InputStream {
        private final InputStream input;
        private int left = MAX_INPUT_BYTES;

        BoundedInput(InputStream input) {
            this.input = input;
        }

        @Override
        public int read() throws IOException {
            if (left <= 0) {
                return -1;
            }
            int value = input.read();
            if (value >= 0) {
                left--;
            }
            return value;
        }

        @Override
        public int read(byte[] buffer, int offset, int length) throws IOException {
            if (left <= 0) {
                return -1;
            }
            int count = input.read(buffer, offset, Math.min(length, left));
            if (count > 0) {
                left -= count;
            }
            return count;
        }
    }
}
