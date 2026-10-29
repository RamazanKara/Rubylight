package com.limelight;

import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import static org.junit.Assert.*;

public class StreamExitPolicyTest {
    @Test public void leavingQuitsOnlyAGameThePlayerChoseToQuit() {
        assertTrue(StreamExitPolicy.quitsOnLeave(true, NvApp.Role.STREAM, true));
        assertFalse(StreamExitPolicy.quitsOnLeave(false, NvApp.Role.STREAM, true));
    }

    @Test public void aStreamThatIsNoLongerConnectedLeavesWithoutQuitting() {
        assertFalse(StreamExitPolicy.quitsOnLeave(true, NvApp.Role.STREAM, false));
    }

    @Test public void remoteMonitorAndInputSessionsAreNeverQuitOnLeave() {
        assertFalse(StreamExitPolicy.quitsOnLeave(true, NvApp.Role.REMOTE_MONITOR, true));
        assertFalse(StreamExitPolicy.quitsOnLeave(true, NvApp.Role.INPUT_ONLY, true));
    }

    @Test public void theHostEndingTheStreamNeverNeedsADialog() {
        assertFalse(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION, false, false, false));
        assertFalse(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION, true, false, false));
    }

    @Test public void aLostConnectionOrAHostThatWentAwayReturnsToTheList() {
        // -1: ENet disconnect or timeout with no reason; the rest are socket errors (ETIMEDOUT,
        // ECONNRESET, WSAETIMEDOUT) and the Java transport's own failures.
        for (int code : new int[] {-1, 32, 54, 104, 110, 111, 113, 10053, 10054, 10060}) {
            assertFalse("code " + code, StreamExitPolicy.terminationNeedsAction(code, false, false, false));
        }
    }

    @Test public void aReasonOnlyThePcKnowsReturnsToTheList() {
        // Host termination reasons: the HTTP-like codes some hosts send and NVST_DISCONN_* values.
        for (int code : new int[] {503, 404, 0x80030020, 0xc0000005}) {
            assertFalse("code " + code, StreamExitPolicy.terminationNeedsAction(code, false, false, false));
        }
    }

    @Test public void errorsWithAdviceKeepTheDialog() {
        assertTrue(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC, false, false, false));
        assertTrue(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_NO_VIDEO_FRAME, false, false, false));
        assertTrue(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION, false, false, false));
        assertTrue(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_PROTECTED_CONTENT, false, false, false));
        assertTrue(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_FRAME_CONVERSION, false, false, false));
        // A failed decoder says what to change before the stream starts again.
        assertTrue(StreamExitPolicy.terminationNeedsAction(MoonBridge.CLIENT_ERROR_DECODER, false, false, false));
    }

    @Test public void aDecoderGivenUpWithVulkanOnNamesTheSettingInEveryLanguage() throws Exception {
        assertEquals(R.string.decoder_failed_vulkan, StreamExitPolicy.decoderFailureMessage(true));
        assertEquals(R.string.decoder_failed, StreamExitPolicy.decoderFailureMessage(false));
        // The TV's 4K HEVC decoder failed into Vulkan's buffers: the dialog tells the player which
        // switch to turn off, by the name the settings show in that language.
        File[] dirs = new File("src/main/res").listFiles((dir, name) ->
                name.matches("values(-[a-z]{2}(-r[A-Z]{2})?)?") && new File(dir, name + "/strings.xml").exists());
        assertNotNull(dirs);
        assertEquals(15, dirs.length);
        for (File dir : dirs) {
            String strings = new String(Files.readAllBytes(new File(dir, "strings.xml").toPath()), StandardCharsets.UTF_8);
            String title = string(strings, "title_vulkan_render");
            String message = string(strings, "decoder_failed_vulkan");
            assertNotNull(dir.getName(), title);
            assertNotNull(dir.getName(), message);
            assertTrue(dir.getName() + ": " + message, message.contains(title));
            assertFalse(dir.getName(), string(strings, "decoder_failed").contains(title));
        }
    }

    private static String string(String xml, String name) {
        Matcher m = Pattern.compile("<string name=\"" + name + "\"[^>]*>([^<]*)</string>").matcher(xml);
        return m.find() ? m.group(1) : null;
    }

    @Test public void aBlockedPortTestKeepsTheDialogForAnyError() {
        assertTrue(StreamExitPolicy.terminationNeedsAction(-1, true, false, false));
        assertTrue(StreamExitPolicy.terminationNeedsAction(110, true, false, false));
        assertTrue(StreamExitPolicy.terminationNeedsAction(503, true, false, false));
    }

    @Test public void aGraceTimeThatMayHaveRunOutKeepsTheDialogForItsAdvice() {
        assertTrue(StreamExitPolicy.terminationNeedsAction(-1, false, true, false));
        assertTrue(StreamExitPolicy.terminationNeedsAction(110, false, true, false));
        assertFalse(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION, false, true, true));
    }

    @Test public void usedUpAutomaticReconnectingKeepsTheDialogForItsReconnectButton() {
        assertTrue(StreamExitPolicy.terminationNeedsAction(-1, false, false, true));
        assertTrue(StreamExitPolicy.terminationNeedsAction(110, false, false, true));
        assertFalse(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION, false, false, true));
    }
}
