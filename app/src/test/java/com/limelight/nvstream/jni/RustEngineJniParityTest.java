package com.limelight.nvstream.jni;

import org.junit.Test;

import java.io.File;
import java.io.IOException;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Map;
import java.util.TreeMap;
import java.util.TreeSet;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import static org.junit.Assert.*;

/**
 * The Rust engine (protocol/android) and the Java glue are built separately, and a mismatch only
 * shows as an UnsatisfiedLinkError or a JNI crash on a device. This reads the Rust sources and
 * checks both directions: every RustBridge native has a Rust export with the same parameter
 * count, and every MoonBridge callback the Rust bridge looks up exists with the same signature.
 */
public class RustEngineJniParityTest {
    private static final String CLASS_PREFIX = "Java_com_limelight_nvstream_jni_RustBridge_";

    private static File rustSources() {
        for (String path : new String[] {"../protocol/android/src", "protocol/android/src"}) {
            File directory = new File(path);
            if (directory.isDirectory()) {
                return directory;
            }
        }
        throw new AssertionError("protocol/android/src not found from " + new File(".").getAbsolutePath());
    }

    private static String read(String name) throws IOException {
        return new String(Files.readAllBytes(new File(rustSources(), name).toPath()), StandardCharsets.UTF_8);
    }

    /** Export name to Rust parameter count, without the JNIEnv and class arguments. */
    private static Map<String, Integer> rustExports() throws IOException {
        Map<String, Integer> exports = new TreeMap<>();
        File[] files = rustSources().listFiles((dir, name) -> name.endsWith(".rs"));
        assertNotNull(files);
        Pattern function = Pattern.compile("fn " + CLASS_PREFIX + "(\\w+)\\s*\\((.*?)\\)\\s*(?:->|\\{)", Pattern.DOTALL);
        Pattern parameter = Pattern.compile("\\w+\\s*:\\s*[A-Za-z]");
        for (File file : files) {
            String text = new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
            Matcher matcher = function.matcher(text);
            while (matcher.find()) {
                Matcher params = parameter.matcher(matcher.group(2));
                int count = 0;
                while (params.find()) {
                    count++;
                }
                assertNull("exported twice: " + matcher.group(1), exports.put(matcher.group(1), count - 2));
            }
        }
        return exports;
    }

    @Test public void everyRustBridgeNativeHasARustExportOfTheSameArity() throws IOException {
        Map<String, Integer> exports = rustExports();
        TreeSet<String> natives = new TreeSet<>();
        for (Method method : RustBridge.class.getDeclaredMethods()) {
            if (!Modifier.isNative(method.getModifiers())) {
                continue;
            }
            natives.add(method.getName());
            Integer arity = exports.get(method.getName());
            assertNotNull("no Rust export for RustBridge." + method.getName(), arity);
            assertEquals("parameter count of " + method.getName(), method.getParameterCount(), (int) arity);
        }
        TreeSet<String> orphans = new TreeSet<>(exports.keySet());
        orphans.removeAll(natives);
        assertTrue("Rust exports without a RustBridge native: " + orphans, orphans.isEmpty());
        assertTrue(natives.contains("startConnection"));
    }

    private static String descriptor(Class<?> type) {
        if (type == void.class) return "V";
        if (type == int.class) return "I";
        if (type == long.class) return "J";
        if (type == short.class) return "S";
        if (type == byte.class) return "B";
        if (type == char.class) return "C";
        if (type == boolean.class) return "Z";
        if (type == float.class) return "F";
        if (type == double.class) return "D";
        if (type.isArray()) return "[" + descriptor(type.getComponentType());
        return "L" + type.getName().replace('.', '/') + ";";
    }

    private static String descriptor(Method method) {
        StringBuilder text = new StringBuilder("(");
        for (Class<?> parameter : method.getParameterTypes()) {
            text.append(descriptor(parameter));
        }
        return text.append(')').append(descriptor(method.getReturnType())).toString();
    }

    @Test public void everyCallbackTheRustBridgeLooksUpExistsInMoonBridge() throws Exception {
        // Not initialized: MoonBridge's static initializer loads the C library.
        Class<?> moonBridge = Class.forName("com.limelight.nvstream.jni.MoonBridge", false,
                RustEngineJniParityTest.class.getClassLoader());
        Map<String, String> callbacks = new TreeMap<>();
        for (Method method : moonBridge.getDeclaredMethods()) {
            if (method.getName().startsWith("bridge")) {
                callbacks.put(method.getName(), descriptor(method));
            }
        }
        Matcher lookup = Pattern.compile("id\\(\"(bridge\\w+)\",\\s*\"([^\"]+)\"\\)").matcher(read("bridge.rs"));
        int found = 0;
        while (lookup.find()) {
            found++;
            assertEquals("signature of MoonBridge." + lookup.group(1), lookup.group(2),
                    callbacks.get(lookup.group(1)));
        }
        assertTrue("expected the Rust bridge to look up its callbacks, found " + found, found >= 28);
    }

    @Test public void theRustEngineHasNoCallbackTheJavaSideLacks() throws Exception {
        // The reverse direction: a bridge* method added to MoonBridge for the C engine must be
        // looked up by the Rust bridge too, or be listed here as C-only on purpose.
        Class<?> moonBridge = Class.forName("com.limelight.nvstream.jni.MoonBridge", false,
                RustEngineJniParityTest.class.getClassLoader());
        String bridge = read("bridge.rs");
        TreeSet<String> missing = new TreeSet<>();
        for (Method method : moonBridge.getDeclaredMethods()) {
            if (method.getName().startsWith("bridge") && !bridge.contains("\"" + method.getName() + "\"")) {
                missing.add(method.getName());
            }
        }
        assertTrue("MoonBridge callbacks the Rust bridge never calls: " + missing, missing.isEmpty());
    }
}
