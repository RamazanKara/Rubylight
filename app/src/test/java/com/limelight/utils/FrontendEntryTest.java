package com.limelight.utils;

import org.junit.Test;

import java.io.IOException;
import java.io.Reader;
import java.nio.charset.StandardCharsets;
import java.io.StringReader;
import java.util.Arrays;
import java.util.Map;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

public class FrontendEntryTest {
    @Test
    public void entriesWithTheSameHostAndNameMustStillMatchTheApp() throws IOException {
        Map<String, String> entry = parse(FrontendEntry.serialize("host", "Desk", "app-a", "Game", 42));
        assertFalse(FrontendEntry.matchesApp(entry, "host", "app-b", 42));
        assertTrue(FrontendEntry.matchesApp(entry, "HOST", "APP-A", 99));
        assertFalse(FrontendEntry.matchesApp(entry, "other-host", "app-a", 42));
    }

    @Test
    public void legacyEntriesMatchOnlyTheSameNumericAppId() throws IOException {
        Map<String, String> entry = parse(FrontendEntry.serialize("host", "Desk", null, "Game", 42));
        assertFalse(FrontendEntry.matchesApp(entry, "host", "", 99));
        assertTrue(FrontendEntry.matchesApp(entry, "host", "new-uuid", 42));
        entry.remove(FrontendEntry.KEY_APP_ID);
        assertFalse(FrontendEntry.matchesApp(entry, "host", "", 0));
    }

    @Test
    public void toleratesBomBeforeExportCommentOrBlankLine() throws IOException {
        for (String prefix : new String[] {"\uFEFF# Nightjar game entry\r\n", "\uFEFF\r\n"}) {
            assertEquals("Desk", parse(prefix + "[host_name] Desk\n").get(FrontendEntry.KEY_HOST_NAME));
        }
    }

    private static Map<String, String> parse(String text) throws IOException {
        return FrontendEntry.parse(new StringReader(text));
    }

    @Test
    public void roundTripsAnEntry() throws IOException {
        String text = FrontendEntry.serialize("host-uuid", "Gaming PC", "app-uuid", "Elden Ring", 42);
        Map<String, String> values = parse(text);
        assertEquals("host-uuid", values.get(FrontendEntry.KEY_HOST_UUID));
        assertEquals("Gaming PC", values.get(FrontendEntry.KEY_HOST_NAME));
        assertEquals("app-uuid", values.get(FrontendEntry.KEY_APP_UUID));
        assertEquals("Elden Ring", values.get(FrontendEntry.KEY_APP_NAME));
        assertEquals("42", values.get(FrontendEntry.KEY_APP_ID));
    }

    @Test
    public void readsArtemisExports() throws IOException {
        // As written by ApolloLauncherExport's generic generator
        Map<String, String> values = parse("# Artemis app entry\n" +
                "[host_uuid] 1234\n[host_name] Desk\n[app_uuid] ABCD\n[app_name] Steam Big Picture\n");
        assertEquals("1234", values.get(FrontendEntry.KEY_HOST_UUID));
        assertEquals("Steam Big Picture", values.get(FrontendEntry.KEY_APP_NAME));
        assertFalse(values.containsKey(FrontendEntry.KEY_APP_ID));
    }

    @Test
    public void toleratesBomCrlfAndBlankValues() throws IOException {
        Map<String, String> values = parse("\uFEFF[host_name] Desk\r\n[app_uuid]\r\n\r\n[APP_NAME] Hades\r\n");
        assertEquals("Desk", values.get(FrontendEntry.KEY_HOST_NAME));
        assertEquals("Hades", values.get(FrontendEntry.KEY_APP_NAME));
        assertFalse(values.containsKey(FrontendEntry.KEY_APP_UUID));
    }

    @Test
    public void rejectsLongLinesBeforeReadingUnboundedInput() {
        Reader source = new Reader() {
            int read;

            @Override
            public int read(char[] buffer, int offset, int length) {
                assertTrue("Read past the entry limit", read <= FrontendEntry.MAX_CHARS);
                int count = Math.min(length, FrontendEntry.MAX_CHARS + 1 - read);
                java.util.Arrays.fill(buffer, offset, offset + count, '#');
                read += count;
                return count;
            }

            @Override public void close() { }
        };
        try {
            FrontendEntry.parse(source);
            fail("Accepted unbounded line");
        } catch (IOException expected) {
            assertEquals("Entry file is too large", expected.getMessage());
        }
    }

    @Test
    public void acceptsAnEntryExactlyAtTheCharacterLimit() throws IOException {
        String text = "[host_name] Desk\n#";
        text += "x".repeat(FrontendEntry.MAX_CHARS - text.length());
        assertEquals("Desk", parse(text).get(FrontendEntry.KEY_HOST_NAME));
    }

    @Test
    public void unicodeFileNamesFitOnDiskAndDoNotSplitSurrogatePairs() {
        for (String name : new String[] {"界".repeat(120), "x".repeat(119) + "🎮"}) {
            String base = FrontendEntry.fileBaseName(name);
            assertTrue((base + ".art").getBytes(StandardCharsets.UTF_8).length <= 255);
            assertFalse(Character.isHighSurrogate(base.charAt(base.length() - 1)));
        }
    }

    @Test
    public void mergingCompactXmlPreservesRootsAndNeighboringSystems() {
        String before = "<system><name>n64</name></system>";
        String after = "<system><name>ps2</name></system>";
        String existing = "<systemList>" + before +
                "<system><name>butterpollo</name><fullname>Old</fullname></system>" + after + "</systemList>";
        String merged = FrontendEntry.mergeEsSystems(existing, "New");
        assertTrue(merged.contains("<systemList>"));
        assertTrue(merged.contains(before));
        assertTrue(merged.contains(after));
        assertTrue(merged.contains("</systemList>"));
        assertFalse(merged.contains("Old"));
    }

    @Test
    public void mergingCompactXmlPreservesOtherFindRules() {
        String other = "<emulator name=\"OTHER\"><rule/></emulator>";
        String merged = FrontendEntry.mergeEsFindRules("<ruleList>" + other +
                "<emulator name=\"BUTTERPOLLO\"><rule/></emulator></ruleList>", "com.butterpollo.client");
        assertTrue(merged.contains("<ruleList>"));
        assertTrue(merged.contains(other));
        assertTrue(merged.contains("</ruleList>"));
    }

    @Test
    public void malformedExistingXmlIsNotSilentlyReplaced() {
        for (String text : new String[] {"<systemList><system>", "<other/>",
                "<systemList><system></systemList>",
                "<!DOCTYPE systemList [<!ENTITY name 'secret'>]><systemList/>"}) {
            org.junit.Assert.assertThrows(IllegalArgumentException.class,
                    () -> FrontendEntry.mergeEsSystems(text, "Nightjar"));
        }
    }

    @Test
    public void mergesXmlByElementIdentityRatherThanCommentsOrWhitespace() {
        String comment = "<!-- <system><name>butterpollo</name></system> -->";
        String merged = FrontendEntry.mergeEsSystems("<systemList>" + comment +
                "<system><name> butterpollo </name><fullname>Old</fullname></system></systemList>", "New");
        assertTrue(merged.contains(comment));
        assertFalse(merged.contains("Old"));
        String rules = FrontendEntry.mergeEsFindRules(
                "<ruleList><emulator name = 'BUTTERPOLLO'><rule>Old</rule></emulator></ruleList>", "client");
        assertFalse(rules.contains("Old"));
        assertEquals(1, count(rules, "<emulator"));
    }

    @Test
    public void rejectsFilesThatAreNotEntries() {
        String[] bad = {
                "PK\u0003\u0004 binary",
                "[app_name] No host here\n",
                "[] empty key\n",
                "",
        };
        for (String text : bad) {
            try {
                parse(text);
                fail("accepted: " + text);
            } catch (IOException expected) {
                // expected
            }
        }
    }

    @Test
    public void rejectsOversizedFiles() {
        StringBuilder sb = new StringBuilder("[host_name] Desk\n");
        while (sb.length() <= FrontendEntry.MAX_CHARS) {
            sb.append("# padding padding padding padding padding padding\n");
        }
        try {
            parse(sb.toString());
            fail("accepted oversized entry");
        } catch (IOException expected) {
            // expected
        }
    }

    @Test
    public void rejectsAnOversizedLineBeforeReadingTheRestOfIt() {
        Reader source = new Reader() {
            private int read;

            @Override
            public int read(char[] buffer, int offset, int length) {
                assertTrue("Read beyond the entry size limit", read + length <= FrontendEntry.MAX_CHARS + 1);
                Arrays.fill(buffer, offset, offset + length, '#');
                read += length;
                return length;
            }

            @Override
            public void close() { }
        };
        assertThrows(IOException.class, () -> FrontendEntry.parse(source));
    }

    @Test
    public void acceptsABomBeforeAnExportedCommentOrBlankLine() throws IOException {
        assertEquals("Desk", parse("\uFEFF" + FrontendEntry.serialize(null, "Desk", null, "Game", 42))
                .get(FrontendEntry.KEY_HOST_NAME));
        assertEquals("Desk", parse("\uFEFF\r\n[host_name] Desk\r\n")
                .get(FrontendEntry.KEY_HOST_NAME));
    }

    @Test
    public void acceptsAnEntryAtTheCharacterLimitWithoutAFinalNewline() throws IOException {
        String entry = "[host_name] Desk";
        assertEquals("Desk", parse(entry + " ".repeat(FrontendEntry.MAX_CHARS - entry.length()))
                .get(FrontendEntry.KEY_HOST_NAME));
    }

    @Test
    public void onlyEntriesTheExportWroteCountAsItsOwn() throws IOException {
        String marked = FrontendEntry.serializeExport("host", "PC", "app", "Game", 42);
        assertTrue(FrontendEntry.writtenByExport(marked, parse(marked)));
        // Exports before the marker: exactly this text, always with a game number.
        String legacy = FrontendEntry.serialize("host", "PC", "app", "Game", 42);
        assertTrue(FrontendEntry.writtenByExport(legacy, parse(legacy)));

        for (String user : new String[] {
                FrontendEntry.serialize("host", "PC", null, null, 0),
                "[host_uuid] host\n[app_name] Game\n",
                legacy + "# edited\n",
                legacy.replace("[app_name] Game", "[app_name]  Game"),
                legacy.replace("\n", "\r\n"),
                legacy + "[nightjar_export] yes\n"}) {
            assertFalse(user, FrontendEntry.writtenByExport(user, parse(user)));
        }
    }

    @Test
    public void serializeDropsLineBreaksAndMissingFields() throws IOException {
        String text = FrontendEntry.serialize("u", "Two\nLines", null, "Game", 0);
        assertFalse(text.contains("[app_uuid]"));
        assertFalse(text.contains("[app_id]"));
        assertEquals("Two Lines", parse(text).get(FrontendEntry.KEY_HOST_NAME));
    }

    @Test
    public void fileNamesAreSafeForSdCards() {
        assertEquals("Half-Life 2 Episode One", FrontendEntry.fileBaseName("Half-Life 2: Episode One"));
        assertEquals("a b", FrontendEntry.fileBaseName("a/\\b"));
        assertEquals("Game", FrontendEntry.fileBaseName("..."));
        assertEquals("Game", FrontendEntry.fileBaseName(null));
        assertEquals("Desktop", FrontendEntry.fileBaseName(".Desktop. "));
        assertTrue(FrontendEntry.fileBaseName(new String(new char[300]).replace('\0', 'x')).length() <= 120);
    }

    @Test
    public void createsEsDeFilesWhenMissing() {
        String systems = FrontendEntry.mergeEsSystems(null, "Nightjar & Co");
        assertTrue(systems.startsWith("<?xml"));
        assertTrue(systems.contains("<name>butterpollo</name>"));
        assertTrue(systems.contains("<fullname>Nightjar &amp; Co</fullname>"));
        assertTrue(systems.contains("%EMULATOR_BUTTERPOLLO%"));
        assertTrue(systems.contains("<command label=\"Nightjar\">"));
        assertTrue(systems.contains("%DATA%=%ROMPROVIDER%"));
        assertTrue(systems.trim().endsWith("</systemList>"));

        String rules = FrontendEntry.mergeEsFindRules("", "com.butterpollo.client");
        assertTrue(rules.contains("<entry>com.butterpollo.client/com.limelight.ShortcutTrampoline</entry>"));
        assertTrue(rules.contains("<!-- Nightjar game streaming client -->"));
        assertTrue(rules.trim().endsWith("</ruleList>"));
    }

    @Test
    public void keepsOtherCustomSystemsAndReplacesOurs() {
        String other = "    <system>\n        <name>n64</name>\n        <path>/sdcard/n64</path>\n    </system>\n";
        String existing = "<?xml version=\"1.0\"?>\n<systemList>\n" + other + "</systemList>\n";

        String once = FrontendEntry.mergeEsSystems(existing, "Old name");
        assertTrue(once.contains(other));
        assertTrue(once.contains("Old name"));

        String twice = FrontendEntry.mergeEsSystems(once, "New name");
        assertTrue(twice.contains(other));
        assertFalse(twice.contains("Old name"));
        assertEquals(1, count(twice, "<name>butterpollo</name>"));
        assertEquals(1, count(twice, "</systemList>"));
    }

    @Test
    public void keepsOtherFindRulesAndReplacesOurs() {
        String existing = "<ruleList>\n    <emulator name=\"SPECCY\">\n        <rule type=\"androidpackage\">\n" +
                "            <entry>a/b</entry>\n        </rule>\n    </emulator>\n</ruleList>";
        String once = FrontendEntry.mergeEsFindRules(existing, "com.butterpollo.client");
        String twice = FrontendEntry.mergeEsFindRules(once, "com.butterpollo.client.root");
        assertTrue(twice.contains("<emulator name=\"SPECCY\">"));
        assertTrue(twice.contains("com.butterpollo.client.root/"));
        assertEquals(1, count(twice, "name=\"BUTTERPOLLO\""));
        assertEquals(2, count(twice, "</emulator>"));
    }

    private static int count(String haystack, String needle) {
        int n = 0;
        for (int i = haystack.indexOf(needle); i >= 0; i = haystack.indexOf(needle, i + 1)) {
            n++;
        }
        return n;
    }
}
