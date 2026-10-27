package com.limelight.policy;

import android.app.Application;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.annotation.Config;

import java.io.File;
import java.lang.reflect.Constructor;
import java.lang.reflect.Executable;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;

import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

/**
 * Replays the shared traces in protocol/policy/tests/traces (format in the crate's trace.rs)
 * against the Java policies, so they stay equal to nightjar-policy. org.json is only real under
 * Robolectric.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class PolicyTraceTest {
    // Gradle runs unit tests in the app module.
    private static final File TRACES = new File("../protocol/policy/tests/traces");
    /** Every module folder, each with at least one Java trace, so a renamed or emptied folder fails. */
    private static final List<String> MODULES = Arrays.asList("bitrate", "health", "launch", "pyrowave", "quality", "recovery");
    private static final String RECOVERY = "{\"rust\": \"recovery::StreamRecoveryPolicy\", \"java\": \"com.limelight.StreamRecoveryPolicy\","
            + " \"new\": [0, 0], \"calls\": [{\"call\": \"next_delay\", \"args\": [%s, -1], \"expect\": 1000}]}";

    @Test public void everyTraceMatchesTheJavaPolicy() throws Exception {
        assertTrue("no Java traces under " + TRACES, replayAll(TRACES, MODULES) > 0);
    }

    @Test public void aMissingTraceFolderFails() {
        assertThrows(AssertionError.class, () -> replayAll(new File("../protocol/policy/tests/no-traces"), MODULES));
    }

    @Test public void missingModulesAndOtherFilesFail() throws Exception {
        File root = Files.createTempDirectory("policy-traces").toFile();
        File recovery = new File(root, "recovery");
        assertTrue(recovery.mkdir());
        Files.write(new File(recovery, "a.json").toPath(), String.format(RECOVERY, 5).getBytes(StandardCharsets.UTF_8));
        List<String> recoveryOnly = Collections.singletonList("recovery");
        assertTrue(replayAll(root, recoveryOnly) == 1);
        assertThrows(AssertionError.class, () -> replayAll(root, Arrays.asList("bitrate", "recovery")));
        File bitrate = new File(root, "bitrate");
        assertTrue(bitrate.mkdir());
        assertThrows(AssertionError.class, () -> replayAll(root, Arrays.asList("bitrate", "recovery")));
        assertTrue(bitrate.delete());
        Files.write(new File(recovery, "b.JSON").toPath(), String.format(RECOVERY, 5).getBytes(StandardCharsets.UTF_8));
        AssertionError error = assertThrows(AssertionError.class, () -> replayAll(root, recoveryOnly));
        assertTrue(error.getMessage(), error.getMessage().contains("b.JSON: not a .json trace"));
        for (File file : sorted(recovery)) assertTrue(file.delete());
        assertTrue(recovery.delete() && root.delete());
    }

    @Test public void aFractionalIntegerFails() {
        AssertionError error = assertThrows(AssertionError.class,
                () -> replay(new JSONObject(String.format(RECOVERY, "5.5"))));
        assertTrue(error.getMessage(), error.getMessage().contains("argument 0 is not an integer: 5.5"));
    }

    @Test public void aWrongExpectationFails() throws Exception {
        String trace = "{\"rust\": \"recovery::StreamRecoveryPolicy\", \"java\": \"com.limelight.StreamRecoveryPolicy\","
                + " \"new\": [0, 0], \"calls\": [{\"call\": \"next_delay\", \"args\": [5, -1], \"expect\": %d}]}";
        replay(new JSONObject(String.format(trace, 1000)));
        AssertionError error = assertThrows(AssertionError.class,
                () -> replay(new JSONObject(String.format(trace, 2000))));
        assertTrue(error.getMessage(), error.getMessage().contains("expected 2000, got 1000"));
    }

    /** Replays every trace with a Java class and returns how many ran. */
    private static int replayAll(File root, List<String> modules) throws Exception {
        assertTrue("missing trace folder " + root.getCanonicalPath(), root.isDirectory());
        List<String> failures = new ArrayList<>();
        List<String> found = new ArrayList<>();
        int replayed = 0;
        for (File module : sorted(root)) {
            if (!module.isDirectory()) {
                failures.add(module + " is not a module folder");
                continue;
            }
            int traces = 0;
            for (File file : sorted(module)) {
                // Only exactly .json: a trace named .JSON or .json.bak would silently not run.
                if (!file.getName().endsWith(".json")) {
                    failures.add(file + ": not a .json trace");
                    continue;
                }
                try {
                    JSONObject trace = new JSONObject(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
                    // Desktop-only policy has no Java class; cargo replays it.
                    if (!trace.has("java")) throw new AssertionError("no java class (use null for desktop-only policy)");
                    if (trace.isNull("java")) continue;
                    replay(trace);
                    traces++;
                } catch (Exception | AssertionError e) {
                    failures.add(file + ": " + e);
                }
            }
            if (traces == 0) failures.add(module + ": no Java traces");
            replayed += traces;
            found.add(module.getName());
        }
        if (!found.equals(modules)) failures.add("module folders under " + root + " are " + found + ", expected " + modules);
        assertTrue(String.join("\n", failures), failures.isEmpty());
        return replayed;
    }

    private static File[] sorted(File dir) {
        File[] files = dir.listFiles();
        assertTrue("cannot list " + dir, files != null);
        Arrays.sort(files);
        return files;
    }

    private static void replay(JSONObject trace) throws Exception {
        Class<?> type = Class.forName(trace.getString("java"));
        JSONArray steps = trace.getJSONArray("calls");
        assertTrue("no calls", steps.length() > 0);
        Object subject = construct(type, trace.get("new"));
        for (int i = 0; i < steps.length(); i++) {
            JSONObject step = steps.getJSONObject(i);
            if (step.has("new")) {
                subject = construct(type, step.get("new"));
                continue;
            }
            String call = step.getString("call");
            JSONArray args = step.getJSONArray("args");
            Object expect = step.get("expect");
            Method method = (Method) find(type.getDeclaredMethods(), camelCase(call), args.length());
            boolean isStatic = Modifier.isStatic(method.getModifiers());
            if (!isStatic && subject == null) throw new AssertionError("step " + i + " " + call + ": no instance (new is null)");
            Object target = isStatic ? null : subject;
            Object actual = invoke(() -> method.invoke(target, convert(method, args)));
            if (!same(actual, expect)) {
                throw new AssertionError("step " + i + " " + call + args + ": expected " + expect + ", got " + actual);
            }
        }
    }

    private static Object construct(Class<?> type, Object args) throws Exception {
        if (args == JSONObject.NULL) return null;
        JSONArray list = (JSONArray) args;
        Constructor<?> constructor = (Constructor<?>) find(type.getDeclaredConstructors(), null, list.length());
        return invoke(() -> constructor.newInstance(convert(constructor, list)));
    }

    private static Executable find(Executable[] candidates, String name, int count) {
        Executable found = null;
        for (Executable candidate : candidates) {
            if ((name == null || candidate.getName().equals(name)) && candidate.getParameterCount() == count) {
                if (found != null) throw new AssertionError("ambiguous " + candidate);
                found = candidate;
            }
        }
        if (found == null) throw new AssertionError("no " + (name == null ? "constructor" : name) + " with " + count + " arguments");
        found.setAccessible(true);
        return found;
    }

    private interface Call { Object run() throws Exception; }

    private static Object invoke(Call call) throws Exception {
        try {
            return call.run();
        } catch (InvocationTargetException e) {
            if (e.getCause() instanceof Error) throw (Error) e.getCause();
            if (e.getCause() instanceof Exception) throw (Exception) e.getCause();
            throw e;
        }
    }

    private static Object[] convert(Executable target, JSONArray args) throws Exception {
        Class<?>[] types = target.getParameterTypes();
        Object[] values = new Object[types.length];
        for (int i = 0; i < types.length; i++) {
            Object value = args.get(i);
            Class<?> type = types[i];
            if (value == JSONObject.NULL) {
                if (type.isPrimitive()) throw new AssertionError("argument " + i + " is null for " + type);
                values[i] = null;
            } else if (type == int.class || type == long.class) {
                // longValue() would truncate 1.5; Rust's Args::int refuses it too.
                if (!(value instanceof Integer || value instanceof Long)) {
                    throw new AssertionError("argument " + i + " is not an integer: " + value);
                }
                long number = ((Number) value).longValue();
                if (type == long.class) {
                    values[i] = number;
                } else {
                    if (number != (int) number) throw new AssertionError("argument " + i + " is not an int: " + value);
                    values[i] = (int) number;
                }
            } else if (type == double.class) {
                values[i] = ((Number) value).doubleValue();
            } else if (type == float.class) {
                // JSON has no NaN: a string such as "NaN" stands for it.
                values[i] = value instanceof String ? Float.parseFloat((String) value) : ((Number) value).floatValue();
            } else if (type == boolean.class || type == String.class) {
                values[i] = type == boolean.class ? (Boolean) value : (String) value;
            } else if (type.isEnum()) {
                values[i] = enumConstant(type, (String) value);
            } else {
                throw new AssertionError("unsupported argument type " + type);
            }
        }
        return values;
    }

    private static Object enumConstant(Class<?> type, String name) {
        for (Object constant : type.getEnumConstants()) {
            if (((Enum<?>) constant).name().equals(name)) return constant;
        }
        throw new AssertionError("no " + type.getSimpleName() + " " + name);
    }

    private static boolean same(Object actual, Object expect) {
        if (expect == JSONObject.NULL) return actual == null;
        if (actual instanceof Number && expect instanceof Number) {
            boolean floating = actual instanceof Double || actual instanceof Float || expect instanceof Double;
            return floating ? ((Number) actual).doubleValue() == ((Number) expect).doubleValue()
                    : ((Number) actual).longValue() == ((Number) expect).longValue();
        }
        return expect.equals(actual);
    }

    static String camelCase(String snake) {
        StringBuilder name = new StringBuilder();
        boolean upper = false;
        for (char c : snake.toCharArray()) {
            if (c == '_') {
                upper = true;
            } else {
                name.append(upper ? Character.toUpperCase(c) : c);
                upper = false;
            }
        }
        return name.toString();
    }
}
