package com.limelight.utils;

import org.junit.Test;

import java.io.ByteArrayInputStream;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;

import static org.junit.Assert.*;

public class AnrTraceTest {
    private static final String TRACE = String.join("\n",
            "----- pid 21100 at 2026-11-03 19:43:54.120 -----",
            "Cmd line: com.butterpollo.client",
            "Build fingerprint: 'OnePlus/OPD2403/OPD2403:16/secret-build/1:user/release-keys'",
            "Subject: Input dispatching timed out (8f1c2a3 com.butterpollo.client/com.limelight.Game (server) is not responding. Waited 5001ms for MotionEvent from desktop.local)",
            "",
            "DALVIK THREADS (3):",
            "\"main\" prio=10 tid=1 Blocked",
            "  | group=\"main\" sCount=1 ucsCount=0 flags=1 obj=0x72a1b2c8 self=0xb400007a1c2d3e40",
            "  | sysTid=21100 nice=-10 cgrp=top-app sched=0/0 handle=0x7b5e6f7a80",
            "  at com.limelight.binding.video.VulkanRendererBridge.stop(VulkanRendererBridge.java:271)",
            "  - waiting to lock <0x0a1b2c3d> (a com.limelight.binding.video.VulkanRendererBridge) held by thread 45",
            "  at com.limelight.binding.video.MediaCodecDecoderRenderer.prepareForStop(MediaCodecDecoderRenderer.java:3833)",
            "  at okhttp3.internal.SecretClient.call(SecretClient.kt:12)",
            "  at com.limelight.GameConnection.destroyVideoSurface(GameConnection.java:1376)",
            "  at android.view.SurfaceView.notifySurfaceDestroyed(SurfaceView.java:2071)",
            "  at android.os.Looper.loop(Looper.java:317)",
            "  at com.android.internal.os.ZygoteInit.main(ZygoteInit.java:932)",
            "",
            "\"Polling thread for MyGamingPC\" prio=5 tid=33 Native",
            "  | group=\"main\" sCount=1 ucsCount=0 flags=1 obj=0x13c0a3f0 self=0xb400007a1c2d3e41",
            "  native: #00 pc 000000000009d5b8  /apex/com.android.runtime/lib64/bionic/libc.so (__ioctl+8) (BuildId: 1a2b3c4d)",
            "  at java.net.SocketInputStream.socketRead0(Native method)",
            "  at org.secretlib.Poller.poll(Poller.java:44)",
            "",
            "\"Stop stream\" prio=5 tid=45 Native",
            "  | group=\"main\" sCount=1 ucsCount=0 flags=1 obj=0x13c0a3f8 self=0xb400007a1c2d3e42",
            "  native: #00 pc 0000000000058d10  /apex/com.android.runtime/lib64/bionic/libc.so (syscall+32) (BuildId: 1a2b3c4d)",
            "  native: #01 pc 000000000004a2c0  /data/app/~~Zx9_private==/com.butterpollo.client-Ab12==/lib/arm64/libvulkan_renderer.so (VulkanRenderer::stop()+96) (BuildId: 5e6f)",
            "  native: #02 pc 0000000000012340  /vendor/lib64/hw/vulkan.adreno.so (secret_vendor_fn+12) (BuildId: 77)",
            "  at com.limelight.binding.video.VulkanRendererBridge.nativeStop(Native method)",
            "  at com.limelight.binding.video.VulkanRendererBridge.stop(VulkanRendererBridge.java:271)",
            "  - locked <0x0a1b2c3d> (a com.limelight.binding.video.VulkanRendererBridge)",
            "",
            "----- end 21100 -----",
            "");

    private static String redact(String trace) throws Exception {
        return AnrTrace.redact(new ByteArrayInputStream(trace.getBytes(StandardCharsets.UTF_8)));
    }

    @Test
    public void mainThreadAndTheLockHolderComeFirstWithFramesOnly() throws Exception {
        String text = redact(TRACE);
        assertTrue(text, text.contains("Cause: input dispatching timed out in com.limelight.Game"));
        assertTrue(text, text.contains("Thread tid=1 Blocked: main\n" +
                "  at com.limelight.binding.video.VulkanRendererBridge.stop(line 271)\n" +
                "  - waiting to lock (a com.limelight.binding.video.VulkanRendererBridge) held by tid=45\n" +
                "  at com.limelight.binding.video.MediaCodecDecoderRenderer.prepareForStop(line 3833)\n" +
                "  at com.limelight.GameConnection.destroyVideoSurface(line 1376)\n"));
        assertTrue(text, text.contains("native #01 pc 000000000004a2c0 libvulkan_renderer.so (VulkanRenderer::stop())"));
        assertTrue(text, text.contains("native #02 pc 0000000000012340 [library omitted]\n"));
        assertTrue(text, text.contains("at com.limelight.binding.video.VulkanRendererBridge.nativeStop(native)"));
        // The lock holder follows the main thread; a thread without app code is left out.
        assertTrue(text, text.indexOf("Thread tid=1 ") < text.indexOf("Thread tid=45 "));
        assertFalse(text, text.contains("tid=33"));
        for (String secret : new String[] {"MyGamingPC", "desktop.local", "secret", "okhttp", "Zx9_private",
                "com.butterpollo.client-", "0x0a1b2c3d", "0xb400", "OPD2403", "8f1c2a3", "/data/", "/vendor/"}) {
            assertFalse(secret + " in\n" + text, text.contains(secret));
        }
    }

    @Test
    public void unknownThreadNamesAreOmittedAndATraceWithoutMainSaysSo() throws Exception {
        String text = redact(TRACE.replace("\"Stop stream\"", "\"Connect to MyGamingPC\""));
        assertTrue(text, text.contains("Thread tid=45 Native: [name omitted]"));
        assertFalse(text, text.contains("MyGamingPC"));
        assertTrue(redact("garbage\n\"x\" prio=5 tid=9 Native\n").contains("No main thread in the trace."));
    }

    @Test(timeout = 5000)
    public void hugeTracesStayBoundedInInputAndOutput() throws Exception {
        StringBuilder big = new StringBuilder("\"main\" prio=5 tid=1 Native\n");
        for (int i = 0; i < 500; i++) big.append("  at com.limelight.Deep.call").append(i).append("(Deep.java:").append(i).append(")\n");
        big.append('\n');
        for (int t = 2; t < 2000; t++) {
            big.append("\"Thread-").append(t).append("\" prio=5 tid=").append(t).append(" Waiting\n");
            for (int i = 0; i < 40; i++) big.append("  at com.limelight.Worker.run(Worker.java:").append(i).append(")\n");
            big.append('\n');
        }
        byte[] bytes = big.toString().getBytes(StandardCharsets.UTF_8);
        assertTrue(bytes.length > AnrTrace.MAX_INPUT_BYTES);
        int[] read = {0};
        InputStream counting = new ByteArrayInputStream(bytes) {
            @Override
            public synchronized int read(byte[] b, int off, int len) {
                int n = super.read(b, off, len);
                if (n > 0) read[0] += n;
                return n;
            }
        };
        String text = AnrTrace.redact(counting);
        assertTrue(read[0] <= AnrTrace.MAX_INPUT_BYTES);
        assertTrue(text.length() <= AnrTrace.MAX_CHARS + 64);
        assertTrue(text.contains("Deep.call63(line 63)"));
        assertFalse(text.contains("Deep.call64("));
        assertTrue(text.contains("[further threads omitted]"));
    }
}
