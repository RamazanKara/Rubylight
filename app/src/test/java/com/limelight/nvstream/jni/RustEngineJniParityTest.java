package com.limelight.nvstream.jni;

import org.junit.Test;

import java.io.File;
import java.io.IOException;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.TreeMap;
import java.util.TreeSet;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Stream;

import static org.junit.Assert.*;

/**
 * The Rust engine (protocol/android) and the Java glue are built separately, and a mismatch only
 * shows as an UnsatisfiedLinkError or a JNI crash on a device. This reads the Rust sources
 * (every .rs file under protocol/android/src, submodules included) and checks that
 * <ul>
 * <li>every RustBridge native has a Rust export whose JNI type at each parameter position, and
 * whose return type, match the Java declaration, and no export lacks a native;</li>
 * <li>startConnection, whose many int parameters the types alone cannot tell apart, also has the
 * same names in the same order;</li>
 * <li>every MoonBridge c* native has a RustBridge counterpart with the same signature, or is on
 * the explicit list below;</li>
 * <li>every MoonBridge callback the Rust bridge looks up exists with the same signature.</li>
 * </ul>
 */
public class RustEngineJniParityTest {
    private static final String CLASS_PREFIX = "Java_com_limelight_nvstream_jni_RustBridge_";

    /** MoonBridge c* natives that the Rust engine does not need, and why. */
    private static final Map<String, String> C_ONLY = new TreeMap<>();
    static {
        C_ONLY.put("cSetNetworkGracePeriodMs", "folded into RustBridge.startConnection (networkGracePeriodMs)");
        C_ONLY.put("cStartConnection", "RustBridge.startConnection has its own parameter list");
    }

    /** A Rust name that differs from the Java one for the same startConnection parameter. */
    private static final Map<String, String> RENAMED = new TreeMap<>();
    static {
        RENAMED.put("network_grace_ms", "network_grace_period_ms");
    }

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

    /** A Rust export: JNI parameter types and names after JNIEnv and the class, and the return type. */
    static final class Export {
        final List<String> types = new ArrayList<>();
        final List<String> names = new ArrayList<>();
        String returns = "V";
    }

    /** The JNI descriptor a Rust type stands for. Object types other than String and arrays are "L". */
    private static String jniDescriptor(String rustType) {
        switch (rustType) {
            case "jint": return "I";
            case "jlong": return "J";
            case "jshort": return "S";
            case "jbyte": return "B";
            case "jboolean": return "Z";
            case "jfloat": return "F";
            case "jdouble": return "D";
            case "jchar": return "C";
            case "JString": case "jstring": return "Ljava/lang/String;";
            case "JByteArray": case "jbyteArray": return "[B";
            case "JShortArray": case "jshortArray": return "[S";
            case "JIntArray": case "jintArray": return "[I";
            case "JLongArray": case "jlongArray": return "[J";
            case "JFloatArray": case "jfloatArray": return "[F";
            case "JObject": case "jobject": return "L";
            default: throw new AssertionError("unknown JNI type in the Rust bridge: " + rustType);
        }
    }

    private static List<File> rustFiles(File directory) throws IOException {
        List<File> files = new ArrayList<>();
        try (Stream<Path> walk = Files.walk(directory.toPath())) {
            walk.filter(path -> path.toString().endsWith(".rs")).sorted().forEach(path -> files.add(path.toFile()));
        }
        return files;
    }

    /** Splits a parameter list at the commas that are not inside generics or parentheses. */
    private static List<String> splitParameters(String list) {
        List<String> parts = new ArrayList<>();
        int depth = 0;
        StringBuilder current = new StringBuilder();
        for (char c : list.toCharArray()) {
            if (c == '<' || c == '(') depth++;
            if (c == '>' || c == ')') depth--;
            if (c == ',' && depth == 0) {
                parts.add(current.toString());
                current.setLength(0);
            } else {
                current.append(c);
            }
        }
        if (!current.toString().trim().isEmpty()) {
            parts.add(current.toString());
        }
        return parts;
    }

    static Map<String, Export> parseExports(String text, String prefix) {
        Map<String, Export> exports = new TreeMap<>();
        Matcher head = Pattern.compile("fn " + prefix + "(\\w+)\\s*\\(").matcher(text);
        Pattern parameter = Pattern.compile("^\\s*(?:mut\\s+)?(\\w+)\\s*:\\s*(\\w+)");
        while (head.find()) {
            int depth = 1;
            int index = head.end();
            while (depth > 0) {
                char c = text.charAt(index++);
                if (c == '(') depth++;
                if (c == ')') depth--;
            }
            Export export = new Export();
            List<String> parameters = splitParameters(text.substring(head.end(), index - 1));
            for (int i = 0; i < parameters.size(); i++) {
                Matcher match = parameter.matcher(parameters.get(i));
                assertTrue("cannot read parameter '" + parameters.get(i) + "' of " + head.group(1), match.find());
                if (i >= 2) { // JNIEnv and the class come first
                    export.names.add(match.group(1));
                    export.types.add(match.group(2));
                } else {
                    assertEquals(head.group(1) + " parameter " + i, i == 0 ? "JNIEnv" : "JClass", match.group(2));
                }
            }
            Matcher result = Pattern.compile("^\\s*(?:->\\s*(\\w+))?\\s*\\{").matcher(text.substring(index));
            assertTrue("cannot read the return type of " + head.group(1), result.find());
            export.returns = result.group(1) == null ? "V" : jniDescriptor(result.group(1));
            assertNull("exported twice: " + head.group(1), exports.put(head.group(1), export));
        }
        return exports;
    }

    private static Map<String, Export> rustExports() throws IOException {
        Map<String, Export> exports = new TreeMap<>();
        for (File file : rustFiles(rustSources())) {
            String text = new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8);
            for (Map.Entry<String, Export> entry : parseExports(text, CLASS_PREFIX).entrySet()) {
                assertNull("exported twice: " + entry.getKey(), exports.put(entry.getKey(), entry.getValue()));
            }
        }
        return exports;
    }

    /** Object types other than String and arrays are all "L" on the Rust side. */
    private static boolean sameType(String javaDescriptor, String rustDescriptor) {
        return javaDescriptor.equals(rustDescriptor)
                || (rustDescriptor.equals("L") && javaDescriptor.startsWith("L"));
    }

    /** The first mismatch between a Java method and a Rust export, or null. */
    static String mismatch(Method method, Export export) {
        Class<?>[] parameters = method.getParameterTypes();
        if (parameters.length != export.types.size()) {
            return method.getName() + " has " + parameters.length + " parameters in Java and "
                    + export.types.size() + " in Rust";
        }
        for (int i = 0; i < parameters.length; i++) {
            String java = descriptor(parameters[i]);
            String rust = jniDescriptor(export.types.get(i));
            if (!sameType(java, rust)) {
                return method.getName() + " parameter " + (i + 1) + " (" + export.names.get(i) + "): Java "
                        + java + ", Rust " + export.types.get(i);
            }
        }
        String java = descriptor(method.getReturnType());
        if (!sameType(java, export.returns)) {
            return method.getName() + " returns " + java + " in Java and " + export.returns + " in Rust";
        }
        return null;
    }

    @Test public void everyRustBridgeNativeMatchesItsRustExportParameterByParameter() throws IOException {
        Map<String, Export> exports = rustExports();
        TreeSet<String> natives = new TreeSet<>();
        for (Method method : RustBridge.class.getDeclaredMethods()) {
            if (!Modifier.isNative(method.getModifiers())) {
                continue;
            }
            natives.add(method.getName());
            Export export = exports.get(method.getName());
            assertNotNull("no Rust export for RustBridge." + method.getName(), export);
            assertNull(mismatch(method, export), mismatch(method, export));
        }
        TreeSet<String> orphans = new TreeSet<>(exports.keySet());
        orphans.removeAll(natives);
        assertTrue("Rust exports without a RustBridge native: " + orphans, orphans.isEmpty());
        assertTrue(natives.contains("startConnection"));
    }

    @Test public void startConnectionHasTheSameParameterNamesInTheSameOrder() throws IOException {
        // Many of its parameters share a type, so a swap of two ints passes the type check.
        String source = new String(Files.readAllBytes(new File(
                "src/main/java/com/limelight/nvstream/jni/RustBridge.java").toPath()), StandardCharsets.UTF_8);
        Matcher declaration = Pattern.compile("static native int startConnection\\((.*?)\\);", Pattern.DOTALL)
                .matcher(source);
        assertTrue(declaration.find());
        List<String> javaNames = new ArrayList<>();
        for (String parameter : splitParameters(declaration.group(1))) {
            String[] words = parameter.trim().split("\\s+");
            javaNames.add(words[words.length - 1].replaceAll("([A-Z])", "_$1").toLowerCase(Locale.ROOT));
        }
        List<String> rustNames = new ArrayList<>();
        for (String name : rustExports().get("startConnection").names) {
            rustNames.add(RENAMED.containsKey(name) ? RENAMED.get(name) : name);
        }
        assertEquals(javaNames, rustNames);
    }

    @Test public void everyMoonBridgeNativeHasARustBridgeCounterpartOrIsListedAsCOnly() throws Exception {
        Class<?> moonBridge = Class.forName("com.limelight.nvstream.jni.MoonBridge", false,
                RustEngineJniParityTest.class.getClassLoader());
        Map<String, Method> rust = new TreeMap<>();
        for (Method method : RustBridge.class.getDeclaredMethods()) {
            if (Modifier.isNative(method.getModifiers())) {
                rust.put(method.getName(), method);
            }
        }
        TreeSet<String> missing = new TreeSet<>();
        TreeSet<String> staleListEntries = new TreeSet<>(C_ONLY.keySet());
        for (Method method : moonBridge.getDeclaredMethods()) {
            String name = method.getName();
            if (!Modifier.isNative(method.getModifiers()) || !name.matches("c[A-Z]\\w*")) {
                continue;
            }
            staleListEntries.remove(name);
            if (C_ONLY.containsKey(name)) {
                continue;
            }
            String counterpart = Character.toLowerCase(name.charAt(1)) + name.substring(2);
            Method other = rust.get(counterpart);
            if (other == null) {
                missing.add(name);
            } else {
                assertEquals("signature of RustBridge." + counterpart + " against MoonBridge." + name,
                        descriptor(method), descriptor(other));
            }
        }
        assertTrue("MoonBridge natives with no RustBridge counterpart (add one, or list the reason in C_ONLY): "
                + missing, missing.isEmpty());
        assertTrue("C_ONLY entries that are not MoonBridge natives any more: " + staleListEntries,
                staleListEntries.isEmpty());
    }

    @Test public void theParserSeesTheTypesInAnExport() {
        Map<String, Export> exports = parseExports(
                "pub extern \"system\" fn Java_x_RustBridge_demo(\n    mut env: JNIEnv,\n    _: JClass,\n"
                        + "    a: jint,\n    b: JString<'local>,\n    c: jboolean,\n) -> jlong {\n}",
                "Java_x_RustBridge_");
        Export demo = exports.get("demo");
        assertEquals(Arrays.asList("jint", "JString", "jboolean"), demo.types);
        assertEquals(Arrays.asList("a", "b", "c"), demo.names);
        assertEquals("J", demo.returns);
    }

    @Test public void aSwappedOrReorderedParameterIsReported() throws Exception {
        String prefix = "Java_x_RustBridge_";
        Method button = RustBridge.class.getDeclaredMethod("sendMouseButton", byte.class, byte.class);
        Export fine = parseExports(
                "fn Java_x_RustBridge_sendMouseButton(_: JNIEnv, _: JClass, a: jbyte, b: jbyte) {}", prefix)
                .get("sendMouseButton");
        assertNull(mismatch(button, fine));

        Method position = RustBridge.class.getDeclaredMethod("sendMousePosition",
                short.class, short.class, short.class, short.class);
        Export wrong = parseExports("fn Java_x_RustBridge_sendMousePosition("
                + "_: JNIEnv, _: JClass, x: jshort, y: jshort, w: jint, h: jshort) {}", prefix)
                .get("sendMousePosition");
        assertTrue(mismatch(position, wrong).contains("parameter 3"));

        Method touch = RustBridge.class.getDeclaredMethod("sendTouchEvent", byte.class, int.class, float.class,
                float.class, float.class, float.class, float.class, short.class);
        Export swapped = parseExports("fn Java_x_RustBridge_sendTouchEvent(_: JNIEnv, _: JClass, e: jbyte, "
                + "p: jfloat, id: jint, x: jfloat, y: jfloat, pr: jfloat, m: jfloat, r: jshort) -> jint {}", prefix)
                .get("sendTouchEvent");
        assertTrue(mismatch(touch, swapped).contains("parameter 2"));

        Export badReturn = parseExports(
                "fn Java_x_RustBridge_sendMouseButton(_: JNIEnv, _: JClass, a: jbyte, b: jbyte) -> jint {}", prefix)
                .get("sendMouseButton");
        assertTrue(mismatch(button, badReturn).contains("returns"));
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
