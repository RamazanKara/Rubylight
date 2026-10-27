package com.limelight.computers;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;

import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostCapabilityCacheTest {
    private final Context context = RuntimeEnvironment.getApplication();

    @Test
    public void lastReportedCapabilitiesSurviveUntilThePcIsForgotten() {
        assertSame(HostCapabilities.UNKNOWN, HostCapabilityCache.load(context, "pc"));
        HostCapabilityCache.save(context, "pc", TestHosts.rubylight());
        HostCapabilityCache.save(context, "other", TestHosts.fixture("sunshine-serverinfo.xml"));
        assertEquals(TestHosts.rubylight(), HostCapabilityCache.load(context, "pc"));
        assertEquals(HostCapabilities.Family.SUNSHINE, HostCapabilityCache.load(context, "other").family());
        HostCapabilityCache.forget(context, "pc");
        assertSame(HostCapabilities.UNKNOWN, HostCapabilityCache.load(context, "pc"));
        assertEquals(HostCapabilities.Family.SUNSHINE, HostCapabilityCache.load(context, "other").family());
        assertSame(HostCapabilities.UNKNOWN, HostCapabilityCache.load(context, null));
        assertSame(HostCapabilities.UNKNOWN, HostCapabilityCache.load(null, "other"));
    }

    @Test
    public void unknownCapabilitiesNeverReplaceWhatAPcReported() {
        HostCapabilityCache.save(context, "pc", TestHosts.rubylight());
        HostCapabilityCache.save(context, "pc", HostCapabilities.UNKNOWN);
        HostCapabilityCache.save(context, null, TestHosts.rubylight());
        assertEquals(TestHosts.rubylight(), HostCapabilityCache.load(context, "pc"));
    }

    @Test
    public void repeatedPollsDoNotRewriteUnchangedCapabilities() {
        SharedPreferences preferences = context.getSharedPreferences(HostCapabilityCache.PREFERENCES, Context.MODE_PRIVATE);
        AtomicInteger writes = new AtomicInteger();
        SharedPreferences.OnSharedPreferenceChangeListener listener = (changed, key) -> writes.incrementAndGet();
        preferences.registerOnSharedPreferenceChangeListener(listener);
        try {
            for (int poll = 0; poll < 5; poll++) {
                HostCapabilityCache.save(context, "pc", TestHosts.rubylight());
            }
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals(1, writes.get());
            HostCapabilityCache.save(context, "pc", TestHosts.fixture("butterpollo-serverinfo.xml"));
            org.robolectric.shadows.ShadowLooper.idleMainLooper();
            assertEquals(2, writes.get());
        } finally {
            preferences.unregisterOnSharedPreferenceChangeListener(listener);
        }
    }

    @Test
    public void damagedEntriesReadAsNeverSeen() {
        context.getSharedPreferences(HostCapabilityCache.PREFERENCES, Context.MODE_PRIVATE).edit()
                .putString("pc", "{not json").commit();
        assertSame(HostCapabilities.UNKNOWN, HostCapabilityCache.load(context, "pc"));
    }
}
