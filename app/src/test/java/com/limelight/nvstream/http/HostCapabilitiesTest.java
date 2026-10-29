package com.limelight.nvstream.http;

import android.app.Application;
import android.content.Context;
import android.content.res.Configuration;

import com.limelight.R;
import com.limelight.nvstream.http.HostCapabilities.Availability;
import com.limelight.nvstream.http.HostCapabilities.Family;
import com.limelight.nvstream.http.HostCapabilities.Feature;
import com.limelight.nvstream.http.HostCapabilities.Reason;

import org.json.JSONException;
import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.EnumSet;
import java.util.Locale;
import java.util.Set;

import static com.limelight.nvstream.http.HostCapabilities.Feature.*;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostCapabilitiesTest {
    private static final String[] FIXTURES = {"sunshine-serverinfo.xml", "apollo-serverinfo.xml",
            "apollo-http-serverinfo.xml", "vibepollo-serverinfo.xml", "vibepollo-http-serverinfo.xml",
            "gamestream-serverinfo.xml", "rubylight-serverinfo.xml", "butterpollo-serverinfo.xml",
            "butterpollo-viewer-serverinfo.xml"};

    private static HostCapabilities caps(String xml) {
        return TestHosts.caps("<root status_code=\"200\">" + xml + "</root>");
    }

    private static void assertSupported(HostCapabilities caps, Feature... expected) {
        Set<Feature> features = expected.length == 0 ? EnumSet.noneOf(Feature.class) : EnumSet.of(expected[0], expected);
        assertEquals(caps.toString(), features, caps.supported());
    }

    private static void assertNever(HostCapabilities caps, Feature... features) {
        for (Feature feature : features) {
            assertTrue(feature + " " + caps.availability(feature), caps.availability(feature).isNever());
        }
    }

    private static void assertReason(HostCapabilities caps, Feature feature, Reason reason) {
        Availability availability = caps.availability(feature);
        assertEquals(feature.name(), Availability.State.UNSUPPORTED, availability.state);
        assertEquals(feature.name(), reason, availability.reason);
    }

    private static void assertUnknown(HostCapabilities caps, Feature... features) {
        for (Feature feature : features) {
            assertTrue(feature + " " + caps.availability(feature), caps.availability(feature).isUnknown());
        }
    }

    @Test
    public void sunshineOffersOnlyItsWebUi() {
        HostCapabilities caps = TestHosts.fixture("sunshine-serverinfo.xml");
        assertEquals(Family.SUNSHINE, caps.family());
        assertNull(caps.hostVersion());
        assertSupported(caps, WEB_UI);
        // Apollo-family hosts announce permissions even to unpaired clients, so their absence is final.
        assertNever(caps, PERMISSIONS, OTP_PAIRING, CLIPBOARD, VIRTUAL_DISPLAY, HOST_DISPLAY_MODES, FRACTIONAL_REFRESH,
                HOST_VRR, RUNTIME_BITRATE, PYROWAVE, REMOTE_SESSION_TILES, HOST_CONFIRMS_REPLACEMENT, LAUNCH_BY_UUID);
    }

    @Test
    public void pairedApolloHasPermissionsVirtualDisplayAndFractionalRatesButNoDisplayModes() {
        HostCapabilities caps = TestHosts.fixture("apollo-serverinfo.xml");
        assertEquals(Family.APOLLO, caps.family());
        assertSupported(caps, PERMISSIONS, OTP_PAIRING, CLIPBOARD, SERVER_COMMANDS, APP_UUIDS, VIRTUAL_DISPLAY,
                FRACTIONAL_REFRESH, WEB_UI);
        assertNever(caps, HOST_DISPLAY_MODES, VIRTUAL_DISPLAY_HDR, HOST_VRR, RUNTIME_BITRATE, PYROWAVE,
                REMOTE_SESSION_TILES, HOST_CONFIRMS_REPLACEMENT, LAUNCH_BY_UUID, HARDWARE_METRICS, NETWORK_PATH);
    }

    @Test
    public void unpairedApolloIsRecognizedButKeepsHttpsOnlyFeaturesOpen() {
        HostCapabilities caps = TestHosts.fixture("apollo-http-serverinfo.xml");
        assertEquals(Family.APOLLO, caps.family());
        assertSupported(caps, PERMISSIONS, OTP_PAIRING, APP_UUIDS, FRACTIONAL_REFRESH, WEB_UI);
        assertUnknown(caps, VIRTUAL_DISPLAY, PYROWAVE);
        assertReason(caps, CLIPBOARD, Reason.PAIR);
        assertReason(caps, SERVER_COMMANDS, Reason.PAIR);
    }

    @Test
    public void vibepolloGetsEveryFeatureItsExtensionsShowWithoutRubylightOnes() {
        HostCapabilities caps = TestHosts.fixture("vibepollo-serverinfo.xml");
        assertEquals(Family.VIBEPOLLO, caps.family());
        assertSupported(caps, PERMISSIONS, OTP_PAIRING, CLIPBOARD, SERVER_COMMANDS, APP_UUIDS, VIRTUAL_DISPLAY,
                VIRTUAL_DISPLAY_HDR, FRACTIONAL_REFRESH, HOST_VRR, RUNTIME_BITRATE, PYROWAVE, PYROWAVE_RECORDS,
                PYROWAVE_BANDWIDTH_PROBE, FRAME_LIMITER_STATUS, REMOTE_SESSION_TILES, HOST_CONFIRMS_REPLACEMENT,
                LAUNCH_BY_UUID, WEB_UI);
        assertNever(caps, HOST_DISPLAY_MODES, HARDWARE_METRICS, NETWORK_PATH, CONTROLLER_PREPARATION);
        assertEquals(HostCapabilities.NATIVE_PYROWAVE_RECORDS, caps.nativeFeatures());
    }

    @Test
    public void unpairedVibepolloHidesPyroWaveUntilPaired() {
        HostCapabilities caps = TestHosts.fixture("vibepollo-http-serverinfo.xml");
        assertEquals(Family.VIBEPOLLO, caps.family());
        assertUnknown(caps, PYROWAVE, PYROWAVE_RECORDS, PYROWAVE_BANDWIDTH_PROBE);
        assertTrue(caps.has(VIRTUAL_DISPLAY));
        assertReason(caps, RUNTIME_BITRATE, Reason.PAIR);
        assertEquals(0, caps.nativeFeatures());
    }

    @Test
    public void gameStreamOffersNoExtensions() {
        HostCapabilities caps = TestHosts.fixture("gamestream-serverinfo.xml");
        assertEquals(Family.GAMESTREAM, caps.family());
        assertSupported(caps);
        for (Feature feature : Feature.values()) {
            assertTrue(feature.name(), caps.availability(feature).isNever());
        }
    }

    @Test
    public void hdrFormatsComeOnlyFromAHostThatListsHlgOrDolbyVision() {
        assertEquals(15, TestHosts.fixture("rubylight-30-serverinfo.xml").hdrFormats());
        // PQ alone is stock HDR10: nothing to ask for.
        assertEquals(0, TestHosts.fixture("rubylight-30-refused-serverinfo.xml").hdrFormats());
        assertEquals(0, TestHosts.rubylight().hdrFormats());
        assertEquals(0, TestHosts.fixture("sunshine-serverinfo.xml").hdrFormats());
        // An AV1 host adds Dolby Vision 10.1 (16); bits past it are not this client's, but for the
        // PyroWave HLG offer flag (128).
        String host = "<RustHostVersion>3.1.0</RustHostVersion><PairStatus>1</PairStatus>";
        assertEquals(31, caps(host + "<HdrFormats>31</HdrFormats>").hdrFormats());
        assertEquals(17, caps(host + "<HdrFormats>17</HdrFormats>").hdrFormats());
        assertEquals(31 | 128, caps(host + "<HdrFormats>255</HdrFormats>").hdrFormats());
    }

    @Test
    public void hdrFormatsKeepThePyroWaveHlgOfferFlag() {
        String xml = TestHosts.serverInfo("rubylight-30-serverinfo.xml");
        assertEquals(143, TestHosts.caps(xml.replace("<HdrFormats>15<", "<HdrFormats>143<")).hdrFormats());
        assertEquals(131, TestHosts.caps(xml.replace("<HdrFormats>15<", "<HdrFormats>131<")).hdrFormats());
        // Unknown bits drop out, Dolby Vision 10.1 (16) and the flag stay; the flag alone is no HDR offer.
        assertEquals(159, TestHosts.caps(xml.replace("<HdrFormats>15<", "<HdrFormats>255<")).hdrFormats());
        assertEquals(0, TestHosts.caps(xml.replace("<HdrFormats>15<", "<HdrFormats>128<")).hdrFormats());
    }

    @Test
    public void currentRubylightSupportsEverythingButCursorShapesTheNativeApiSleepAndFiles() {
        HostCapabilities caps = TestHosts.rubylight();
        assertEquals(Family.RUBYLIGHT, caps.family());
        assertEquals("2.2.21", caps.hostVersion());
        // Everything up to file transfer; the 3.0 extensions came later.
        Set<Feature> expected = EnumSet.range(PERMISSIONS, FILE_DOWNLOAD);
        expected.removeAll(EnumSet.of(DISPLAY_SCALE, CURSOR_SHAPE, NATIVE_API, SLEEP, FILE_UPLOAD, FILE_DOWNLOAD));
        assertEquals(expected, caps.supported());
        assertReason(caps, DISPLAY_SCALE, Reason.UPDATE_HOST);
        assertNever(caps, DOLBY_VISION, HLG, CLIENT_FEC, BANDWIDTH_PROBE, AUDIO_PASSTHROUGH, SURROUND_714, TEXT_INPUT,
                CLIENT_KEYBOARD_LAYOUT, PEN_BARREL_ROLL, PIPELINE_RATES);
        assertNever(caps, CURSOR_SHAPE);
        assertReason(caps, NATIVE_API, Reason.UPDATE_HOST);
        assertReason(caps, SLEEP, Reason.UPDATE_HOST);
        // File transfer rides on the native API, which this version lacks.
        assertReason(caps, FILE_UPLOAD, Reason.UPDATE_HOST);
        assertReason(caps, FILE_DOWNLOAD, Reason.UPDATE_HOST);
        assertEquals(-1, caps.nativeApiVersion());
    }

    @Test
    public void nativeApiVersionDecidesTheNativeApi() {
        String paired = "<RustHostVersion>3.0.0</RustHostVersion><PairStatus>1</PairStatus>";
        for (String version : new String[] {"1", "2"}) {
            HostCapabilities caps = caps(paired + "<NativeApiVersion>" + version + "</NativeApiVersion>");
            assertTrue(version, caps.has(NATIVE_API));
            assertEquals(Integer.parseInt(version), caps.nativeApiVersion());
        }
        // The host reports 0 over plain HTTP and to an unpaired certificate.
        assertReason(caps(paired + "<NativeApiVersion>0</NativeApiVersion>"), NATIVE_API, Reason.PAIR);
        assertReason(caps("<RustHostVersion>3.0.0</RustHostVersion><PairStatus>0</PairStatus>"
                + "<NativeApiVersion>1</NativeApiVersion>"), NATIVE_API, Reason.PAIR);
        // Without the field a Rubylight host needs an update and every other host never has it.
        assertReason(caps(paired), NATIVE_API, Reason.UPDATE_HOST);
        for (String host : new String[] {"sunshine-serverinfo.xml", "apollo-serverinfo.xml", "vibepollo-serverinfo.xml",
                "gamestream-serverinfo.xml"}) {
            HostCapabilities caps = TestHosts.fixture(host);
            assertFalse(host, caps.has(NATIVE_API));
            assertTrue(host, caps.availability(NATIVE_API).isNever());
            assertEquals(host, -1, caps.nativeApiVersion());
        }
        assertFalse(HostCapabilities.UNKNOWN.has(NATIVE_API));
        assertEquals(-1, HostCapabilities.UNKNOWN.nativeApiVersion());
        HostCapabilities stored = caps(paired + "<NativeApiVersion>1</NativeApiVersion>");
        assertEquals(stored, HostCapabilities.fromJson(stored.toJson()));
        assertTrue(HostCapabilities.fromJson(stored.toJson()).has(NATIVE_API));
        // Capabilities stored before the field existed still load, without the native API.
        String olderJson = TestHosts.rubylight().toJson().replace(",\"nativeApiVersion\":-1", "");
        assertFalse(olderJson, olderJson.contains("nativeApiVersion"));
        assertFalse(HostCapabilities.fromJson(olderJson).has(NATIVE_API));
    }

    /** The cases the Rust parser reads too (http/serverinfo.rs), so both gate Sleep PC alike. */
    @Test
    public void powerActionFieldsDecideSleepLikeTheRustParser() throws IOException, JSONException {
        Path cases = Paths.get("../protocol/client/tests/fixtures/power/sleep-serverinfo.txt");
        if (!Files.exists(cases)) {
            cases = Paths.get("protocol/client/tests/fixtures/power/sleep-serverinfo.txt");
        }
        int count = 0;
        for (String line : Files.readAllLines(cases, StandardCharsets.UTF_8)) {
            if (line.isEmpty() || line.startsWith("#")) {
                continue;
            }
            int bar = line.indexOf('|');
            String expected = line.substring(0, bar);
            HostCapabilities caps = TestHosts.caps(line.substring(bar + 1));
            Availability availability = caps.availability(SLEEP);
            switch (expected) {
                case "supported": assertTrue(line, availability.isSupported()); break;
                case "never": assertTrue(line, availability.isNever()); break;
                case "unknown": assertTrue(line, availability.isUnknown()); break;
                case "pair": assertReason(caps, SLEEP, Reason.PAIR); break;
                case "update_host": assertReason(caps, SLEEP, Reason.UPDATE_HOST); break;
                case "sleep_permission": assertReason(caps, SLEEP, Reason.SLEEP_PERMISSION); break;
                default: fail("unknown availability " + expected);
            }
            assertEquals(line, availability.isSupported(), caps.supported().contains(SLEEP));
            assertEquals(line, caps, HostCapabilities.fromJson(caps.toJson()));
            assertEquals(line, availability, HostCapabilities.fromJson(caps.toJson()).availability(SLEEP));
            count++;
        }
        assertEquals(14, count);
        assertFalse(HostCapabilities.UNKNOWN.has(SLEEP));
        // Capabilities stored before the fields existed still load, without sleep.
        HostCapabilities allowed = caps("<RustHostVersion>3.0.0</RustHostVersion><PairStatus>1</PairStatus>"
                + "<PowerActionVersion>1</PowerActionVersion><SleepSupported>1</SleepSupported><SleepAllowed>1</SleepAllowed>");
        assertTrue(allowed.has(SLEEP));
        JSONObject older = new JSONObject(allowed.toJson());
        older.remove("powerActionVersion");
        older.remove("sleepSupported");
        older.remove("sleepAllowed");
        assertReason(HostCapabilities.fromJson(older.toString()), SLEEP, Reason.UPDATE_HOST);
    }

    private static void assertAvailability(String line, HostCapabilities caps, Feature feature, String expected) {
        Availability availability = caps.availability(feature);
        switch (expected) {
            case "supported": assertTrue(line, availability.isSupported()); break;
            case "never": assertTrue(line, availability.isNever()); break;
            case "unknown": assertTrue(line, availability.isUnknown()); break;
            case "pair": assertReason(caps, feature, Reason.PAIR); break;
            case "update_host": assertReason(caps, feature, Reason.UPDATE_HOST); break;
            case "file_permission": assertReason(caps, feature, Reason.FILE_PERMISSION); break;
            default: fail("unknown availability " + expected);
        }
        assertEquals(line, availability.isSupported(), caps.supported().contains(feature));
    }

    /** The cases the Rust parser reads too (http/capabilities.rs), so both gate file transfer alike. */
    @Test
    public void nativeCapabilitiesDecideFileTransferLikeTheRustParser() throws IOException, JSONException {
        Path cases = Paths.get("../protocol/client/tests/fixtures/files/files-capabilities.txt");
        if (!Files.exists(cases)) {
            cases = Paths.get("protocol/client/tests/fixtures/files/files-capabilities.txt");
        }
        String capabilities = null;
        int count = 0;
        int invalid = 0;
        for (String line : Files.readAllLines(cases, StandardCharsets.UTF_8)) {
            if (line.isEmpty() || line.startsWith("#")) {
                continue;
            }
            String[] fields = line.split("\\|", 4);
            HostCapabilities caps = TestHosts.caps(fields[3]);
            // Serverinfo alone never decides file transfer on a host with the native API.
            assertFalse(line, caps.has(FILE_UPLOAD));
            assertFalse(line, caps.has(FILE_DOWNLOAD));
            if ("invalid".equals(fields[0])) {
                // Refused like the Rust parser refuses it, even with no serverinfo to add to.
                for (HostCapabilities base : new HostCapabilities[] {caps, HostCapabilities.UNKNOWN}) {
                    try {
                        base.withNativeCapabilities(fields[2]);
                        fail(line);
                    } catch (JSONException expected) {
                        // Thrown, as it should be.
                    }
                }
                invalid++;
                continue;
            }
            if (!"-".equals(fields[2])) {
                capabilities = fields[2];
                caps = caps.withNativeCapabilities(fields[2]);
            }
            assertAvailability(line, caps, FILE_UPLOAD, fields[0]);
            assertAvailability(line, caps, FILE_DOWNLOAD, fields[1]);
            HostCapabilities restored = HostCapabilities.fromJson(caps.toJson());
            assertEquals(line, caps, restored);
            assertEquals(line, caps.availability(FILE_UPLOAD), restored.availability(FILE_UPLOAD));
            assertEquals(line, caps.availability(FILE_DOWNLOAD), restored.availability(FILE_DOWNLOAD));
            count++;
        }
        assertEquals(13, count);
        assertEquals(15, invalid);
        // Without serverinfo there is nothing to add to.
        assertSame(HostCapabilities.UNKNOWN, HostCapabilities.UNKNOWN.withNativeCapabilities(capabilities));
        // A body that is not a capabilities object is an error, not a host without file transfer.
        HostCapabilities rubylight = caps("<RustHostVersion>3.1.0</RustHostVersion><PairStatus>1</PairStatus>"
                + "<NativeApiVersion>1</NativeApiVersion>");
        for (String damaged : new String[] {"", "[]", "{}", "{\"features\":[]}", "{\"permissions\":{}}",
                "{\"features\":[\"files.upload\"],\"permissions\":{\"file_upload\":\"true\"}}",
                "{\"features\":[\"files.upload\"],\"permissions\":{\"file_upload\":1}}"}) {
            try {
                rubylight.withNativeCapabilities(damaged);
                fail(damaged);
            } catch (JSONException expected) {
                // Thrown, as it should be.
            }
        }
        // Capabilities stored before the fields existed still load, with file transfer unknown.
        JSONObject older = new JSONObject(rubylight.withNativeCapabilities(capabilities).toJson());
        for (String key : new String[] {"nativeCapabilities", "filesUpload", "filesDownload", "fileUploadPermission",
                "fileDownloadPermission"}) {
            assertNotNull(key, older.remove(key));
        }
        assertUnknown(HostCapabilities.fromJson(older.toString()), FILE_UPLOAD, FILE_DOWNLOAD);
    }

    /** Matches the Rust test of the same name: a second read replaces the file flags, it never adds to them. */
    @Test
    public void readingTheNativeCapabilitiesAgainDropsWhatTheHostStoppedOffering() throws JSONException {
        String offered = "{\"api_version\":1,\"host\":{\"name\":\"RL\",\"version\":\"3.1.0\",\"unique_id\":\"rl\"},"
                + "\"features\":[\"library\",\"files.upload\",\"files.download\"],\"permissions\":{\"list\":true,"
                + "\"view\":true,\"launch\":true,\"host_control\":false,\"file_upload\":true,\"file_download\":true}}";
        String withdrawn = offered.replace(",\"files.upload\",\"files.download\"", "")
                .replace(",\"file_upload\":true,\"file_download\":true", "");
        assertNotEquals(offered, withdrawn);
        HostCapabilities caps = caps("<RustHostVersion>3.1.0</RustHostVersion><PairStatus>1</PairStatus>"
                + "<NativeApiVersion>1</NativeApiVersion>").withNativeCapabilities(offered);
        assertTrue(caps.has(FILE_UPLOAD));
        assertTrue(caps.has(FILE_DOWNLOAD));
        caps = caps.withNativeCapabilities(withdrawn);
        assertNever(caps, FILE_UPLOAD, FILE_DOWNLOAD);
        // Also after a round trip through the stored form.
        assertNever(HostCapabilities.fromJson(caps.toJson()), FILE_UPLOAD, FILE_DOWNLOAD);
    }

    @Test
    public void cursorShapesNeedTheirVersionFieldFromAnyHost() {
        for (String host : new String[] {"<appversion>7.1.431.-1</appversion>", "<Permission>0</Permission>",
                "<FrameLimiterSupported>1</FrameLimiterSupported>", "<RustHostVersion>2.2.21</RustHostVersion>",
                "<appversion>7.1.431.0</appversion>"}) {
            assertTrue(host, caps(host + "<CursorShapeVersion>1</CursorShapeVersion>").has(CURSOR_SHAPE));
            assertTrue(host, caps(host + "<CursorShapeVersion>2</CursorShapeVersion>").has(CURSOR_SHAPE));
            for (String version : new String[] {"0", "-1", "bad", "", "65536"}) {
                assertFalse(host + " " + version, caps(host + "<CursorShapeVersion>" + version +
                        "</CursorShapeVersion>").has(CURSOR_SHAPE));
            }
        }
        assertNever(caps("<appversion>7.1.431.-1</appversion>"), CURSOR_SHAPE);
        assertUnknown(caps("<appversion>7.1.431.0</appversion>"), CURSOR_SHAPE);
        HostCapabilities stored = caps("<RustHostVersion>2.2.21</RustHostVersion><CursorShapeVersion>1</CursorShapeVersion>");
        assertEquals(stored, HostCapabilities.fromJson(stored.toJson()));
        assertTrue(HostCapabilities.fromJson(stored.toJson()).has(CURSOR_SHAPE));
        // Capabilities stored before the field existed still load, without cursor shapes.
        String olderJson = TestHosts.rubylight().toJson().replace(",\"cursorShapeVersion\":-1", "")
                .replace("\"cursorShapeVersion\":-1,", "");
        assertFalse(olderJson, olderJson.contains("cursorShapeVersion"));
        HostCapabilities older = HostCapabilities.fromJson(olderJson);
        assertTrue(older.isKnown());
        assertFalse(older.has(CURSOR_SHAPE));
    }

    /**
     * Each 3.0 extension follows its own serverinfo field. The cases are this test's own; the shared contract
     * fixtures (HostCapabilitiesContractTest) are what holds the Java and Rust parsers to the same answers.
     */
    @Test
    public void the30ExtensionsFollowTheirServerinfoBits() {
        String host = "<RustHostVersion>3.1.0</RustHostVersion><PairStatus>1</PairStatus>"
                + "<NativeApiVersion>1</NativeApiVersion>";
        Object[][] cases = {
                {"HdrFormats", DOLBY_VISION, new String[] {"4", "8", "16"}, new String[] {"3", "x", ""}},
                {"HdrFormats", HLG, new String[] {"2"}, new String[] {"13", "256"}},
                {"AudioCodecs", AUDIO_PASSTHROUGH, new String[] {"2", "4", "8"}, new String[] {"1"}},
                {"AudioLayouts", SURROUND_714, new String[] {"8"}, new String[] {"7"}},
                {"FecControlVersion", CLIENT_FEC, new String[] {"1", "2"}, new String[] {"0", "-1"}},
                {"BandwidthProbeVersion", BANDWIDTH_PROBE, new String[] {"1"}, new String[] {"0"}},
                {"TextInputVersion", TEXT_INPUT, new String[] {"1"}, new String[] {"0"}},
                {"KeyboardLayoutVersion", CLIENT_KEYBOARD_LAYOUT, new String[] {"1"}, new String[] {"0"}},
                {"PenBarrelRollVersion", PEN_BARREL_ROLL, new String[] {"1"}, new String[] {"65536"}},
                {"PipelineRatesVersion", PIPELINE_RATES, new String[] {"1"}, new String[] {""}},
        };
        for (Object[] row : cases) {
            String field = (String) row[0];
            Feature feature = (Feature) row[1];
            for (String value : (String[]) row[2]) {
                HostCapabilities caps = caps(host + "<" + field + ">" + value + "</" + field + ">");
                assertTrue(field + " " + value, caps.has(feature));
                assertEquals(field + " " + value, caps, HostCapabilities.fromJson(caps.toJson()));
            }
            for (String value : (String[]) row[3]) {
                assertNever(caps(host + "<" + field + ">" + value + "</" + field + ">"), feature);
            }
            // Missing: never from Rubylight and Sunshine, unknown from an unrecognized host.
            assertNever(caps(host), feature);
            assertNever(caps("<appversion>7.1.431.-1</appversion>"), feature);
            assertUnknown(caps("<appversion>7.1.431.0</appversion>"), feature);
        }
        // Before the native API answered, its features are unknown, never supported.
        HostCapabilities caps = caps(host + "<HdrFormats>15</HdrFormats>");
        assertUnknown(caps, USB_FORWARDING, IMAGE_CLIPBOARD, SHARED_FOLDERS);
        assertReason(TestHosts.rubylight(), USB_FORWARDING, Reason.UPDATE_HOST);
    }

    @Test
    public void nativeCapabilitiesDecideUsbImageClipboardAndFolders() throws JSONException {
        String offered = "{\"api_version\":1,\"host\":{\"name\":\"RL\",\"version\":\"3.1.0\",\"unique_id\":\"rl\"},"
                + "\"features\":[\"library\",\"usb\",\"clipboard.image\",\"folders\"],\"permissions\":{\"list\":true,"
                + "\"view\":true,\"launch\":true,\"host_control\":false,\"usb\":true,\"shared_folders\":true}}";
        String refused = offered.replace("\"usb\":true,\"shared_folders\":true", "\"usb\":false");
        String withdrawn = offered.replace(",\"usb\",\"clipboard.image\",\"folders\"", "");
        String host = "<RustHostVersion>3.1.0</RustHostVersion><PairStatus>1</PairStatus>"
                + "<NativeApiVersion>1</NativeApiVersion>";
        HostCapabilities caps = caps(host + "<Permission>" + 0xFFFFFFFFL + "</Permission>")
                .withNativeCapabilities(offered);
        assertTrue(caps.has(USB_FORWARDING));
        assertTrue(caps.has(IMAGE_CLIPBOARD));
        assertTrue(caps.has(SHARED_FOLDERS));
        assertEquals(caps, HostCapabilities.fromJson(caps.toJson()));
        // A missing permission is refused, like a false one.
        caps = caps.withNativeCapabilities(refused);
        assertReason(caps, USB_FORWARDING, Reason.USB_PERMISSION);
        assertReason(caps, SHARED_FOLDERS, Reason.FOLDER_PERMISSION);
        // The image clipboard follows the clipboard permission.
        assertReason(caps(host + "<Permission>0</Permission>").withNativeCapabilities(offered), IMAGE_CLIPBOARD,
                Reason.CLIPBOARD_PERMISSION);
        assertNever(caps.withNativeCapabilities(withdrawn), USB_FORWARDING, IMAGE_CLIPBOARD, SHARED_FOLDERS);
        // A 3.0 permission that is not a boolean is refused; file transfer beside it still counts.
        String files = offered.replace("\"folders\"]", "\"folders\",\"files.upload\"]")
                .replace("\"host_control\":false,", "\"host_control\":false,\"file_upload\":true,");
        for (String odd : new String[] {"\"true\"", "1", "null", "{}"}) {
            HostCapabilities read = caps.withNativeCapabilities(files.replace("\"usb\":true", "\"usb\":" + odd)
                    .replace("\"shared_folders\":true", "\"shared_folders\":" + odd));
            assertReason(read, USB_FORWARDING, Reason.USB_PERMISSION);
            assertReason(read, SHARED_FOLDERS, Reason.FOLDER_PERMISSION);
            assertTrue(odd, read.has(FILE_UPLOAD));
        }
    }

    @Test
    public void olderRubylightNeedsAnUpdateForNewerFeatures() {
        HostCapabilities caps = TestHosts.fixture("butterpollo-serverinfo.xml");
        assertEquals(Family.RUBYLIGHT, caps.family());
        assertEquals("2.0.0-rc.19", caps.hostVersion());
        for (Feature feature : new Feature[] {HOST_DISPLAY_MODES, RUNTIME_BITRATE, HARDWARE_METRICS, NETWORK_PATH,
                CONTROLLER_PREPARATION}) {
            assertReason(caps, feature, Reason.UPDATE_HOST);
        }
        assertTrue(caps.has(PYROWAVE_RECORDS));
        assertTrue(caps.has(FRACTIONAL_REFRESH));
        assertTrue(caps.has(VIRTUAL_DISPLAY));
    }

    @Test
    public void viewOnlyDeviceSeesWhichPermissionsToAllow() {
        HostCapabilities caps = TestHosts.fixture("butterpollo-viewer-serverinfo.xml");
        assertReason(caps, CLIPBOARD, Reason.CLIPBOARD_PERMISSION);
        assertReason(caps, SERVER_COMMANDS, Reason.COMMAND_PERMISSION);
        HostCapabilities noView = caps("<RustHostVersion>2.2.21</RustHostVersion><PairStatus>1</PairStatus>" +
                "<Permission>" + ComputerDetails.PERMISSION_LIST + "</Permission><NetworkPathVersion>0</NetworkPathVersion>" +
                "<HardwareMetricsVersion>0</HardwareMetricsVersion><RustHostEncoderBitrateKbps>0</RustHostEncoderBitrateKbps>" +
                "<RustHostInitialEncoderBitrateKbps>0</RustHostInitialEncoderBitrateKbps>");
        assertReason(noView, RUNTIME_BITRATE, Reason.VIEW_PERMISSION);
        assertReason(noView, HARDWARE_METRICS, Reason.VIEW_PERMISSION);
        assertReason(noView, NETWORK_PATH, Reason.VIEW_PERMISSION);
        HostCapabilities metricsOff = caps("<RustHostVersion>2.2.21</RustHostVersion><PairStatus>1</PairStatus>" +
                "<Permission>4294967295</Permission><HardwareMetricsVersion>0</HardwareMetricsVersion>");
        assertReason(metricsOff, HARDWARE_METRICS, Reason.HARDWARE_METRICS_OFF);
        HostCapabilities noCommands = caps("<Permission>4294967295</Permission><PairStatus>1</PairStatus>" +
                "<appversion>7.1.431.-1</appversion>");
        assertReason(noCommands, SERVER_COMMANDS, Reason.NO_COMMANDS);
    }

    @Test
    public void familyFollowsTheDocumentedOrder() {
        assertEquals(Family.GAMESTREAM, caps("<state>MJOLNIR_SERVER_FREE</state><RustHostVersion>2.2.21</RustHostVersion>").family());
        assertEquals(Family.RUBYLIGHT, caps("<RustHostVersion>2.2.21</RustHostVersion><FrameLimiterSupported>1</FrameLimiterSupported>").family());
        // The tag marks Vibepollo even when its value says the limiter is off.
        assertEquals(Family.VIBEPOLLO, caps("<FrameLimiterSupported>0</FrameLimiterSupported><Permission>0</Permission>").family());
        assertEquals(Family.VIBEPOLLO, caps("<PyroWaveHostLinkMbps>0</PyroWaveHostLinkMbps>").family());
        assertEquals(Family.APOLLO, caps("<Permission>0</Permission>").family());
        assertEquals(Family.APOLLO, caps("<ServerCommand>Sleep</ServerCommand>").family());
        assertEquals(Family.APOLLO, caps("<VirtualDisplayCapable>true</VirtualDisplayCapable>").family());
        assertEquals(Family.SUNSHINE, caps("<appversion>7.1.431.-1</appversion>").family());
        assertEquals(Family.UNKNOWN, caps("<appversion>7.1.431.0</appversion><state>SUNSHINE_SERVER_FREE</state>").family());
        assertEquals(Family.RUBYLIGHT, caps("<RustHostVersion> 2.2.21 </RustHostVersion>").family());
        assertEquals(Family.SUNSHINE, caps("<appversion>7.1.431.-1</appversion><RustHostVersion></RustHostVersion>").family());
    }

    @Test
    public void unrecognizedHostsRuleNothingOutAndNeverSeenHostsKnowNothing() {
        HostCapabilities other = caps("<appversion>7.1.431.0</appversion><PairStatus>1</PairStatus>");
        for (Feature feature : Feature.values()) {
            assertFalse(other.has(feature));
            assertTrue(HostCapabilities.UNKNOWN.availability(feature).isUnknown());
            if (feature == SLEEP) {
                // Paired: a host with Sleep PC would have sent its power fields.
                assertTrue(other.availability(feature).isNever());
                continue;
            }
            assertTrue(feature.name(), other.availability(feature).isUnknown());
            assertTrue(other.mayHave(feature));
        }
        assertFalse(HostCapabilities.UNKNOWN.isKnown());
        assertTrue(other.isKnown());
        assertEquals(Family.UNKNOWN, HostCapabilities.UNKNOWN.family());
    }

    @Test
    public void pyroWaveCodecBitsAreTrustedOnlyFromHostsThatShowPyroWave() {
        int codecs = 0x07800000 | 0x30301;
        // Sunshine has no PyroWave; a future use of these bits must not become PyroWave.
        assertEquals(0x30301, caps("<appversion>7.1.431.-1</appversion><ServerCodecModeSupport>" + codecs +
                "</ServerCodecModeSupport>").trustedCodecModeSupport(codecs));
        assertEquals(0x30301, caps("<Permission>4294967295</Permission><PairStatus>1</PairStatus><ServerCodecModeSupport>" +
                codecs + "</ServerCodecModeSupport>").trustedCodecModeSupport(codecs));
        assertEquals(codecs, TestHosts.fixture("vibepollo-serverinfo.xml").trustedCodecModeSupport(codecs));
        assertEquals(codecs, TestHosts.rubylight().trustedCodecModeSupport(codecs));
        assertEquals(0x30301, HostCapabilities.UNKNOWN.trustedCodecModeSupport(codecs));
        // PyroWave fields without codec bits: the host has PyroWave off or no encoder for it.
        assertNever(caps("<RustHostVersion>2.2.21</RustHostVersion><ServerCodecModeSupport>197377</ServerCodecModeSupport>"),
                PYROWAVE, PYROWAVE_RECORDS);
    }

    @Test
    public void pyroWaveRecordsKeepTheNativeVersionRule() {
        for (String version : new String[] {"2.0.0-rc.1", "2.0.0-rc.25", "2.0.0", "2.1.0", "3.0.0", "2.0.0-rc.2+build.7",
                "3.0.0-alpha.1", "3.0.0-beta.1", "3.0.0-beta.12+abc.1"}) {
            assertEquals(version, HostCapabilities.NATIVE_PYROWAVE_RECORDS, rubylightWithPyroWave(version).nativeFeatures());
        }
        for (String version : new String[] {"1.9.9", "2.0.0-rc.0", "3.0.0-beta.0", "3.0.0-beta", "3.0.0-gamma.1", "test", "2.0", "2.0.0-rc.1junk",
                "2.0.0-rc.01", "2.0.0+", "02.0.0", "999999999.0.0"}) {
            assertEquals(version, 0, rubylightWithPyroWave(version).nativeFeatures());
            assertTrue(rubylightWithPyroWave(version).has(PYROWAVE));
        }
    }

    @Test
    public void preReleasesRankAlphaBelowBetaBelowRcBelowTheRelease() {
        String[] ascending = {"3.0.0-alpha.1", "3.0.0-alpha.9", "3.0.0-beta.1", "3.0.0-beta.12", "3.0.0-rc.1", "3.0.0-rc.30", "3.0.0"};
        for (int i = 0; i + 1 < ascending.length; i++) {
            assertTrue(ascending[i], !HostCapabilities.versionAtLeast(
                    HostCapabilities.parseRubylightVersion(ascending[i]), HostCapabilities.parseRubylightVersion(ascending[i + 1])));
            assertTrue(ascending[i + 1], HostCapabilities.versionAtLeast(
                    HostCapabilities.parseRubylightVersion(ascending[i + 1]), HostCapabilities.parseRubylightVersion(ascending[i])));
        }
    }

    private static HostCapabilities rubylightWithPyroWave(String version) {
        return caps("<RustHostVersion>" + version + "</RustHostVersion><ServerCodecModeSupport>" + 0x07800001 +
                "</ServerCodecModeSupport>");
    }

    @Test
    public void displayModesNeedRubylight221AndAWorkingVirtualDisplay() {
        String fields = "<VirtualDisplayCapable>true</VirtualDisplayCapable><VirtualDisplayDriverReady>true</VirtualDisplayDriverReady>";
        for (String version : new String[] {"2.2.0", "2.2.1-rc.3", "2.2.1-beta.3", "2.0.0-rc.19", "1.0", "test"}) {
            assertReason(caps("<RustHostVersion>" + version + "</RustHostVersion>" + fields), HOST_DISPLAY_MODES, Reason.UPDATE_HOST);
        }
        for (String version : new String[] {"2.2.1", "2.2.21", "2.10.0", "3.0.0-rc.1", "3.0.0-alpha.1", "3.0.0-beta.1"}) {
            assertTrue(version, caps("<RustHostVersion>" + version + "</RustHostVersion>" + fields).has(HOST_DISPLAY_MODES));
        }
        HostCapabilities off = caps("<RustHostVersion>2.2.21</RustHostVersion><VirtualDisplayCapable>false</VirtualDisplayCapable>");
        assertReason(off, HOST_DISPLAY_MODES, Reason.VIRTUAL_DISPLAY_OFF);
        assertReason(off, VIRTUAL_DISPLAY, Reason.VIRTUAL_DISPLAY_OFF);
        HostCapabilities driver = caps("<RustHostVersion>2.2.21</RustHostVersion><VirtualDisplayCapable>1</VirtualDisplayCapable>" +
                "<VirtualDisplayDriverReady>0</VirtualDisplayDriverReady>");
        assertReason(driver, HOST_DISPLAY_MODES, Reason.VIRTUAL_DISPLAY_DRIVER);
        assertReason(driver, VIRTUAL_DISPLAY_HDR, Reason.VIRTUAL_DISPLAY_DRIVER);
        // A platform without one (Vibepollo on Linux) can never use it.
        assertNever(caps("<FrameLimiterSupported>1</FrameLimiterSupported><VirtualDisplayCapable>false</VirtualDisplayCapable>"),
                VIRTUAL_DISPLAY);
    }

    @Test
    public void displayScaleNeedsDisplayScaleVersionAndAWorkingVirtualDisplay() {
        String host = "<RustHostVersion>3.0.0</RustHostVersion><VirtualDisplayCapable>true</VirtualDisplayCapable>" +
                "<VirtualDisplayDriverReady>true</VirtualDisplayDriverReady>";
        // Older hosts (2.6.x, 3.0.0-rc.5) don't send it and ignore rlDisplayScale.
        for (String version : new String[] {"", "<DisplayScaleVersion>0</DisplayScaleVersion>",
                "<DisplayScaleVersion>x</DisplayScaleVersion>"}) {
            assertReason(caps(host + version), DISPLAY_SCALE, Reason.UPDATE_HOST);
        }
        for (String version : new String[] {"1", "2"}) {
            assertTrue(version, caps(host + "<DisplayScaleVersion>" + version + "</DisplayScaleVersion>").has(DISPLAY_SCALE));
        }
        assertReason(caps("<RustHostVersion>3.0.0</RustHostVersion><VirtualDisplayCapable>false</VirtualDisplayCapable>" +
                "<DisplayScaleVersion>1</DisplayScaleVersion>"), DISPLAY_SCALE, Reason.VIRTUAL_DISPLAY_OFF);
        // Only Rubylight reads it; Apollo and Vibepollo have a virtual display but not this option.
        for (String fixture : new String[] {"apollo-serverinfo.xml", "vibepollo-serverinfo.xml", "sunshine-serverinfo.xml"}) {
            assertNever(TestHosts.fixture(fixture), DISPLAY_SCALE);
        }
    }

    @Test
    public void bandwidthProbeNeedsPairingAndTheExactAdvertisedSize() {
        assertReason(caps("<RustHostVersion>2.2.21</RustHostVersion><PyroWaveBandwidthProbeBytes>0</PyroWaveBandwidthProbeBytes>"),
                PYROWAVE_BANDWIDTH_PROBE, Reason.PAIR);
        for (String bytes : new String[] {"0", "-1", "33554433", "bad"}) {
            assertNever(caps("<FrameLimiterSupported>1</FrameLimiterSupported><PairStatus>1</PairStatus>" +
                    "<PyroWaveBandwidthProbeBytes>" + bytes + "</PyroWaveBandwidthProbeBytes>"), PYROWAVE_BANDWIDTH_PROBE);
        }
        assertReason(caps("<RustHostVersion>2.0.0-rc.1</RustHostVersion><PairStatus>1</PairStatus>"),
                PYROWAVE_BANDWIDTH_PROBE, Reason.UPDATE_HOST);
    }

    @Test
    public void storedCapabilitiesRoundTripWithoutIdentifyingDetails() {
        for (String fixture : FIXTURES) {
            HostCapabilities caps = TestHosts.fixture(fixture);
            String json = caps.toJson();
            HostCapabilities restored = HostCapabilities.fromJson(json);
            assertEquals(fixture, caps, restored);
            assertEquals(fixture, caps.family(), restored.family());
            for (Feature feature : Feature.values()) {
                assertEquals(fixture + " " + feature, caps.availability(feature), restored.availability(feature));
            }
            for (String secret : new String[] {"fixture", "192.0.2", "00:00:00", "ef95b248", "6f2d5a1c", "Sleep", "Fixture command"}) {
                assertFalse(fixture + " stores " + secret, json.contains(secret));
            }
        }
        assertNull(HostCapabilities.UNKNOWN.toJson());
        for (String damaged : new String[] {null, "", "{", "[]", "{\"v\":2}", "{\"v\":1}",
                TestHosts.rubylight().toJson().replace("\"v\":1", "\"v\":99"),
                TestHosts.rubylight().toJson().replace("4294967295", "4294967296")}) {
            assertSame(String.valueOf(damaged), HostCapabilities.UNKNOWN, HostCapabilities.fromJson(damaged));
        }
    }

    @Test
    public void familyLabelsNameTheHostOnlyForIdentification() {
        Context context = RuntimeEnvironment.getApplication();
        assertEquals("Rubylight 2.2.21", TestHosts.rubylight().familyLabel(context));
        assertEquals("Apollo-compatible host", TestHosts.fixture("apollo-serverinfo.xml").familyLabel(context));
        assertEquals("Vibepollo-compatible host", TestHosts.fixture("vibepollo-serverinfo.xml").familyLabel(context));
        assertEquals("Sunshine-compatible host", TestHosts.fixture("sunshine-serverinfo.xml").familyLabel(context));
        assertEquals("NVIDIA GameStream", TestHosts.fixture("gamestream-serverinfo.xml").familyLabel(context));
        assertEquals("Moonlight-compatible host", HostCapabilities.UNKNOWN.familyLabel(context));
    }

    @Test
    public void reasonsFitSixtyCharactersInEveryLanguage() {
        for (Locale locale : new Locale[] {Locale.ENGLISH, Locale.GERMAN}) {
            Configuration configuration = new Configuration(RuntimeEnvironment.getApplication().getResources().getConfiguration());
            configuration.setLocale(locale);
            Context context = RuntimeEnvironment.getApplication().createConfigurationContext(configuration);
            for (Reason reason : Reason.values()) {
                String text = context.getString(reason.text);
                assertTrue(locale + " " + reason + ": " + text, text.length() <= 60);
            }
            assertTrue(context.getString(R.string.host_feature_unknown).length() <= 60);
        }
    }
}
