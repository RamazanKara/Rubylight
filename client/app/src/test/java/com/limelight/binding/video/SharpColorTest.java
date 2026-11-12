package com.limelight.binding.video;

import android.app.Application;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.HashMap;
import java.util.Iterator;
import java.util.Map;
import java.util.TreeMap;

import static org.junit.Assert.*;

/**
 * Sharper colored text over JNI. The contract test holds {@link SharpColor} to
 * protocol/client/src/sharp_color_contract.json, which nightjar-core's test and the JNI entry's test check
 * the model answers exactly:
 * each request Java sends is one of the file's, and Java reads each answer as the file means it.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class SharpColorTest {
    @After
    public void forgetTheModel() {
        SharpColor.use(null);
    }

    private static JSONArray contract() throws Exception {
        for (String path : new String[] {"../protocol/client/src/sharp_color_contract.json", "protocol/client/src/sharp_color_contract.json"}) {
            File file = new File(path);
            if (file.isFile()) return new JSONArray(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
        }
        throw new AssertionError("sharp_color_contract.json not found from " + new File(".").getAbsolutePath());
    }

    /** The JSON with its keys sorted, so two requests compare whatever order they were built in. */
    private static String canonical(JSONObject json) throws Exception {
        Map<String, Object> sorted = new TreeMap<>();
        for (Iterator<String> keys = json.keys(); keys.hasNext();) {
            String key = keys.next();
            sorted.put(key, json.isNull(key) ? "null" : json.get(key).toString());
        }
        return sorted.toString();
    }

    private static SharpColor.Request request(JSONObject json) throws Exception {
        SharpColor.Request request = new SharpColor.Request();
        request.auto = json.getBoolean("auto");
        request.hdr = json.getBoolean("hdr");
        request.sdr10 = json.getBoolean("sdr10");
        request.pyroWave = json.getBoolean("pyrowave");
        request.packedEngine = json.getBoolean("packed_engine");
        request.base = json.getInt("base");
        request.decodable = json.getInt("decodable");
        request.decoderReason = json.getString("decoder_reason");
        request.hostScm = json.getInt("host_scm");
        request.hostPacked = json.isNull("host_packed") ? -1 : json.getInt("host_packed");
        request.packedRefusedBefore = json.getBoolean("packed_refused_before");
        return request;
    }

    @Test
    public void theBindingSpeaksTheSharedContract() throws Exception {
        JSONArray cases = contract();
        Map<String, String> answers = new HashMap<>();
        for (int i = 0; i < cases.length(); i++) {
            JSONObject c = cases.getJSONObject(i);
            answers.put(canonical(c.getJSONObject("request")), c.getJSONObject("response").toString());
        }
        int decided = 0;
        for (int i = 0; i < cases.length(); i++) {
            JSONObject c = cases.getJSONObject(i);
            JSONObject ok = c.getJSONObject("response").getJSONObject("ok");
            if (ok.has("need_device_modes")) continue;
            JSONObject sent = c.getJSONObject("request");
            int[] probes = {0};
            SharpColor.use(request -> {
                try {
                    JSONObject json = new JSONObject(request);
                    String answer = answers.get(canonical(json));
                    if (answer != null) return answer;
                    // The engine asks for the modes whenever the decision reaches packed without them.
                    assertTrue(c.getString("name") + ": not a contract request: " + request, json.isNull("device_modes"));
                    return "{\"ok\":{\"need_device_modes\":true}}";
                } catch (Exception e) {
                    throw new AssertionError(e);
                }
            });
            boolean verified = !sent.isNull("device_modes");
            SharpColor.Pick pick = SharpColor.decide(request(sent), () -> {
                probes[0]++;
                try {
                    return verified ? sent.getInt("device_modes") : 0;
                } catch (org.json.JSONException e) {
                    throw new AssertionError(e);
                }
            });
            String name = c.getString("name");
            // The device's modes cost a decode: asked only when the case carries them.
            assertEquals(name, verified ? 1 : 0, probes[0]);
            switch (ok.getString("pick")) {
                case "native":
                    assertEquals(name, SharpColor.Pick.Kind.NATIVE, pick.kind);
                    assertEquals(name, ok.getInt("formats"), pick.formats);
                    break;
                case "packed":
                    assertEquals(name, SharpColor.Pick.Kind.PACKED, pick.kind);
                    assertEquals(name, ok.getInt("modes"), pick.modes);
                    break;
                default:
                    assertEquals(name, SharpColor.Pick.Kind.WITHOUT, pick.kind);
                    assertEquals(name, ok.getString("why"), pick.why);
                    String detail = ok.optString("detail", "");
                    assertEquals(name, detail.isEmpty() ? null : detail, pick.detail);
                    assertEquals(name, ok.getBoolean("tell"), pick.worthSaying());
            }
            decided++;
        }
        assertTrue(decided >= 6);
    }

    @Test
    public void onlyAReasonThePlayerCanActOnIsSaid() throws Exception {
        for (String why : new String[] {"off", "pyrowave", "hdr", SharpColor.Pick.UNANSWERED}) {
            assertFalse(why, SharpColor.read(new JSONObject().put("pick", "without").put("why", why)).worthSaying());
        }
        for (String why : new String[] {"decoder", "host", "device", "no_common_mode"}) {
            assertTrue(why, SharpColor.read(new JSONObject().put("pick", "without").put("why", why)).worthSaying());
        }
        assertTrue(SharpColor.read(new JSONObject().put("pick", "without").put("why", "no_common_mode")).fromHost());
        assertFalse(SharpColor.read(new JSONObject().put("pick", "without").put("why", "device")).fromHost());
        assertFalse(SharpColor.read(new JSONObject().put("pick", "packed").put("modes", 1)).worthSaying());
    }

    @Test
    public void aModelThatCannotAnswerStartsWithoutEitherAndSilently() {
        SharpColor.use(request -> { throw new UnsatisfiedLinkError("no library"); });
        SharpColor.Pick pick = SharpColor.decide(new SharpColor.Request(), () -> { throw new AssertionError("no probe"); });
        assertEquals(SharpColor.Pick.Kind.WITHOUT, pick.kind);
        assertFalse(pick.worthSaying());
        SharpColor.use(request -> "{\"error\":\"invalid\",\"detail\":\"x\"}");
        assertEquals(SharpColor.Pick.Kind.WITHOUT, SharpColor.decide(new SharpColor.Request(), () -> 0).kind);
    }
}
