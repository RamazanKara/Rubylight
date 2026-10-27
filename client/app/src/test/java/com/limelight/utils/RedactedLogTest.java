package com.limelight.utils;

import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Test;
import static org.junit.Assert.*;

public class RedactedLogTest {
    @Test public void audioBackendSelectionSurvivesReportFiltering() {
        RedactedLog log = new RedactedLog();
        log.add("INFO", "Audio output: AAudio low latency");
        log.add("INFO", "Audio track configuration: 1920 false");
        for (int i = 0; i < 500; i++) log.add("INFO", "Too much pending audio data: 45 ms");
        assertEquals("INFO Audio output: AAudio low latency\nINFO Audio track configuration: 1920 false", log.snapshot());
    }

    @Test public void streamingEngineSelectionSurvivesReportFiltering() {
        RedactedLog log = new RedactedLog();
        log.add("INFO", "Streaming engine: Rust (nightjar-core)");
        for (int i = 0; i < 500; i++) log.add("INFO", "frame 12345 queued");
        assertEquals("INFO Streaming engine: Rust (nightjar-core)", log.snapshot());
        log.add("WARNING", "Streaming engine: C (moonlight-common-c), Rust engine requested but unavailable: the host reports no codec support");
        assertTrue(log.snapshot().contains("Rust engine requested but unavailable: the host reports no codec support"));
    }

    @Test
    public void displayRefreshDiagnosticsRemainInTheSharedReport() {
        RedactedLog log = new RedactedLog();
        String diagnostic = new com.limelight.binding.video.DisplayRefreshDiagnostics().update(
                36, 0, 2, false, true, false, 0, 0, 120f);
        log.add("INFO", diagnostic);
        for (int i = 0; i < 500; i++) log.add("INFO", "frame 12345 queued");
        assertEquals("INFO " + diagnostic, log.snapshot());
        assertTrue(log.snapshot().contains("arrSupported=false, selected=max Hz"));
    }

    @Test
    public void keepsControllerSetupAndFirstFeedbackWithoutHighRateInputNoise() {
        RedactedLog log = new RedactedLog();
        log.add("INFO", "DualSense USB waveform backend ready");
        log.add("INFO", "USB controller announcement queued: slot=0, type=2, capabilities=0x803f");
        log.add("INFO", "DualSense USB first adaptive-trigger packet received");
        log.add("INFO", "DualSense USB first waveform packet received");
        log.add("WARNING", "DualSense USB waveform initialization failed while claiming audio control interface");
        for (int i = 0; i < 500; i++) log.add("INFO", "Controller input packet received");
        String snapshot = log.snapshot();
        assertEquals(5, snapshot.split("\n").length);
        assertTrue(snapshot.contains("capabilities=0x803f"));
        assertTrue(snapshot.contains("waveform backend ready"));
        assertTrue(snapshot.contains("first adaptive-trigger packet received"));
        assertTrue(snapshot.contains("first waveform packet received"));
        assertTrue(snapshot.contains("claiming audio control interface"));
    }

    @Test
    public void pairingUuidIsRemovedBeforeItsNumericGroupsCanBeReplaced() {
        assertEquals("Pairing failed for [id] with PIN [omitted]",
                RedactedLog.redact("Pairing failed for 3fa85f64-5717-4562-b3fc-2c963f66afa6 with PIN 1234"));
    }

    @Test
    public void allSavedComputerNamesAndAddressesAreRedactedBeforeStreaming() {
        ComputerDetails computer = new ComputerDetails();
        computer.name = "Private desktop";
        computer.localAddress = new ComputerDetails.AddressTuple("private-pc.lan", 47989);
        computer.remoteAddress = new ComputerDetails.AddressTuple("private-remote.example", 47989);
        computer.manualAddress = new ComputerDetails.AddressTuple("private-manual.local", 47989);
        computer.ipv6Address = new ComputerDetails.AddressTuple("fd01::5", 47989);
        computer.activeAddress = new ComputerDetails.AddressTuple("private-active.lan", 47989);
        RedactedLog.addSecret("private-pc");
        RedactedLog.addComputerSecrets(computer);
        assertEquals("[PC] is offline", RedactedLog.redact("Private desktop is offline"));
        for (String address : new String[] {"private-pc.lan", "private-remote.example", "private-manual.local",
                "fd01::5", "private-active.lan"}) {
            assertEquals("Unable to resolve host [PC]", RedactedLog.redact("Unable to resolve host " + address));
        }
        RedactedLog log = new RedactedLog();
        log.add("INFO", "Stream starting");
        for (int i = 0; i < 500; i++) log.add("INFO", "PC is offline (try 1)");
        assertEquals("INFO Stream starting", log.snapshot());
    }

    @Test
    public void addressesPinsAndKnownNamesNeverEnterTheExport() {
        RedactedLog.addSecret("desktop.local");
        RedactedLog.addSecret("my-private-host");
        RedactedLog log = new RedactedLog();
        String[] secrets = {"192.168.1.12", "fe80::1234%wlan0", "::ffff:192.168.1.2",
                "AA:BB:CC:DD:EE:FF", "desktop.local", "my-private-host", "https://user:secret@host/pair?pin=0123",
                "PIN 1234", "passphrase hunter2", "Stream connected\nPIN=0000", "Pairing succeeded 0123",
                "3fa85f64-5717-4562-b3fc-2c963f66afa6", "/storage/emulated/0/Download/report.txt"};
        for (String secret : secrets) {
            log.add("SEVERE", "Connection to " + secret + " failed");
        }
        String snapshot = log.snapshot();
        for (String part : new String[] {"192.168", "fe80", "ffff", "AA:BB", "desktop.local", "my-private-host",
                "user:secret", "0123", "1234", "hunter2", "0000", "3fa85f64", "/storage", "report.txt"}) {
            assertFalse(part, snapshot.contains(part));
        }
        assertTrue(snapshot.contains("ERROR Connection to [PC] failed"));
    }

    @Test
    public void ourOwnErrorsAndKeyLinesStayReadable() {
        RedactedLog log = new RedactedLog();
        log.add("SEVERE", "Connection terminated: -102 (ended right after starting)");
        log.add("WARNING", "java.lang.IllegalStateException at MediaCodecDecoderRenderer.queueNextInputBuffer");
        log.add("INFO", "Stream request: 2184x1968 at 120 fps (resolution native, PC profile)");
        log.add("INFO", "Selected HEVC decoder: c2.qti.hevc.decoder");
        log.add("INFO", "frame 12345 queued");
        String snapshot = log.snapshot();
        assertTrue(snapshot.contains("ERROR Connection terminated: -102 (ended right after starting)"));
        assertTrue(snapshot.contains("IllegalStateException at MediaCodecDecoderRenderer.queueNextInputBuffer"));
        assertTrue(snapshot.contains("INFO Stream request: 2184x1968 at 120 fps (resolution native, PC profile)"));
        assertTrue(snapshot.contains("c2.qti.hevc.decoder"));
        assertFalse(snapshot.contains("frame 12345"));
    }

    @Test
    public void fixedEventsRemainUsefulAndOldEntriesAreEvicted() {
        RedactedLog log = new RedactedLog();
        log.add("INFO", "Stream starting");
        for (int i = 0; i < 400; i++) {
            log.add("INFO", "Stream connected");
        }
        assertEquals(400, log.snapshot().split("\n").length);
        assertFalse(log.snapshot().contains("Stream starting"));
        log.add("PIN 1234", "Stream disconnected");
        assertTrue(log.snapshot().endsWith("INFO Stream disconnected"));
        assertFalse(log.snapshot().contains("1234"));
    }
}
