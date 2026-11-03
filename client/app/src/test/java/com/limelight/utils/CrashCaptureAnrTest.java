package com.limelight.utils;

import android.app.ActivityManager;
import android.app.Application;
import android.app.ApplicationExitInfo;
import android.content.Context;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowActivityManager;

import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class CrashCaptureAnrTest {
    // #73: an ANR's record held no stack, so the report could not say where the app hung.
    @Test
    public void anrRecordCarriesTheMainThreadStack() throws Exception {
        Context context = RuntimeEnvironment.getApplication();
        String trace = "Subject: Input dispatching timed out (com.butterpollo.client/com.limelight.Game is not responding)\n" +
                "\"main\" prio=5 tid=1 Blocked\n" +
                "  at com.limelight.binding.video.VulkanRendererBridge.stop(VulkanRendererBridge.java:271)\n" +
                "  - waiting to lock <0x0a1b2c3d> (a com.limelight.binding.video.VulkanRendererBridge) held by thread 45\n" +
                "\n";
        ActivityManager manager = (ActivityManager) context.getSystemService(Context.ACTIVITY_SERVICE);
        shadowOf(manager).addApplicationExitInfo(ShadowActivityManager.ApplicationExitInfoBuilder.newBuilder()
                .setProcessName(context.getPackageName())
                .setPid(21100)
                .setTimestamp(1793735039381L)
                .setReason(ApplicationExitInfo.REASON_ANR)
                .setTraceInputStream(new ByteArrayInputStream(trace.getBytes(StandardCharsets.UTF_8)))
                .build());

        new CrashCapture(context).collectExitReason();

        String report = CrashCapture.lastReport(context);
        assertTrue(report, report.contains("Reason: ANR"));
        assertTrue(report, report.contains("Cause: input dispatching timed out in com.limelight.Game"));
        assertTrue(report, report.contains("Thread tid=1 Blocked: main\n" +
                "  at com.limelight.binding.video.VulkanRendererBridge.stop(line 271)\n" +
                "  - waiting to lock (a com.limelight.binding.video.VulkanRendererBridge) held by tid=45\n"));
    }

    @Test
    public void anrWithoutATraceSaysSo() {
        Context context = RuntimeEnvironment.getApplication();
        ActivityManager manager = (ActivityManager) context.getSystemService(Context.ACTIVITY_SERVICE);
        shadowOf(manager).addApplicationExitInfo(ShadowActivityManager.ApplicationExitInfoBuilder.newBuilder()
                .setProcessName(context.getPackageName())
                .setPid(28878)
                .setTimestamp(1793735693137L)
                .setReason(ApplicationExitInfo.REASON_ANR)
                .build());

        new CrashCapture(context).collectExitReason();

        String report = CrashCapture.lastReport(context);
        assertTrue(report, report.contains("Reason: ANR"));
        assertTrue(report, report.contains("ANR trace unavailable."));
    }
}
