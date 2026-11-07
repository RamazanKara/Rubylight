package com.limelight.ui;

import android.app.Application;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;
import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.Set;

import static org.junit.Assert.*;

/**
 * The wire contract with libnightjar_core: each request {@link StreamMenuModel} sends must be the one in
 * protocol/android/src/stream_menu_contract.json, and it must read the answer there as meant. The Rust test
 * {@code answers_each_request_of_the_shared_contract_exactly} checks that the JNI entry gives exactly those
 * answers, so the JVM tests' {@link RustStreamMenuModel} cannot hide a change in the JNI glue or the JSON.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class StreamMenuModelContractTest {
    @After
    public void forgetTheModel() {
        StreamMenuModel.use(null);
    }

    private static JSONArray cases() throws Exception {
        for (String path : new String[] {"../protocol/android/src/stream_menu_contract.json", "protocol/android/src/stream_menu_contract.json"}) {
            File file = new File(path);
            if (file.isFile()) return new JSONArray(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
        }
        throw new AssertionError("stream_menu_contract.json not found from " + new File(".").getAbsolutePath());
    }

    /** Answers the fixture's response once the request is exactly the fixture's request. */
    private static void answer(JSONObject fixture) throws JSONException {
        Object expected = fixture.get("request");
        String response = fixture.get("response").toString();
        StreamMenuModel.use(request -> {
            try {
                assertTrue(fixture.getString("name") + " sent " + request, same(expected, new JSONObject(request)));
            } catch (JSONException e) {
                throw new AssertionError(e);
            }
            return response;
        });
    }

    /**
     * A libnightjar_core from another build loads but lacks the call: the first menu opened crashed the stream on the UI
     * thread. Every failure of the call itself, and a null answer, is the model being unavailable.
     */
    @Test
    public void aCallThatFailsOrAnswersNothingIsRefusedAsUnavailable() {
        StreamMenuModel.use(request -> { throw new UnsatisfiedLinkError("No implementation found for call"); });
        StreamMenuModel.Refused e = refused(StreamMenuModel::picker);
        assertEquals("unavailable", e.code);
        assertTrue(e.detail.contains("No implementation found"));
        StreamMenuModel.use(request -> { throw new IllegalStateException("broken"); });
        assertEquals("unavailable", refused(() -> StreamMenuModel.chordLabel(new String[] {"A"})).code);
        StreamMenuModel.use(request -> null);
        assertEquals("unavailable", refused(StreamMenuModel::labels).code);
        StreamMenuModel.use(request -> "not json");
        assertEquals("invalid", refused(StreamMenuModel::picker).code);
    }

    private static StreamMenuModel.Refused refused(Call call) {
        try {
            call.run();
        } catch (StreamMenuModel.Refused e) {
            return e;
        } catch (JSONException e) {
            throw new AssertionError(e);
        }
        throw new AssertionError("not refused");
    }

    private interface Call {
        void run() throws StreamMenuModel.Refused, JSONException;
    }

    @Test
    public void everyRequestAndAnswerMatchesTheSharedContract() throws Exception {
        JSONArray cases = cases();
        Set<String> run = new HashSet<>();
        for (int i = 0; i < cases.length(); i++) {
            JSONObject fixture = cases.getJSONObject(i);
            String name = fixture.getString("name");
            answer(fixture);
            JSONObject response = fixture.getJSONObject("response");
            switch (name) {
                case "layout": {
                    StreamMenuModel.Layout layout = StreamMenuModel.layout(Arrays.asList(new StreamMenuModel.Offer("keyboard", null),
                            new StreamMenuModel.Offer("overlay", false), new StreamMenuModel.Offer("reconnect", null),
                            new StreamMenuModel.Offer("disconnect", null), new StreamMenuModel.Offer("quit_game", null)), 1);
                    assertEquals(5, layout.grid.size());
                    assertEquals(StreamMenuModel.Cell.Kind.TILE, layout.grid.get(0).kind);
                    assertEquals("keyboard", layout.grid.get(0).id);
                    assertNull(layout.grid.get(0).on);
                    assertEquals(0, layout.hotkey("y"));
                    assertEquals("overlay", layout.grid.get(1).id);
                    assertEquals(Boolean.FALSE, layout.grid.get(1).on);
                    assertEquals(1, layout.hotkey("rb"));
                    assertEquals(StreamMenuModel.MORE, layout.grid.get(2).id);
                    assertNull(layout.grid.get(2).hotkey);
                    assertEquals(StreamMenuModel.Cell.Kind.SHORTCUT, layout.grid.get(3).kind);
                    assertEquals(0, layout.grid.get(3).shortcut);
                    assertEquals(StreamMenuModel.Cell.Kind.ADD, layout.grid.get(4).kind);
                    assertEquals(Collections.singletonList("reconnect"), layout.more);
                    assertEquals(Arrays.asList(StreamMenuModel.CLOSE, "disconnect", "quit_game"), layout.footer);
                    break;
                }
                case "layout_with_buttons": {
                    java.util.Map<String, String> buttons = new java.util.HashMap<>();
                    buttons.put("y", "overlay");
                    buttons.put("x", "disconnect");
                    buttons.put("rb", "none");
                    StreamMenuModel.Layout layout = StreamMenuModel.layout(Arrays.asList(new StreamMenuModel.Offer("keyboard", null),
                            new StreamMenuModel.Offer("overlay", false), new StreamMenuModel.Offer("disconnect", null)), null, buttons);
                    assertEquals(1, layout.hotkey("y"));
                    assertEquals("y", layout.grid.get(1).hotkey);
                    assertNull(layout.grid.get(0).hotkey);
                    assertEquals(-1, layout.hotkey("x"));
                    assertEquals("disconnect", layout.buttons.get("x"));
                    assertEquals(-1, layout.hotkey("rb"));
                    assertNull(layout.buttons.get("rb"));
                    break;
                }
                case "layout_without_shortcuts": {
                    StreamMenuModel.Layout layout = StreamMenuModel.layout(Arrays.asList(new StreamMenuModel.Offer("overlay_page", null),
                            new StreamMenuModel.Offer("disconnect", null)), null);
                    assertEquals(1, layout.grid.size());
                    assertEquals("overlay_page", layout.grid.get(0).id);
                    assertTrue(layout.more.isEmpty());
                    assertEquals(Arrays.asList(StreamMenuModel.CLOSE, "disconnect"), layout.footer);
                    break;
                }
                case "layout_unknown_tile": {
                    StreamMenuModel.Refused e = refused(() -> StreamMenuModel.layout(
                            Collections.singletonList(new StreamMenuModel.Offer("teleport", null)), null));
                    assertEquals("invalid", e.code);
                    assertEquals("unknown tile teleport", e.detail);
                    break;
                }
                case "built_ins": {
                    List<StreamMenuModel.BuiltIn> builtIns = StreamMenuModel.builtIns(Arrays.asList(new String[] {"ALT", "TAB"},
                            new String[] {"CTRL", "SHIFT", "ESC", "MEDIA_PLAY_PAUSE"}));
                    JSONArray expected = response.getJSONArray("ok");
                    assertEquals(expected.length(), builtIns.size());
                    assertEquals("task_manager", builtIns.get(0).id);
                    assertEquals("Ctrl + Shift + Esc", builtIns.get(0).label);
                    assertArrayEquals(new String[] {"CTRL", "SHIFT", "ESC"}, builtIns.get(0).keys);
                    for (StreamMenuModel.BuiltIn builtIn : builtIns) assertNotEquals("switch_window", builtIn.id);
                    break;
                }
                case "picker": {
                    List<StreamMenuModel.Key> keys = StreamMenuModel.picker();
                    JSONArray expected = response.getJSONArray("ok");
                    assertEquals(expected.length(), keys.size());
                    for (int k = 0; k < keys.size(); k++) {
                        assertEquals(expected.getJSONObject(k).getString("key"), keys.get(k).name);
                        assertEquals(expected.getJSONObject(k).getString("label"), keys.get(k).label);
                        assertEquals(expected.getJSONObject(k).getBoolean("modifier"), keys.get(k).modifier);
                    }
                    break;
                }
                case "validate":
                    assertArrayEquals(new String[] {"CTRL", "SHIFT", "ESC"},
                            StreamMenuModel.validate("Task Manager", new String[] {"ESC", "CTRL", "SHIFT"}));
                    break;
                case "validate_no_keys": {
                    StreamMenuModel.Refused e = refused(() -> StreamMenuModel.validate("x", new String[0]));
                    assertEquals("chord_length", e.code);
                    assertEquals("A null detail is no detail", "", e.detail);
                    assertEquals("chord_length", e.getMessage());
                    break;
                }
                case "validate_unknown_key": {
                    StreamMenuModel.Refused e = refused(() -> StreamMenuModel.validate("Old", new String[] {"CTRL", "ANDROID_KEYCODE_300"}));
                    assertEquals("unsupported_key", e.code);
                    assertEquals("ANDROID_KEYCODE_300", e.detail);
                    break;
                }
                case "chord_label":
                    assertEquals("Ctrl + Shift + Esc", StreamMenuModel.chordLabel(new String[] {"CTRL", "SHIFT", "ESC"}));
                    break;
                case "chord_label_picked_modifier_last":
                    assertEquals("Shift + Caps Lock", StreamMenuModel.chordLabel(new String[] {"CAPS_LOCK", "SHIFT"}));
                    break;
                case "mode_line":
                    assertEquals("1440p · 144 FPS · HEVC HDR", StreamMenuModel.modeLine(2560, 1440, 144, 0x0200, true));
                    break;
                case "mode_line_unknown":
                    assertNull(StreamMenuModel.modeLine(0, 0, 0, 0, false));
                    break;
                case "labels": {
                    Map<String, String> labels = StreamMenuModel.labels();
                    JSONObject expected = response.getJSONObject("ok");
                    assertEquals(expected.length(), labels.size());
                    assertEquals("return", labels.get(StreamMenuModel.CLOSE));
                    assertEquals("add_shortcut", labels.get(StreamMenuModel.ADD));
                    for (Iterator<String> ids = expected.keys(); ids.hasNext(); ) {
                        String id = ids.next();
                        assertEquals(id, expected.getString(id), labels.get(id));
                        if (!id.equals(StreamMenuModel.ADD)) assertNotEquals(id, 0, StreamMenu.tileLabel(labels.get(id)));
                    }
                    break;
                }
                case "hotkeys": {
                    List<StreamMenuModel.Hotkey> hotkeys = StreamMenuModel.hotkeys(new HashMap<>());
                    assertEquals(response.getJSONArray("ok").length(), hotkeys.size());
                    StreamMenuModel.Hotkey menu = hotkeys.get(0);
                    assertEquals("stream_menu", menu.id);
                    assertEquals("Ctrl + Alt + Shift + Tab", menu.chord.label);
                    assertEquals(7, menu.chord.modifiers);
                    assertEquals(9, menu.chord.vk);
                    assertFalse(menu.changed);
                    assertEquals("Ctrl + Alt + Shift + M", menu.old.get(0).label);
                    assertArrayEquals(new String[] {"Ctrl", "Alt", "Shift", "Tab"}, menu.chord.caps);
                    assertNull(menu.taken);
                    break;
                }
                case "hotkeys_changed": {
                    Map<String, String> bindings = new HashMap<>();
                    bindings.put("cursor", "CTRL+F2");
                    bindings.put("overlay", "off");
                    for (StreamMenuModel.Hotkey hotkey : StreamMenuModel.hotkeys(bindings)) {
                        if (hotkey.id.equals("overlay")) {
                            assertNull(hotkey.chord);
                            assertTrue(hotkey.changed);
                        } else if (hotkey.id.equals("cursor")) {
                            assertArrayEquals(new String[] {"CTRL", "F2"}, hotkey.chord.keys);
                            assertEquals("Ctrl + Alt + Shift + P", hotkey.standard.label);
                            assertTrue(hotkey.old.isEmpty());
                        }
                    }
                    break;
                }
                case "check_hotkey":
                    assertArrayEquals(new String[] {"CTRL", "F2"}, StreamMenuModel.checkHotkey("cursor", new String[] {"F2", "CTRL"}, new HashMap<>()));
                    break;
                case "check_hotkey_taken": {
                    StreamMenuModel.Refused e = refused(() -> StreamMenuModel.checkHotkey("cursor",
                            new String[] {"CTRL", "ALT", "SHIFT", "Q"}, new HashMap<>()));
                    assertEquals("taken", e.code);
                    assertEquals("quit", e.detail);
                    break;
                }
                case "check_hotkey_length": {
                    StreamMenuModel.Refused e = refused(() -> StreamMenuModel.checkHotkey("quit",
                            new String[] {"CTRL", "ALT", "SHIFT", "Q", "W"}, new HashMap<>()));
                    assertEquals("hotkey_length", e.code);
                    break;
                }
                case "check_hotkey_lone_key":
                    assertArrayEquals(new String[] {"CTRL", "ALT", "SHIFT", "F2"},
                            StreamMenuModel.checkHotkey("cursor", new String[] {"F2"}, new HashMap<>()));
                    break;
                case "check_hotkey_typing":
                    assertEquals("hotkey_typing", refused(() -> StreamMenuModel.checkHotkey("cursor",
                            new String[] {"ALT", "Q"}, new HashMap<>())).code);
                    break;
                case "check_hotkey_shell":
                    assertEquals("reserved", refused(() -> StreamMenuModel.checkHotkey("cursor",
                            new String[] {"ALT", "TAB"}, new HashMap<>())).code);
                    break;
                case "check_reset": {
                    Map<String, String> bindings = new HashMap<>();
                    bindings.put("cursor", "CTRL+F2");
                    StreamMenuModel.checkReset("cursor", bindings);
                    break;
                }
                case "check_reset_taken": {
                    Map<String, String> bindings = new HashMap<>();
                    bindings.put("quit", "CTRL+ALT+SHIFT+TAB");
                    bindings.put("stream_menu", "off");
                    StreamMenuModel.Refused e = refused(() -> StreamMenuModel.checkReset("stream_menu", bindings));
                    assertEquals("taken", e.code);
                    assertEquals("quit", e.detail);
                    break;
                }
                default:
                    if (name.startsWith("check_hotkey_typing_")) {
                        // More typing chords, read from the case: AltGr+Shift, Shift with an arrow or punctuation, redo.
                        JSONObject request = fixture.getJSONObject("request");
                        assertEquals(name, "hotkey_typing", refused(() -> StreamMenuModel.checkHotkey(request.getString("id"),
                                RustStreamMenuModel.strings(request.getJSONArray("keys")), new HashMap<>())).code);
                        break;
                    }
                    throw new AssertionError("a contract case this test does not send: " + name);
            }
            run.add(name);
        }
        assertEquals(cases.length(), run.size());
    }

    /**
     * The JVM tests' {@link RustStreamMenuModel} refuses and accepts each contract chord as the Rust model does, so
     * its copy of {@code shortcuts::refused_chord} cannot drift from the policy unseen.
     */
    @Test
    public void theTestModelChecksEachContractChordAsRustDoes() throws Exception {
        JSONArray cases = cases();
        RustStreamMenuModel model = new RustStreamMenuModel();
        int checked = 0;
        for (int i = 0; i < cases.length(); i++) {
            JSONObject fixture = cases.getJSONObject(i);
            if (!fixture.getJSONObject("request").getString("op").equals("check_hotkey")) continue;
            String name = fixture.getString("name");
            JSONObject expected = fixture.getJSONObject("response");
            JSONObject answer = new JSONObject(model.call(fixture.getJSONObject("request").toString()));
            if (expected.has("ok")) {
                assertTrue(name + ": " + answer, same(expected.get("ok"), answer.opt("ok")));
            } else {
                assertEquals(name, expected.getString("error"), answer.optString("error", null));
                if (!expected.isNull("detail")) assertEquals(name, expected.getString("detail"), answer.optString("detail"));
            }
            checked++;
        }
        assertTrue(checked >= 10);
    }

    // JSON equality: the same keys and values, whatever the key order.
    private static boolean same(Object a, Object b) throws JSONException {
        if (a instanceof JSONObject && b instanceof JSONObject) {
            JSONObject x = (JSONObject) a, y = (JSONObject) b;
            if (x.length() != y.length()) return false;
            for (Iterator<String> keys = x.keys(); keys.hasNext(); ) {
                String key = keys.next();
                if (!y.has(key) || !same(x.get(key), y.get(key))) return false;
            }
            return true;
        }
        if (a instanceof JSONArray && b instanceof JSONArray) {
            JSONArray x = (JSONArray) a, y = (JSONArray) b;
            if (x.length() != y.length()) return false;
            for (int i = 0; i < x.length(); i++) if (!same(x.get(i), y.get(i))) return false;
            return true;
        }
        if (a instanceof Number && b instanceof Number) return ((Number) a).doubleValue() == ((Number) b).doubleValue();
        return a.equals(b);
    }
}
