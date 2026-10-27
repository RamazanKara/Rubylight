package com.limelight.computers;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;

import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;

import org.json.JSONException;
import org.json.JSONObject;
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

    /** The 3.0 host with its native capabilities, as the contract reads it. */
    private static HostCapabilities rubylight30() throws JSONException {
        JSONObject contract = new JSONObject(TestHosts.serverInfo("capabilities/rubylight-30-serverinfo.json"));
        return TestHosts.fixture("rubylight-30-serverinfo.xml")
                .withNativeCapabilities(contract.getJSONObject("nativeCapabilities").toString());
    }

    @Test
    public void the30ExtensionsSurviveTheCache() throws JSONException {
        HostCapabilities caps = rubylight30();
        HostCapabilityCache.save(context, "pc", caps);
        HostCapabilities loaded = HostCapabilityCache.load(context, "pc");
        assertEquals(caps, loaded);
        for (HostCapabilities.Feature feature : EXTENSIONS_30) {
            assertTrue(feature.name(), loaded.has(feature));
        }
    }

    @Test
    public void capabilitiesStoredBeforeThe30ExtensionsLoadWithoutThem() throws JSONException {
        JSONObject stored = new JSONObject(rubylight30().toJson());
        JSONObject defaults = new JSONObject(stored.toString());
        for (String key : STORED_VERSIONS_30) {
            assertNotNull(key, stored.remove(key));
            defaults.put(key, -1);
        }
        for (String key : STORED_FLAGS_30) {
            assertNotNull(key, stored.remove(key));
            defaults.put(key, false);
        }
        context.getSharedPreferences(HostCapabilityCache.PREFERENCES, Context.MODE_PRIVATE).edit()
                .putString("pc", stored.toString()).commit();
        HostCapabilities older = HostCapabilityCache.load(context, "pc");
        assertTrue(older.isKnown());
        assertEquals(HostCapabilities.fromJson(defaults.toString()), older);
        assertTrue(older.has(HostCapabilities.Feature.FILE_UPLOAD));
        for (HostCapabilities.Feature feature : EXTENSIONS_30) {
            assertTrue(feature.name(), older.availability(feature).isNever());
        }
    }

    private static final HostCapabilities.Feature[] EXTENSIONS_30 = {
            HostCapabilities.Feature.DOLBY_VISION, HostCapabilities.Feature.HLG, HostCapabilities.Feature.CLIENT_FEC,
            HostCapabilities.Feature.BANDWIDTH_PROBE, HostCapabilities.Feature.USB_FORWARDING,
            HostCapabilities.Feature.AUDIO_PASSTHROUGH, HostCapabilities.Feature.SURROUND_714,
            HostCapabilities.Feature.TEXT_INPUT, HostCapabilities.Feature.CLIENT_KEYBOARD_LAYOUT,
            HostCapabilities.Feature.PEN_BARREL_ROLL, HostCapabilities.Feature.PIPELINE_RATES,
            HostCapabilities.Feature.IMAGE_CLIPBOARD, HostCapabilities.Feature.SHARED_FOLDERS};
    private static final String[] STORED_VERSIONS_30 = {"hdrFormats", "fecControlVersion", "bandwidthProbeVersion",
            "audioCodecs", "audioLayouts", "textInputVersion", "keyboardLayoutVersion", "penBarrelRollVersion",
            "pipelineRatesVersion"};
    private static final String[] STORED_FLAGS_30 = {"usbOffered", "usbPermission", "imageClipboardOffered",
            "foldersOffered", "folderPermission"};

    @Test
    public void damagedEntriesReadAsNeverSeen() {
        context.getSharedPreferences(HostCapabilityCache.PREFERENCES, Context.MODE_PRIVATE).edit()
                .putString("pc", "{not json").commit();
        assertSame(HostCapabilities.UNKNOWN, HostCapabilityCache.load(context, "pc"));
    }
}
