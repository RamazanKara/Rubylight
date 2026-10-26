package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.nvstream.http.HostCapabilities.Availability;
import com.limelight.nvstream.http.HostCapabilities.Family;
import com.limelight.nvstream.http.HostCapabilities.Feature;
import com.limelight.nvstream.http.HostCapabilities.Reason;

import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.File;
import java.net.URL;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;
import java.util.Objects;
import java.util.TreeMap;
import java.util.TreeSet;

import static org.junit.Assert.*;

/**
 * The capabilities contract shared with nightjar-core: every serverinfo fixture has an
 * expected-features JSON in src/test/resources/protocol/capabilities, and both this test and
 * protocol/client/tests/capabilities_contract.rs assert it, so HostCapabilities and
 * http/capabilities.rs cannot drift apart unnoticed. A feature only one side has is an object
 * {"only": "java" | "rust", "availability": ..., "comment": why}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostCapabilitiesContractTest {
    private static final String SERVERINFO = "-serverinfo.xml";
    private static final String CONTRACT = "-serverinfo.json";

    /** The contract name of each feature; a feature without one fails every contract. */
    private static String featureName(Feature feature) {
        switch (feature) {
            case PERMISSIONS: return "permissions";
            case OTP_PAIRING: return "otp_pairing";
            case CLIPBOARD: return "clipboard";
            case SERVER_COMMANDS: return "server_commands";
            case APP_UUIDS: return "app_uuids";
            case LAUNCH_BY_UUID: return "launch_by_uuid";
            case VIRTUAL_DISPLAY: return "virtual_display";
            case VIRTUAL_DISPLAY_HDR: return "virtual_display_hdr";
            case HOST_DISPLAY_MODES: return "host_display_modes";
            case FRACTIONAL_REFRESH: return "fractional_refresh";
            case HOST_VRR: return "host_vrr";
            case RUNTIME_BITRATE: return "runtime_bitrate";
            case PYROWAVE: return "pyrowave";
            case PYROWAVE_RECORDS: return "pyrowave_records";
            case PYROWAVE_BANDWIDTH_PROBE: return "pyrowave_bandwidth_probe";
            case HARDWARE_METRICS: return "hardware_metrics";
            case NETWORK_PATH: return "network_path";
            case FRAME_LIMITER_STATUS: return "frame_limiter_status";
            case CONTROLLER_PREPARATION: return "controller_preparation";
            case REMOTE_SESSION_TILES: return "remote_session_tiles";
            case HOST_CONFIRMS_REPLACEMENT: return "host_confirms_replacement";
            case HOST_TIMING: return "host_timing";
            case WEB_UI: return "web_ui";
            case CURSOR_SHAPE: return "cursor_shape";
            case NATIVE_API: return "native_api";
            case SLEEP: return "sleep";
            default: return null;
        }
    }

    private static String reasonName(Reason reason) {
        switch (reason) {
            case UPDATE_HOST: return "update_host";
            case PAIR: return "pair";
            case VIEW_PERMISSION: return "view_permission";
            case CLIPBOARD_PERMISSION: return "clipboard_permission";
            case COMMAND_PERMISSION: return "command_permission";
            case NO_COMMANDS: return "no_commands";
            case VIRTUAL_DISPLAY_OFF: return "virtual_display_off";
            case VIRTUAL_DISPLAY_DRIVER: return "virtual_display_driver";
            case HARDWARE_METRICS_OFF: return "hardware_metrics_off";
            case SLEEP_PERMISSION: return "sleep_permission";
            default: throw new AssertionError("Reason " + reason + " has no contract name");
        }
    }

    private static String familyName(Family family) {
        switch (family) {
            case GAMESTREAM: return "gamestream";
            case SUNSHINE: return "sunshine";
            case APOLLO: return "apollo";
            case VIBEPOLLO: return "vibepollo";
            case RUBYLIGHT: return "rubylight";
            case UNKNOWN: return "unknown";
            default: throw new AssertionError("Family " + family + " has no contract name");
        }
    }

    private static String availabilityName(Availability availability) {
        switch (availability.state) {
            case SUPPORTED: return "supported";
            case UNKNOWN: return "unknown";
            default: return availability.reason == null ? "unsupported" : "unsupported:" + reasonName(availability.reason);
        }
    }

    private static File resourceDir(String path) throws Exception {
        URL url = HostCapabilitiesContractTest.class.getResource(path);
        assertNotNull("No resource folder " + path, url);
        return new File(url.toURI());
    }

    private static TreeSet<String> names(File dir, String suffix) {
        TreeSet<String> names = new TreeSet<>();
        for (String name : Objects.requireNonNull(dir.list(), dir.toString())) {
            if (name.endsWith(suffix)) {
                names.add(name.substring(0, name.length() - suffix.length()));
            }
        }
        return names;
    }

    /** The features this side must have, with their availability; Rust-only ones are left out. */
    private static TreeMap<String, String> expectedForJava(String fixture, JSONObject features) throws Exception {
        TreeMap<String, String> expected = new TreeMap<>();
        for (Iterator<String> keys = features.keys(); keys.hasNext(); ) {
            String name = keys.next();
            JSONObject entry = features.optJSONObject(name);
            if (entry == null) {
                expected.put(name, features.getString(name));
                continue;
            }
            String only = entry.optString("only");
            assertTrue(fixture + ": " + name + " needs \"only\": \"java\" or \"rust\"",
                    only.equals("java") || only.equals("rust"));
            assertFalse(fixture + ": platform-specific " + name + " needs a comment",
                    entry.optString("comment").isEmpty());
            if (only.equals("java")) {
                expected.put(name, entry.getString("availability"));
            }
        }
        return expected;
    }

    @Test
    public void javaCapabilitiesMatchTheSharedContract() throws Exception {
        File dir = resourceDir("/protocol/capabilities");
        List<String> drift = new ArrayList<>();
        for (String fixture : names(dir, CONTRACT)) {
            JSONObject contract = new JSONObject(TestHosts.serverInfo("capabilities/" + fixture + CONTRACT));
            assertEquals(fixture + ": serverinfo field", fixture + SERVERINFO, contract.getString("serverinfo"));
            HostCapabilities caps = TestHosts.fixture(fixture + SERVERINFO);
            if (!familyName(caps.family()).equals(contract.getString("family"))) {
                drift.add(fixture + ": family " + familyName(caps.family()) + " != " + contract.getString("family"));
            }
            String hostVersion = contract.isNull("hostVersion") ? null : contract.getString("hostVersion");
            if (!Objects.equals(caps.hostVersion(), hostVersion)) {
                drift.add(fixture + ": hostVersion " + caps.hostVersion() + " != " + hostVersion);
            }
            TreeMap<String, String> expected = expectedForJava(fixture, contract.getJSONObject("features"));
            for (Feature feature : Feature.values()) {
                String name = featureName(feature);
                if (name == null) {
                    drift.add(fixture + ": " + feature + " has no contract name");
                    continue;
                }
                String actual = availabilityName(caps.availability(feature));
                String want = expected.remove(name);
                if (want == null) {
                    drift.add(fixture + ": " + name + " is not in the contract");
                } else if (!want.equals(actual)) {
                    drift.add(fixture + ": " + name + " " + actual + " != " + want);
                }
            }
            for (String name : expected.keySet()) {
                drift.add(fixture + ": " + name + " is in the contract but not in Feature");
            }
        }
        assertTrue("capabilities drift:\n" + String.join("\n", drift), drift.isEmpty());
    }

    @Test
    public void everyServerInfoFixtureHasAContract() throws Exception {
        TreeSet<String> fixtures = names(resourceDir("/protocol"), SERVERINFO);
        assertFalse(fixtures.isEmpty());
        assertEquals(fixtures, names(resourceDir("/protocol/capabilities"), CONTRACT));
    }
}
