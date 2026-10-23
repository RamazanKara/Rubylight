package com.limelight;

import android.app.Application;

import com.limelight.utils.StreamHealth;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameHealthTextTest {
    private static String evidence(boolean hostIncludesSend) {
        StreamHealth health = new StreamHealth();
        for (int n = 1; n <= 3; n++) {
            health.add(new StreamHealth.Sample(n * 1000, true, 60, 0, 1, 4, 5, 2, 60, 60, 60, 0, hostIncludesSend));
        }
        return Game.healthText(RuntimeEnvironment.getApplication(), health.report(3000), "connection").toString();
    }

    @Test
    public void hostEvidenceNamesTheSendOnlyWhenTheHostTimingReportsMeasuredIt() {
        String withSend = evidence(true);
        assertTrue(withSend, withSend.contains("Host processing + send p95: 5.0 ms"));
        String withoutSend = evidence(false);
        assertTrue(withoutSend, withoutSend.contains("Host processing p95: 5.0 ms"));
        assertFalse(withoutSend.contains("send"));
    }
}
