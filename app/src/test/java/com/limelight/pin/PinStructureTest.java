package com.limelight.pin;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import org.junit.Test;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.Node;
import org.w3c.dom.NodeList;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import java.util.stream.Collectors;
import java.util.stream.Stream;

import javax.xml.parsers.DocumentBuilderFactory;

/** What the PIN lock leans on that no behavioural test would notice breaking: the manifest, backups, the one door to Game. */
public class PinStructureTest {
    private static final Path MAIN = Paths.get("src/main");
    private static final String ANDROID = "http://schemas.android.com/apk/res/android";

    private static Document parse(Path file) throws Exception {
        DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
        factory.setNamespaceAware(true);
        return factory.newDocumentBuilder().parse(file.toFile());
    }

    private static Element activity(Document manifest, String name) {
        NodeList activities = manifest.getElementsByTagName("activity");
        for (int i = 0; i < activities.getLength(); i++) {
            Element element = (Element) activities.item(i);
            if (name.equals(element.getAttributeNS(ANDROID, "name"))) return element;
        }
        return null;
    }

    private static boolean hasIntentFilter(Element activity) {
        for (Node child = activity.getFirstChild(); child != null; child = child.getNextSibling()) {
            if (child instanceof Element && "intent-filter".equals(((Element) child).getTagName())) return true;
        }
        return false;
    }

    @Test
    public void gameCannotBeStartedByOtherApps() throws Exception {
        Document manifest = parse(MAIN.resolve("AndroidManifest.xml"));
        Element game = activity(manifest, ".Game");
        assertNotNull("Game is declared", game);
        assertFalse("Game must not be exported", "true".equals(game.getAttributeNS(ANDROID, "exported")));
        assertFalse("an intent filter would export Game", hasIntentFilter(game));
    }

    @Test
    public void thePinScreensAreNotExported() throws Exception {
        Document manifest = parse(MAIN.resolve("AndroidManifest.xml"));
        for (String name : new String[] {".pin.PinLockActivity", ".pin.PinLockSettingsActivity"}) {
            Element element = activity(manifest, name);
            assertNotNull(name + " is declared", element);
            assertEquals(name, "false", element.getAttributeNS(ANDROID, "exported"));
            assertFalse(name, hasIntentFilter(element));
        }
    }

    @Test
    public void onlyServerHelperNamesGameAsAStartTarget() throws IOException {
        Path root = MAIN.resolve("java");
        List<String> users = new ArrayList<>();
        try (Stream<Path> files = Files.walk(root)) {
            for (Path file : files.filter(f -> f.toString().endsWith(".java")).collect(Collectors.toList())) {
                String text = new String(Files.readAllBytes(file), StandardCharsets.UTF_8);
                if (text.contains("Game.class") || text.contains("\"com.limelight.Game\"")) {
                    users.add(root.relativize(file).toString().replace(File.separatorChar, '/'));
                }
            }
        }
        // Every stream start passes ServerHelper.doStart, where the PIN lock sits; a second door would skip it.
        assertEquals(java.util.Collections.singletonList("com/limelight/utils/ServerHelper.java"), users);
    }

    @Test
    public void preferencesAreExcludedFromEveryBackupSoThePinHashStaysOnTheDevice() throws Exception {
        for (String file : new String[] {"backup_rules.xml", "backup_rules_s.xml"}) {
            Document rules = parse(MAIN.resolve("res/xml").resolve(file));
            NodeList excludes = rules.getElementsByTagName("exclude");
            NodeList includes = rules.getElementsByTagName("include");
            assertEquals(file + " includes nothing", 0, includes.getLength());
            // Every section that backs data up must drop shared preferences.
            NodeList sections = rules.getElementsByTagName("*");
            int sectionsChecked = 0;
            for (int i = 0; i < sections.getLength(); i++) {
                Element section = (Element) sections.item(i);
                String tag = section.getTagName();
                if (!tag.equals("full-backup-content") && !tag.equals("cloud-backup") && !tag.equals("device-transfer")) {
                    continue;
                }
                sectionsChecked++;
                boolean dropsPrefs = false;
                for (Node child = section.getFirstChild(); child != null; child = child.getNextSibling()) {
                    if (child instanceof Element && "exclude".equals(((Element) child).getTagName())) {
                        Element exclude = (Element) child;
                        if ("sharedpref".equals(exclude.getAttribute("domain")) && ".".equals(exclude.getAttribute("path"))) {
                            dropsPrefs = true;
                        }
                    }
                }
                assertTrue(file + " <" + tag + "> must exclude all shared preferences", dropsPrefs);
            }
            assertTrue(file, sectionsChecked > 0 && excludes.getLength() >= sectionsChecked);
        }
        assertEquals("pin_lock", PinStore.FILE);
    }

    @Test
    public void thePinPackageNeverLogsAnything() throws IOException {
        try (Stream<Path> files = Files.list(MAIN.resolve("java/com/limelight/pin"))) {
            for (Path file : files.filter(f -> f.toString().endsWith(".java")).collect(Collectors.toList())) {
                String text = new String(Files.readAllBytes(file), StandardCharsets.UTF_8);
                assertFalse(file + " logs", text.contains("android.util.Log") || text.contains("Log.d(")
                        || text.contains("Log.i(") || text.contains("Log.w(") || text.contains("Log.e(")
                        || text.contains("System.out") || text.contains("printStackTrace"));
            }
        }
    }
}
