package com.limelight.utils;

import android.app.Application;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.annotation.Config;

import java.io.File;
import java.lang.reflect.Field;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Set;

import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

/**
 * Replays the shared cases in protocol/client/tests/fixtures/redact against RedactedLog.redact,
 * so it stays equal to nightjar-core's report::redact (cargo replays the same files). org.json
 * is only real under Robolectric.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class RedactedLogContractTest {
    // Gradle runs unit tests in the app module.
    private static final File CASES = new File("../protocol/client/tests/fixtures/redact");

    @After public void forgetSecrets() throws Exception {
        secrets().clear();
    }

    @Test public void everyCaseMatchesRedactedLog() throws Exception {
        assertTrue("no redaction cases under " + CASES, replayAll(CASES) > 0);
    }

    @Test public void aMissingCaseFolderFails() {
        assertThrows(AssertionError.class, () -> replayAll(new File("../protocol/client/tests/fixtures/no-cases")));
    }

    @Test public void aWrongExpectationFails() throws Exception {
        String json = "{\"name\": \"wrong\", \"input\": \"PIN 1234\", \"expect\": \"%s\"}";
        replay(new JSONObject(String.format(json, "PIN [omitted]")));
        AssertionError error = assertThrows(AssertionError.class,
                () -> replay(new JSONObject(String.format(json, "PIN 1234"))));
        assertTrue(error.getMessage(), error.getMessage().contains("got <PIN [omitted]>"));
    }

    /** Replays every case and returns how many ran. */
    private static int replayAll(File root) throws Exception {
        assertTrue("missing case folder " + root.getCanonicalPath(), root.isDirectory());
        File[] files = root.listFiles((dir, name) -> name.endsWith(".json"));
        assertTrue(files != null);
        Arrays.sort(files);
        List<String> failures = new ArrayList<>();
        int replayed = 0;
        for (File file : files) {
            JSONArray cases = new JSONObject(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8))
                    .getJSONArray("cases");
            for (int i = 0; i < cases.length(); i++) {
                JSONObject c = cases.getJSONObject(i);
                try {
                    replay(c);
                } catch (AssertionError e) {
                    failures.add(file.getName() + " / " + c.getString("name") + ": " + e.getMessage());
                }
                replayed++;
            }
        }
        assertTrue(failures.size() + " of " + replayed + " cases failed:\n" + String.join("\n", failures),
                failures.isEmpty());
        return replayed;
    }

    private static void replay(JSONObject c) throws Exception {
        // RedactedLog keeps its secrets for the process; each case starts with its own.
        secrets().clear();
        JSONArray list = c.optJSONArray("secrets");
        for (int i = 0; list != null && i < list.length(); i++) {
            RedactedLog.addSecret(list.getString(i));
        }
        String got = RedactedLog.redact(c.getString("input"));
        String expect = c.getString("expect");
        if (!expect.equals(got)) {
            throw new AssertionError("expected <" + expect + ">, got <" + got + ">");
        }
    }

    @SuppressWarnings("unchecked")
    private static Set<String> secrets() throws Exception {
        Field field = RedactedLog.class.getDeclaredField("SECRETS");
        field.setAccessible(true);
        return (Set<String>) field.get(null);
    }
}
