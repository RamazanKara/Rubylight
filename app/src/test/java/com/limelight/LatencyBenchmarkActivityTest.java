package com.limelight;

import android.app.Application;
import android.widget.TextView;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class LatencyBenchmarkActivityTest {
    @Test public void startsWithoutHostAndStopsOnPause() {
        try (ActivityController<LatencyBenchmarkActivity> controller =
                     Robolectric.buildActivity(LatencyBenchmarkActivity.class).setup()) {
            LatencyBenchmarkActivity activity = controller.get();
            assertTrue(((TextView) activity.findViewById(R.id.benchmark_results)).getText().toString()
                    .contains(activity.getString(R.string.latency_unavailable)));
            activity.findViewById(R.id.benchmark_start).performClick();
            assertTrue(ReflectionHelpers.<Boolean>getField(activity, "running"));
            controller.pause();
            assertFalse(ReflectionHelpers.<Boolean>getField(activity, "running"));
        }
    }

    @Test public void finishesAfterTenSecondsWithoutInput() {
        try (ActivityController<LatencyBenchmarkActivity> controller =
                     Robolectric.buildActivity(LatencyBenchmarkActivity.class).setup()) {
            LatencyBenchmarkActivity activity = controller.get();
            activity.findViewById(R.id.benchmark_start).performClick();
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper())
                    .idleFor(java.time.Duration.ofSeconds(10));
            assertFalse(ReflectionHelpers.<Boolean>getField(activity, "running"));
        }
    }

    @Test public void manualStopAndRestartDiscardPreviousSamples() {
        try (ActivityController<LatencyBenchmarkActivity> controller =
                     Robolectric.buildActivity(LatencyBenchmarkActivity.class).setup()) {
            LatencyBenchmarkActivity activity = controller.get();
            activity.findViewById(R.id.benchmark_start).performClick();
            Object first = ReflectionHelpers.getField(activity, "stats");
            activity.findViewById(R.id.benchmark_start).performClick();
            assertFalse(ReflectionHelpers.<Boolean>getField(activity, "running"));
            activity.findViewById(R.id.benchmark_start).performClick();
            assertNotSame(first, ReflectionHelpers.getField(activity, "stats"));
        }
    }

    @Test public void exportAddsLatencyResultsOnlyAfterATestRan() {
        try (ActivityController<LatencyBenchmarkActivity> controller =
                     Robolectric.buildActivity(LatencyBenchmarkActivity.class).setup()) {
            LatencyBenchmarkActivity activity = controller.get();
            String before = ReflectionHelpers.callInstanceMethod(activity, "exportResults");
            assertTrue(before.contains("\"format\": \"nightjar-device-results\""));
            assertTrue(before.contains("\"measured\": false"));
            assertTrue(before.contains("\"latencyTest\": null"));
            activity.findViewById(R.id.benchmark_start).performClick();
            controller.pause();
            String after = ReflectionHelpers.callInstanceMethod(activity, "exportResults");
            assertTrue(after.contains("\"panelHz\": "));
            assertFalse(after.contains("\"latencyTest\": null"));
        }
    }
}
