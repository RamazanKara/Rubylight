package com.limelight.utils;

import android.app.ApplicationExitInfo;

import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;

import static org.junit.Assert.*;

public class CrashRecordTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    @Test
    public void messagesCausesSuppressedMessagesAndFilePathsAreOmitted() {
        IllegalStateException error = new IllegalStateException("https://user:secret@desktop.local/192.168.1.2") {
            @Override public String toString() { throw new AssertionError("Must not format the exception"); }
        };
        error.initCause(new IllegalArgumentException("PIN 1234 at fe80::1234"));
        error.addSuppressed(new RuntimeException("password hunter2"));
        error.setStackTrace(new StackTraceElement[] {
                new StackTraceElement("com.limelight.Game", "start", "/private/desktop.local/Game.java", 42),
                new StackTraceElement("com.limelight.nvstream.NvConnection", "start", "NvConnection.java", 12),
                new StackTraceElement("android.app.ActivityThread", "main", "ActivityThread.java", 123),
                new StackTraceElement("java.lang.Thread", "run", "Thread.java", 10),
                new StackTraceElement("okhttp3.internal.Connection", "connect", "Connection.java", 20)
        });
        String text = CrashRecord.javaCrash(34567, 100, "12.2 (315)", 35, "Pixel 8", "main", error).text;
        assertTrue(text.contains("1970-01-01 00:00:34.567 UTC"));
        assertTrue(text.contains("Nightjar: 12.2 (315)"));
        assertTrue(text.contains("Android API: 35"));
        assertTrue(text.contains("Device model: Pixel 8"));
        assertTrue(text.contains("Thread: main"));
        assertTrue(text.contains("Caused by: java.lang.IllegalArgumentException"));
        assertTrue(text.contains("Suppressed: java.lang.RuntimeException"));
        assertTrue(text.contains("com.limelight.Game.start(line 42)"));
        assertTrue(text.contains("com.limelight.nvstream.NvConnection.start(line 12)"));
        assertTrue(text.contains("android.app.ActivityThread.main(line 123)"));
        assertTrue(text.contains("java.lang.Thread.run(line 10)"));
        for (String secret : new String[] {"secret", "desktop.local", "192.168", "PIN", "1234", "fe80", "hunter2", "okhttp3", "/private/"}) {
            assertFalse(secret, text.contains(secret));
        }
    }

    @Test
    public void urlThreadNamesAreRedactedButGeneratedNamesRemainUseful() {
        assertEquals("pool-2-thread-3", RedactedLog.crashThread("pool-2-thread-3"));
        assertEquals("Thread-12", RedactedLog.crashThread("Thread-12"));
        assertEquals("[name omitted]", RedactedLog.crashThread("OkHttp https://secret@desktop.local"));
        assertEquals("[name omitted]", RedactedLog.crashThread("main\nPIN=1234"));
    }

    @Test(timeout = 2000)
    public void deepCyclicAndLargeStacksStayBounded() {
        RuntimeException root = new RuntimeException("private-host");
        Throwable tail = root;
        for (int i = 0; i < 100; i++) {
            RuntimeException next = new RuntimeException("private-host");
            tail.initCause(next);
            tail = next;
        }
        tail.initCause(root);
        StackTraceElement[] frames = new StackTraceElement[10000];
        Arrays.fill(frames, new StackTraceElement("com.limelight.Game", "frame", "Game.java", 42));
        root.setStackTrace(frames);
        String text = CrashRecord.javaCrash(100, 10, "12.2", 35, "Pixel", "main", root).text;
        assertTrue(text.getBytes(StandardCharsets.UTF_8).length <= CrashRecord.MAX_BYTES);
        assertFalse(text.contains("private-host"));
        assertTrue(text.split("  at ").length <= 129);
    }

    @Test
    public void recordBoundIsInUtf8BytesAndDoesNotSplitCharacters() throws Exception {
        CrashRecord record = CrashRecord.create(100, 10, "NATIVE_CRASH", "12.2", 35, "Pixel", "🎮".repeat(20000));
        assertTrue(record.text.getBytes(StandardCharsets.UTF_8).length <= CrashRecord.MAX_BYTES);
        assertTrue(record.text.endsWith("[truncated]\n"));
        assertFalse(record.text.contains("\ufffd"));
        File directory = temporary.newFolder();
        record.save(directory);
        assertTrue(new File(directory, "last-crash.txt").length() <= CrashRecord.MAX_BYTES);
        assertEquals(record.text, CrashRecord.read(directory).text);
    }

    @Test
    public void shownCrashRemainsDismissedAcrossReloadsAndNewCrashPromptsOnce() throws Exception {
        File directory = temporary.newFolder();
        assertNull(CrashRecord.read(directory));
        CrashRecord first = CrashRecord.javaCrash(100, 10, "12.2", 35, "Pixel", "main", new RuntimeException());
        first.save(directory);
        assertTrue(CrashRecord.read(directory).markShown(directory));
        assertFalse(CrashRecord.read(directory).markShown(directory));
        first.save(directory);
        assertFalse(CrashRecord.read(directory).markShown(directory));

        CrashRecord second = CrashRecord.create(200, 20, "NATIVE_CRASH", "12.2", 35, "Pixel", "Signal: 6\n");
        second.save(directory);
        assertTrue(CrashRecord.read(directory).markShown(directory));
        assertFalse(CrashRecord.read(directory).markShown(directory));
        assertEquals(second.text, CrashRecord.read(directory).text);
        assertEquals(2, directory.list().length);
    }

    @Test
    public void exitReasonMappingExcludesExpectedExitsAndDuplicateJavaCrashes() {
        assertEquals("NATIVE_CRASH", CrashRecord.exitReason(ApplicationExitInfo.REASON_CRASH_NATIVE));
        assertEquals("ANR", CrashRecord.exitReason(ApplicationExitInfo.REASON_ANR));
        for (int reason : new int[] {ApplicationExitInfo.REASON_CRASH, ApplicationExitInfo.REASON_EXIT_SELF,
                ApplicationExitInfo.REASON_LOW_MEMORY,
                ApplicationExitInfo.REASON_USER_REQUESTED, ApplicationExitInfo.REASON_USER_STOPPED,
                ApplicationExitInfo.REASON_SIGNALED, ApplicationExitInfo.REASON_PACKAGE_UPDATED,
                ApplicationExitInfo.REASON_UNKNOWN, 999}) {
            assertNull(CrashRecord.exitReason(reason));
        }
    }
}
