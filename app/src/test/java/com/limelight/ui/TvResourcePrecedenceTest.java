package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

import java.io.File;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import javax.xml.parsers.DocumentBuilderFactory;

/**
 * Width and orientation outrank {@code television} in resource matching, and a TV is landscape and past 840dp. So a
 * television resource with a twin in {@code -w840dp}, {@code -w600dp} or {@code -land} never applies on a TV unless that twin
 * also has a television variant (Settings' rail rows did: they looked like a handheld's on every TV).
 */
public class TvResourcePrecedenceTest {
    private static final Pattern LOCALE = Pattern.compile("^(?:[a-z]{2}|r[A-Z]{2})$");
    private static final Pattern WIDTH = Pattern.compile("^w(\\d+)dp$");

    private static final class Variant {
        final List<String> qualifiers;
        final String value;

        Variant(List<String> qualifiers, String value) {
            this.qualifiers = qualifiers;
            this.value = value;
        }

        boolean television() {
            return qualifiers.contains("television");
        }

        int width() {
            for (String qualifier : qualifiers) {
                Matcher width = WIDTH.matcher(qualifier);
                if (width.matches()) return Integer.parseInt(width.group(1));
            }
            return 0;
        }

        /** Whether this variant applies to a dark landscape TV of that width. */
        boolean applies(int tvWidth) {
            for (String qualifier : qualifiers) {
                if (qualifier.equals("port") || qualifier.equals("notnight") || qualifier.equals("watch")) return false;
                if (qualifier.startsWith("sw")) return false;
            }
            return width() <= tvWidth;
        }

        /** Android's order: width, then orientation, then UI mode. */
        long rank() {
            return width() * 100L + (qualifiers.contains("land") ? 10 : 0) + (television() ? 1 : 0);
        }
    }

    private static File res() {
        for (String path : new String[] {"src/main/res", "app/src/main/res", "client/app/src/main/res"}) {
            File candidate = new File(path);
            if (candidate.isDirectory()) return candidate;
        }
        throw new AssertionError("no res folder from " + new File("").getAbsolutePath());
    }

    private static Map<String, List<Variant>> variants() throws Exception {
        Map<String, List<Variant>> found = new HashMap<>();
        for (File folder : res().listFiles()) {
            String[] parts = folder.getName().split("-");
            List<String> qualifiers = new ArrayList<>(Arrays.asList(parts).subList(1, parts.length));
            if (qualifiers.stream().anyMatch(q -> LOCALE.matcher(q).matches())) continue;
            for (File file : folder.listFiles()) {
                if (!parts[0].equals("values")) {
                    found.computeIfAbsent(parts[0] + "/" + file.getName().replaceFirst("\\..*", ""), k -> new ArrayList<>())
                            .add(new Variant(qualifiers, file.getPath()));
                    continue;
                }
                NodeList items = DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(file).getDocumentElement().getChildNodes();
                for (int i = 0; i < items.getLength(); i++) {
                    if (!(items.item(i) instanceof Element)) continue;
                    Element item = (Element) items.item(i);
                    if (!item.hasAttribute("name")) continue;
                    String type = item.hasAttribute("type") ? item.getAttribute("type") : item.getTagName();
                    found.computeIfAbsent(type + "/" + item.getAttribute("name"), k -> new ArrayList<>())
                            .add(new Variant(qualifiers, item.getTextContent().trim()));
                }
            }
        }
        return found;
    }

    @Test
    public void everyTelevisionResourceReachesATv() throws Exception {
        Map<String, List<Variant>> all = variants();
        assertTrue("the scan found the app's resources", all.containsKey("layout/activity_console_home"));
        List<String> lost = new ArrayList<>();
        for (int tvWidth : new int[] {960, 1280}) {
            for (Map.Entry<String, List<Variant>> entry : all.entrySet()) {
                Variant television = null;
                Variant winner = null;
                for (Variant variant : entry.getValue()) {
                    if (variant.television()) television = variant;
                    if (variant.applies(tvWidth) && (winner == null || variant.rank() > winner.rank())) winner = variant;
                }
                if (television == null || winner == null || winner.television()) continue;
                // A twin that says the same thing costs nothing.
                if (!winner.value.isEmpty() && winner.value.equals(television.value)) continue;
                lost.add(entry.getKey() + " at " + tvWidth + "dp: " + winner.qualifiers + " beats " + television.qualifiers);
            }
        }
        assertEquals("television resources a TV never gets: " + lost, 0, lost.size());
    }
}
