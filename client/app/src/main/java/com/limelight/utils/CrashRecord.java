package com.limelight.utils;

import android.app.ApplicationExitInfo;

import com.limelight.BuildConfig;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.text.SimpleDateFormat;
import java.util.Collections;
import java.util.Date;
import java.util.IdentityHashMap;
import java.util.Locale;
import java.util.Set;
import java.util.TimeZone;

final class CrashRecord {
    static final int MAX_BYTES = 16 * 1024;
    private static final int MAX_THROWABLES = 16;
    private static final int MAX_FRAMES = 128;
    final long timestamp;
    final String id;
    final String text;

    private CrashRecord(long timestamp, String id, String text) {
        this.timestamp = timestamp;
        this.id = id;
        this.text = text;
    }

    static CrashRecord javaCrash(long timestamp, int pid, String version, int api, String model,
                                 String thread, Throwable exception) {
        StringBuilder details = new StringBuilder("Thread: ").append(RedactedLog.crashThread(thread)).append('\n');
        Set<Throwable> seen = Collections.newSetFromMap(new IdentityHashMap<>());
        appendException(details, exception, "", seen);
        return create(timestamp, pid, "JAVA_CRASH", version, api, model, details.toString());
    }

    private static void appendException(StringBuilder text, Throwable exception, String label, Set<Throwable> seen) {
        if (exception == null || seen.size() >= MAX_THROWABLES || text.length() >= MAX_BYTES / 4 || !seen.add(exception)) {
            return;
        }
        // Throwable.toString() and printStackTrace() include messages supplied by remote hosts.
        text.append(label).append(exception.getClass().getName()).append('\n');
        StackTraceElement[] frames = exception.getStackTrace();
        for (int i = 0; i < Math.min(frames.length, MAX_FRAMES) && text.length() < MAX_BYTES / 4; i++) {
            StackTraceElement frame = frames[i];
            String name = frame.getClassName();
            if (name.startsWith("com.limelight.") ||
                    name.startsWith("android.") || name.startsWith("com.android.") ||
                    name.startsWith("androidx.") || name.startsWith("java.") ||
                    name.startsWith("javax.") || name.startsWith("dalvik.") || name.startsWith("libcore.")) {
                text.append("  at ").append(name).append('.').append(frame.getMethodName())
                        .append(frame.isNativeMethod() ? "(native)" : "(line " + frame.getLineNumber() + ")").append('\n');
            }
        }
        for (Throwable suppressed : exception.getSuppressed()) {
            if (seen.size() >= MAX_THROWABLES || text.length() >= MAX_BYTES / 4) {
                break;
            }
            appendException(text, suppressed, "Suppressed: ", seen);
        }
        appendException(text, exception.getCause(), "Caused by: ", seen);
    }

    static String exitReason(int reason) {
        switch (reason) {
            case ApplicationExitInfo.REASON_CRASH_NATIVE:
                return "NATIVE_CRASH";
            case ApplicationExitInfo.REASON_ANR:
                return "ANR";
            default:
                // Java crashes are already recorded by the handler, with their original identity.
                return null;
        }
    }

    static CrashRecord create(long timestamp, int pid, String reason, String version, int api, String model,
                              String details) {
        String id = timestamp + ":" + pid + ":" + reason;
        SimpleDateFormat date = new SimpleDateFormat("yyyy-MM-dd HH:mm:ss.SSS 'UTC'", Locale.ROOT);
        date.setTimeZone(TimeZone.getTimeZone("UTC"));
        String text = "Crash: " + id + "\nTime: " + date.format(new Date(timestamp)) + "\nReason: " + reason
                + "\n" + BuildConfig.BRAND_NAME + ": " + singleLine(version) + "\nAndroid API: " + api + "\nDevice model: " + singleLine(model)
                + "\nException messages, file paths and free-text diagnostics omitted.\n" + details;
        byte[] bytes = text.getBytes(StandardCharsets.UTF_8);
        if (bytes.length > MAX_BYTES) {
            String marker = "\n[truncated]\n";
            int end = MAX_BYTES - marker.length();
            while ((bytes[end] & 0xc0) == 0x80) {
                end--;
            }
            text = new String(bytes, 0, end, StandardCharsets.UTF_8) + marker;
        }
        return new CrashRecord(timestamp, id, text);
    }

    private static String singleLine(String value) {
        return value.substring(0, Math.min(value.length(), 128)).replaceAll("[\\p{Cntrl}]", " ");
    }

    void save(File directory) throws IOException {
        write(new File(directory, "last-crash.txt"), text);
    }

    static CrashRecord read(File directory) throws IOException {
        String text = readText(new File(directory, "last-crash.txt"), MAX_BYTES);
        int newline = text.indexOf('\n');
        if (!text.startsWith("Crash: ") || newline < 0) {
            return null;
        }
        String id = text.substring(7, newline);
        int separator = id.indexOf(':');
        try {
            return separator < 0 ? null : new CrashRecord(Long.parseLong(id.substring(0, separator)), id, text);
        } catch (NumberFormatException e) {
            return null;
        }
    }

    boolean markShown(File directory) throws IOException {
        File shown = new File(directory, "last-crash-shown");
        if (id.equals(readText(shown, 128))) {
            return false;
        }
        // Persist before showing so activity recreation or process death cannot repeat the prompt.
        write(shown, id);
        return true;
    }

    private static String readText(File file, int limit) throws IOException {
        if (!file.isFile()) {
            return "";
        }
        try (FileInputStream input = new FileInputStream(file)) {
            byte[] bytes = new byte[limit];
            int length = 0;
            int count;
            while (length < limit && (count = input.read(bytes, length, limit - length)) != -1) {
                length += count;
            }
            return new String(bytes, 0, length, StandardCharsets.UTF_8);
        }
    }

    private static void write(File file, String text) throws IOException {
        try (FileOutputStream output = new FileOutputStream(file)) {
            output.write(text.getBytes(StandardCharsets.UTF_8));
            output.getFD().sync();
        }
    }
}
