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

    @Test public void moonBridgePassesRustBridgeItsArgumentsInTheDeclaredOrder() throws IOException {
        // The same swap that the types cannot catch, one call up: MoonBridge.startConnection hands
        // its parameters to RustBridge.startConnection by position.
        String rustBridge = new String(Files.readAllBytes(new File(
                "src/main/java/com/limelight/nvstream/jni/RustBridge.java").toPath()), StandardCharsets.UTF_8);
        String moonBridge = new String(Files.readAllBytes(new File(
                "src/main/java/com/limelight/nvstream/jni/MoonBridge.java").toPath()), StandardCharsets.UTF_8);
        Matcher declaration = Pattern.compile("static native int startConnection\\((.*?)\\);", Pattern.DOTALL)
                .matcher(rustBridge);
        Matcher call = Pattern.compile("RustBridge\\.startConnection\\((.*?)\\);", Pattern.DOTALL).matcher(moonBridge);
        assertTrue(declaration.find());
        assertTrue(call.find());
        // Passed under another name than RustBridge declares it.
        Map<String, String> passedAs = new TreeMap<>();
        passedAs.put("packed444Requested", "packed444");
        passedAs.put("packedModes", "packed444Modes");
        passedAs.put("roamingRequested", "roaming");
        passedAs.put("roamingMeteredRequested", "roamingMetered");
        passedAs.put("audioPassthroughRequested", "audioPassthrough");
        passedAs.put("hdrFormatRequested", "hdrFormat");
        passedAs.put("videoFecRequested", "videoFecPercentage");
        passedAs.put("pyrowaveEarlyRequested", "pyrowaveEarly");
        passedAs.put("codecDirectRequested", "codecDirect");
        passedAs.put("receiveDecodePriorityRequested", "receiveDecodePriority");
        passedAs.put("receiveCoalescingRequested", "receiveCoalescing");
        List<String> declared = new ArrayList<>();
        for (String parameter : splitParameters(declaration.group(1))) {
            String[] words = parameter.trim().split("\\s+");
            declared.add(words[words.length - 1]);
        }
        List<String> passed = new ArrayList<>();
        for (String argument : splitParameters(call.group(1))) {
            String name = argument.trim();
            passed.add(passedAs.containsKey(name) ? passedAs.get(name) : name);
        }
        assertEquals(declared, passed);
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

    /** The {@code pub const NAME: i32 = value;} constants of a Rust file whose names start with {@code prefix}. */
    private static Map<String, Integer> rustConstants(String text, String prefix) {
        Map<String, Integer> constants = new TreeMap<>();
        Matcher constant = Pattern.compile("pub const (" + prefix + "\\w+): i32 = (-?\\d+);").matcher(text);
        while (constant.find()) {
            assertNull("declared twice: " + constant.group(1),
                    constants.put(constant.group(1), Integer.parseInt(constant.group(2))));
        }
        return constants;
    }

    @Test public void frameWarpCodesAgreeAcrossRustJavaAndTheNativeRenderers() throws IOException {
        // A swap would draw an unmasked HUD shifted, or a stale mask; the overlay would name the wrong state.
        String rust = read("frame_warp.rs");
        Map<String, Integer> status = rustConstants(rust, "STATUS_");
        status.remove("STATUS_NUMBERS");
        Map<String, Integer> expected = new TreeMap<>();
        expected.put("STATUS_OFF", MoonBridge.FRAME_WARP_OFF);
        expected.put("STATUS_NOT_OFFERED", MoonBridge.FRAME_WARP_NOT_OFFERED);
        expected.put("STATUS_LEARNING", MoonBridge.FRAME_WARP_LEARNING);
        expected.put("STATUS_MOUSE", MoonBridge.FRAME_WARP_MOUSE);
        expected.put("STATUS_STICK", MoonBridge.FRAME_WARP_STICK);
        expected.put("STATUS_DECLINED", MoonBridge.FRAME_WARP_OFF_FOR_STREAM);
        assertEquals(expected, status);

        Map<String, Integer> codes = rustConstants(rust, "(?:WARP|MASK)_");
        Map<String, Integer> java = new TreeMap<>();
        java.put("WARP_NO_MASK", MoonBridge.FRAME_WARP_NO_MASK);
        java.put("WARP_INACTIVE", MoonBridge.FRAME_WARP_INACTIVE);
        java.put("MASK_UNCHANGED", MoonBridge.FRAME_WARP_MASK_UNCHANGED);
        java.put("MASK_CLEARED", MoonBridge.FRAME_WARP_MASK_CLEARED);
        java.put("MASK_TOO_SMALL", MoonBridge.FRAME_WARP_MASK_TOO_SMALL);
        assertEquals(java, codes);

        String c = new String(Files.readAllBytes(new File("src/main/jni/frame_warp_api.h").toPath()),
                StandardCharsets.UTF_8);
        Map<String, Integer> defines = new TreeMap<>();
        Matcher define = Pattern.compile("#define FRAME_WARP_(\\w+) \\(?(-?\\d+)\\)?").matcher(c);
        while (define.find()) {
            defines.put(define.group(1), Integer.parseInt(define.group(2)));
        }
        // The header leaves the WARP_ off the warp codes: FRAME_WARP_NO_MASK, FRAME_WARP_INACTIVE.
        Map<String, Integer> fromRust = new TreeMap<>();
        for (Map.Entry<String, Integer> code : codes.entrySet()) {
            String name = code.getKey();
            fromRust.put(name.startsWith("WARP_") ? name.substring("WARP_".length()) : name, code.getValue());
        }
        Matcher version = Pattern.compile("pub const API_VERSION: u32 = (\\d+);").matcher(rust);
        assertTrue(version.find());
        fromRust.put("API_VERSION", Integer.parseInt(version.group(1)));
        // How many floats warp() and peek() write: a renderer with a shorter array would be written past its end.
        Matcher floats = Pattern.compile("pub const WARP_FLOATS: usize = (\\d+);").matcher(rust);
        assertTrue(floats.find());
        fromRust.put("FLOATS", Integer.parseInt(floats.group(1)));
        assertEquals(fromRust, defines);
        assertEquals(MoonBridge.FRAME_WARP_FLOATS, (int) defines.get("FLOATS"));
    }

    @Test public void frameWarpWritesTheShiftThenTheLensTermsEverywhere() throws IOException {
        // Eight floats: the centre's shift as fractions of the picture and in pixels, then the lens terms. A renderer
        // that read them in another order would turn the picture by its shift in pixels.
        String rust = read("frame_warp.rs");
        assertTrue(rust.contains("pub const API_VERSION: u32 = 3;"));
        assertTrue(rust.contains("pub const WARP_FLOATS: usize = 8;"));
        assertTrue(rust.contains(
                "pub const WARP_STILL: [f32; WARP_FLOATS] = [0.0, 0.0, 0.0, 0.0, FLAT[0], FLAT[1], FLAT[2], FLAT[3]];"));
        assertTrue(rust.contains("let [kx, ky, nx, ny] = warp.lens;\n"
                + "    (code, [warp.uv[0], warp.uv[1], warp.pixels[0], warp.pixels[1], kx, ky, nx, ny])"));
        assertTrue(rust.contains("unsafe fn write_shift(values: &[f32; WARP_FLOATS], out: *mut f32) {"));
        // Java's array is as long as Rust writes, and a shorter one draws the picture as it is.
        String lib = read("lib.rs");
        assertEquals(3, lib.split(Pattern.quote("if env.get_array_length(&out).unwrap_or(0) < values.len() as i32"),
                -1).length);
        String gles = new String(Files.readAllBytes(
                new File("src/main/java/com/limelight/binding/video/GlesUpscaler.java").toPath()), StandardCharsets.UTF_8);
        assertTrue(gles.contains("private final float[] warpOut = new float[MoonBridge.FRAME_WARP_FLOATS];"));
        // The native renderers size theirs by the header, and a picture drawn as it is has a flat picture's terms.
        String mask = new String(Files.readAllBytes(new File("src/main/jni/frame_warp_mask.h").toPath()),
                StandardCharsets.UTF_8).replace("\r\n", "\n");
        assertTrue(mask.contains("for (int i = 0; i < FRAME_WARP_FLOATS; ++i) {\n        out[i] = i >= 6 ? 1.0f : 0.0f;"));
        for (String renderer : new String[] {"vulkan/vulkan_renderer.cpp", "pyrowave-renderer/pyrowave_renderer.cpp"}) {
            String cpp = new String(Files.readAllBytes(new File("src/main/jni/" + renderer).toPath()),
                    StandardCharsets.UTF_8).replace("\r\n", "\n");
            assertFalse(renderer, Pattern.compile("float (?:warpOut|out|peeked)\\[4\\]").matcher(cpp).find());
            assertTrue(renderer, cpp.contains("float warpOut[FRAME_WARP_FLOATS];\n"));
            assertTrue(renderer, cpp.contains("[FRAME_WARP_FLOATS] = {};\n"));
        }
    }

    @Test public void frameWarpTableHasTheSameFieldsInTheSameOrderInRustAndC() throws IOException {
        // The renderers call through this table by offset: a field out of order calls the wrong function.
        String rust = read("frame_warp.rs");
        int rustStart = rust.indexOf("pub struct FrameWarpApi {");
        assertTrue(rustStart >= 0);
        String rustBody = rust.substring(rustStart, rust.indexOf("\n}", rustStart));
        List<String> rustFields = new ArrayList<>();
        Matcher rustField = Pattern.compile("\\n\\s*pub (\\w+):").matcher(rustBody);
        while (rustField.find()) rustFields.add(rustField.group(1));

        String c = new String(Files.readAllBytes(new File("src/main/jni/frame_warp_api.h").toPath()),
                StandardCharsets.UTF_8);
        int cStart = c.indexOf("typedef struct FrameWarpApi {");
        assertTrue(cStart >= 0);
        String cBody = c.substring(cStart, c.indexOf("} FrameWarpApi;", cStart));
        List<String> cFields = new ArrayList<>();
        Matcher cField = Pattern.compile("\\n\\s*(?:uint32_t (\\w+);|[\\w ]+\\(\\*(\\w+)\\)\\()").matcher(cBody);
        while (cField.find()) cFields.add(cField.group(1) != null ? cField.group(1) : cField.group(2));

        assertEquals(Arrays.asList("version", "frame_for_pts", "warp", "mask", "peek", "input_seq", "stick_deflected"),
                rustFields);
        assertEquals(rustFields, cFields);
    }

    @Test public void networkChangedAnswersWithMoonBridgesNetworkCodes() throws IOException {
        // A swap would pass every type check: a stream that roamed would reconnect, or one that
        // cannot carry on would wait for its timeout. The constants are compile-time, so reading
        // them does not load the C library.
        Map<String, Integer> codes = new TreeMap<>();
        Matcher arm = Pattern.compile("NetworkChange::(\\w+)\\s*=>\\s*(\\d+)").matcher(read("lib.rs"));
        while (arm.find()) {
            assertNull("mapped twice: " + arm.group(1), codes.put(arm.group(1), Integer.parseInt(arm.group(2))));
        }
        Map<String, Integer> expected = new TreeMap<>();
        expected.put("CarriesOn", MoonBridge.NETWORK_CARRIES_ON);
        expected.put("Roamed", MoonBridge.NETWORK_ROAMED);
        expected.put("CannotContinue", MoonBridge.NETWORK_CANNOT_CONTINUE);
        assertEquals(expected, codes);
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

    @Test public void noControllerCapabilityAsksForSteamControllerHapticsOrSettings() throws Exception {
        // LI_CCAP_STEAM_HAPTIC (0x800) and LI_CCAP_STEAM_SETTINGS (0x2000) make a host send 0x5504
        // and 0x5505 for the real Steam Controller; bridgeClSteamHaptic/Settings are no-ops until a
        // Nightjar driver owns one. Every arrival capability is built from these constants.
        // Read from the source: reading the fields would run MoonBridge's static initializer, which
        // loads the C library.
        String source = new String(Files.readAllBytes(new File(
                "src/main/java/com/limelight/nvstream/jni/MoonBridge.java").toPath()), StandardCharsets.UTF_8);
        Matcher constant = Pattern.compile("short (LI_CCAP_\\w+)\\s*=\\s*(?:\\(short\\)\\s*)?0x([0-9A-Fa-f]+);")
                .matcher(source);
        int found = 0;
        while (constant.find()) {
            found++;
            int bits = Integer.parseInt(constant.group(2), 16);
            assertEquals(constant.group(1) + " overlaps the Steam Controller bits", 0, bits & (0x800 | 0x2000));
        }
        assertTrue("expected MoonBridge's LI_CCAP_* constants, found " + found, found >= 10);
        assertEquals("every LI_CCAP_* constant is a hex literal this test can read", found,
                source.split("short LI_CCAP_", -1).length - 1);
        // The callbacks exist so the Rust engine can look them up, and default to doing nothing.
        com.limelight.nvstream.NvConnectionListener.class.getMethod("steamHaptic", short.class, byte[].class);
        com.limelight.nvstream.NvConnectionListener.class.getMethod("steamSettings", short.class, byte.class,
                byte[].class);
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
