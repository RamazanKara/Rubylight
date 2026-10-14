package com.limelight.utils;

import android.app.Application;

import com.limelight.R;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.Locale;

import static org.junit.Assert.*;

/** The connection test's result text: units on every value, loss named as loss, and why speed is missing. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class ConnectionTestResultTest {
    private final Locale original = Locale.getDefault();

    @After public void restoreLocale() {
        Locale.setDefault(original);
    }

    private static ConnectionTestStats samples(double... milliseconds) {
        ConnectionTestStats stats = new ConnectionTestStats();
        for (double value : milliseconds) stats.add(value);
        return stats;
    }

    @Test public void delayLineHasUnitsAndCallsLossLoss() {
        Locale.setDefault(Locale.US);
        Application app = RuntimeEnvironment.getApplication();
        assertEquals("Ping 63 ms · Jitter 41 ms · Loss 0.0%",
                PyroWaveBandwidthTest.result(app, samples(42.5, 83.5), null));
        // Below 10 ms a tenth of a millisecond still tells two links apart.
        assertEquals("Ping 2.5 ms · Jitter 1.0 ms · Loss 25.0%",
                PyroWaveBandwidthTest.result(app, samples(2, 3, Double.NaN, 2.5), null));
        assertEquals("Ping – · Jitter – · Loss 100.0%",
                PyroWaveBandwidthTest.result(app, samples(Double.NaN), null));
    }

    @Test public void speedFollowsOnItsOwnLineOrSaysWhyItIsMissing() {
        Locale.setDefault(Locale.US);
        Application app = RuntimeEnvironment.getApplication();
        assertEquals("Ping 63 ms · Jitter 41 ms · Loss 0.0%\nSpeed 312.4 Mbps · PC link 1000 Mbps",
                PyroWaveBandwidthTest.result(app, samples(42.5, 83.5), PyroWaveBandwidthTest.speedText(app, 312.4, 1000)));
        assertEquals("Speed 87.0 Mbps", PyroWaveBandwidthTest.speedText(app, 87, 0));
        String unpaired = PyroWaveBandwidthTest.result(app, samples(5), app.getString(R.string.connection_test_speed_needs_pairing));
        assertTrue(unpaired, unpaired.endsWith("\nSpeed not measured: pair this PC first"));
    }
}
