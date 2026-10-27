package com.limelight;

import org.junit.Test;
import org.w3c.dom.Element;
import org.w3c.dom.Node;
import org.w3c.dom.NodeList;

import java.io.File;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import javax.xml.parsers.DocumentBuilderFactory;

import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

/**
 * Checks every translated values-xx directory against the default strings. The rules are
 * documented in docs/TRANSLATIONS.md.
 */
public class TranslationResourcesTest {
    private static final Pattern PLACEHOLDER = Pattern.compile("%(\\d+\\$)?[-#+0,(]*\\d*(\\.\\d+)?[sdfxXc%]");
    private static final Pattern BARE_APOSTROPHE = Pattern.compile("(?<!\\\\)'");
    private static final Pattern BARE_QUOTE = Pattern.compile("(?<!\\\\)\"");

    /** Buttons and menu entries whose layouts leave little room; see docs/TRANSLATIONS.md. */
    static final Set<String> TIGHT_LABELS = Set.of(
            "addpc_button", "applist_menu_cancel", "crash_report_not_now",
            "keyboard_shortcuts_save", "keyboard_shortcuts_delete", "keyboard_shortcuts_replace_button",
            "host_profile_save", "host_menu_rename", "host_rename_save", "latency_benchmark_start", "latency_benchmark_share", "library_retry",
            "remote_next", "remote_previous", "remote_test", "report_problem_share",
            "help", "settings", "library_search", "library_options",
            "help_connect_link", "help_pairing_link", "help_controls_link", "help_profiles_link",
            "help_problems_link", "help_report_link");

    /** A translated tight label may be at most this many times as wide as the English one ... */
    static final int TIGHT_RATIO = 2;
    /** ... or this many columns wider, whichever allows more, so very short labels can still grow. */
    static final int TIGHT_SLACK = 10;

    private static final class Entry {
        final String type;
        final boolean formatted;
        final Map<String, String> values = new LinkedHashMap<>();

        Entry(String type, boolean formatted) {
            this.type = type;
            this.formatted = formatted;
        }
    }

    private static Map<String, Entry> load(File dir) throws Exception {
        Map<String, Entry> entries = new LinkedHashMap<>();
        File[] files = dir.listFiles((d, name) -> name.endsWith(".xml"));
        assertNotNull(files);
        DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
        for (File file : files) {
            NodeList nodes = factory.newDocumentBuilder().parse(file).getDocumentElement().getChildNodes();
            for (int i = 0; i < nodes.getLength(); i++) {
                if (nodes.item(i).getNodeType() != Node.ELEMENT_NODE) continue;
                Element element = (Element) nodes.item(i);
                String type = element.getTagName();
                if (!type.equals("string") && !type.equals("plurals") && !type.equals("string-array")) continue;
                if ("false".equals(element.getAttribute("translatable"))) continue;
                Entry entry = new Entry(type, !"false".equals(element.getAttribute("formatted")));
                if (type.equals("string")) {
                    entry.values.put("", element.getTextContent());
                } else {
                    NodeList items = element.getElementsByTagName("item");
                    for (int j = 0; j < items.getLength(); j++) {
                        Element item = (Element) items.item(j);
                        String key = type.equals("plurals") ? item.getAttribute("quantity") : String.valueOf(j);
                        entry.values.put(key, item.getTextContent());
                    }
                }
                entries.put(element.getAttribute("name"), entry);
            }
        }
        return entries;
    }

    private static File[] translatedDirs() {
        File[] dirs = new File("src/main/res").listFiles((dir, name) ->
                name.matches("values-[a-z]{2}(-r[A-Z]{2})?") && new File(dir, name + "/strings.xml").exists());
        assertNotNull(dirs);
        return dirs;
    }

    static List<String> placeholders(String text) {
        List<String> found = new ArrayList<>();
        Matcher m = PLACEHOLDER.matcher(text);
        while (m.find()) found.add(m.group());
        Collections.sort(found);
        return found;
    }

    /** Display columns: wide and full-width (CJK) characters take two. */
    static int width(String text) {
        int columns = 0;
        for (int i = 0; i < text.length(); ) {
            int cp = text.codePointAt(i);
            Character.UnicodeScript script = Character.UnicodeScript.of(cp);
            boolean wide = script == Character.UnicodeScript.HAN || script == Character.UnicodeScript.HIRAGANA
                    || script == Character.UnicodeScript.KATAKANA || script == Character.UnicodeScript.HANGUL
                    || (cp >= 0x3000 && cp <= 0x303F) || (cp >= 0xFF00 && cp <= 0xFF60);
            columns += wide ? 2 : 1;
            i += Character.charCount(cp);
        }
        return columns;
    }

    static boolean fitsTightLabel(String english, String translated) {
        int en = width(english);
        return width(translated) <= Math.max(TIGHT_RATIO * en, en + TIGHT_SLACK);
    }

    private static void checkEscaping(String where, String text, List<String> problems) {
        if (BARE_APOSTROPHE.matcher(text).find()) problems.add(where + ": unescaped apostrophe (use \\' or ’)");
        if (BARE_QUOTE.matcher(text).find()) problems.add(where + ": unescaped double quote (use \\\" or typographic quotes)");
    }

    @Test
    public void translationsKeepPlaceholdersEscapingAndTightLabelLengths() throws Exception {
        Map<String, Entry> defaults = load(new File("src/main/res/values"));
        List<String> problems = new ArrayList<>();
        for (Map.Entry<String, Entry> e : defaults.entrySet()) {
            for (String value : e.getValue().values.values()) checkEscaping("values/" + e.getKey(), value, problems);
        }
        File[] dirs = translatedDirs();
        assertTrue(dirs.length > 0);
        for (File dir : dirs) {
            for (Map.Entry<String, Entry> e : load(dir).entrySet()) {
                String where = dir.getName() + "/" + e.getKey();
                Entry source = defaults.get(e.getKey());
                if (source == null) {
                    problems.add(where + ": not in the default strings");
                    continue;
                }
                Entry translated = e.getValue();
                for (Map.Entry<String, String> item : translated.values.entrySet()) {
                    String value = item.getValue();
                    checkEscaping(where, value, problems);
                    if (!source.formatted) continue;
                    String english = source.type.equals("plurals") ? source.values.get("other") : source.values.get(item.getKey());
                    if (english == null) {
                        problems.add(where + ": item " + item.getKey() + " not in the default strings");
                        continue;
                    }
                    if (!placeholders(english).equals(placeholders(value))) {
                        problems.add(where + ": placeholders " + placeholders(value) + " differ from " + placeholders(english));
                    }
                    if (TIGHT_LABELS.contains(e.getKey()) && !fitsTightLabel(english, value)) {
                        problems.add(where + ": \"" + value + "\" is too long for the label \"" + english + "\"");
                    }
                }
            }
        }
        assertTrue(String.join("\n", problems), problems.isEmpty());
    }

    @Test
    public void tightLabelsExistInTheDefaultStrings() throws Exception {
        Map<String, Entry> defaults = load(new File("src/main/res/values"));
        for (String key : TIGHT_LABELS) {
            assertTrue(key, defaults.containsKey(key) && defaults.get(key).type.equals("string"));
        }
    }

    @Test
    public void checksCatchBrokenTranslations() {
        assertTrue(placeholders("%1$d of %2$d games").equals(placeholders("%2$d spośród %1$d")));
        assertTrue(!placeholders("%1$s failed").equals(placeholders("échec")));
        assertTrue(!placeholders("%d taps").equals(placeholders("%s taps")));
        assertTrue(BARE_APOSTROPHE.matcher("l'hôte").find());
        assertTrue(!BARE_APOSTROPHE.matcher("l\\'hôte").find());
        assertTrue(fitsTightLabel("Add", "Hinzufügen"));
        assertTrue(fitsTightLabel("Save", "保存"));
        assertTrue(!fitsTightLabel("Help", "Ayuda y documentación completa"));
        assertTrue(width("設定") == 4);
    }
}
