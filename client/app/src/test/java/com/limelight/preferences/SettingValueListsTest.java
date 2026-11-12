package com.limelight.preferences;

import com.limelight.utils.NetworkGrace;

import org.junit.Test;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import javax.xml.parsers.DocumentBuilderFactory;

import static org.junit.Assert.*;

/**
 * Value lists both clients offer come from nightjar-policy: Minimum resolution and Minimum frame rate
 * ({@code quality.rs}) and Wait through network dropouts ({@code recovery.rs}). Each platform keeps its own
 * default; the lists are held equal here.
 */
public class SettingValueListsTest {
    private static String policy(String file) throws Exception {
        for (String dir : new String[] {"../protocol/policy/src/", "protocol/policy/src/"}) {
            File source = new File(dir + file);
            if (source.isFile()) return new String(Files.readAllBytes(source.toPath()), StandardCharsets.UTF_8);
        }
        throw new AssertionError(file + " not found from " + new File(".").getAbsolutePath());
    }

    private static int[] rustArray(String source, String name) {
        Matcher m = Pattern.compile("const " + name + ":\\s*\\[u32;\\s*\\d+\\]\\s*=\\s*\\[([^\\]]*)\\]").matcher(source);
        assertTrue(name, m.find());
        String[] items = m.group(1).split(",");
        int[] values = new int[items.length];
        for (int i = 0; i < items.length; i++) values[i] = Integer.parseInt(items[i].trim());
        return values;
    }

    private static int[] xmlArray(String file, String name) throws Exception {
        Element root = DocumentBuilderFactory.newInstance().newDocumentBuilder()
                .parse(new File("src/main/res/values/" + file)).getDocumentElement();
        NodeList arrays = root.getElementsByTagName("string-array");
        for (int i = 0; i < arrays.getLength(); i++) {
            Element array = (Element) arrays.item(i);
            if (!name.equals(array.getAttribute("name"))) continue;
            NodeList items = array.getElementsByTagName("item");
            List<Integer> values = new ArrayList<>();
            for (int j = 0; j < items.getLength(); j++) values.add(Integer.parseInt(items.item(j).getTextContent().trim()));
            return values.stream().mapToInt(Integer::intValue).toArray();
        }
        throw new AssertionError(name + " not in " + file);
    }

    @Test
    public void qualityFloorsAreTheSharedLists() throws Exception {
        String quality = policy("quality.rs");
        int[] edges = rustArray(quality, "MIN_SHORT_EDGE_CHOICES");
        int[] fps = rustArray(quality, "MIN_FPS_CHOICES");
        assertArrayEquals(edges, PreferenceConfiguration.QUALITY_MIN_SHORT_EDGES);
        assertArrayEquals(edges, xmlArray("adaptive_quality.xml", "adaptive_quality_resolution_values"));
        assertArrayEquals(fps, PreferenceConfiguration.QUALITY_MIN_FPS);
        assertArrayEquals(fps, xmlArray("adaptive_quality.xml", "adaptive_quality_fps_values"));
        for (int edge : edges) assertEquals(edge, PreferenceConfiguration.listChoice(Integer.toString(edge), edges, 720));
        // What no list offers reads as the default, which stays Android's own.
        assertEquals(720, PreferenceConfiguration.listChoice("2160", edges, 720));
        assertEquals(30, PreferenceConfiguration.listChoice("15", fps, 30));
        assertEquals(30, PreferenceConfiguration.listChoice(null, fps, 30));
    }

    @Test
    public void networkGraceWaitsAreTheSharedList() throws Exception {
        int[] waits = rustArray(policy("recovery.rs"), "NETWORK_GRACE_CHOICES_S");
        assertArrayEquals(waits, xmlArray("network_grace.xml", "network_grace_values"));
        List<Integer> offered = new ArrayList<>();
        for (int seconds : waits) offered.add(seconds);
        // Every shared wait reads back as itself and nothing else does: the list is exactly the shared one.
        for (int seconds = 0; seconds <= 300; seconds++) {
            assertEquals(Integer.toString(seconds), offered.contains(seconds) ? seconds : 0,
                    NetworkGrace.parseSeconds(Integer.toString(seconds)));
        }
        // Off stays Android's default.
        assertEquals(0, NetworkGrace.parseSeconds(null));
    }
}
