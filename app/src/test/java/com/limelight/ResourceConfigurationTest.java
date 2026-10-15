package com.limelight;

import com.limelight.binding.video.UpscalingPolicy;

import org.junit.Test;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.HashSet;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import javax.xml.parsers.DocumentBuilderFactory;

import static org.junit.Assert.*;

public class ResourceConfigurationTest {
    private static final String ANDROID = "http://schemas.android.com/apk/res/android";
    private static final String APP = "http://schemas.android.com/apk/res-auto";

    private static Document read(String path) throws Exception {
        DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
        factory.setNamespaceAware(true);
        return factory.newDocumentBuilder().parse(new File("src/main", path));
    }

    @Test
    public void upscalingLabelsAndValuesMatchGlobalHostAndOverlayModeOrder() throws Exception {
        UpscalingPolicy.Mode[] modes = UpscalingPolicy.Mode.values();
        NodeList arrays = read("res/values/arrays.xml").getElementsByTagName("string-array");
        int checked = 0;
        for (int i = 0; i < arrays.getLength(); i++) {
            Element array = (Element) arrays.item(i);
            String name = array.getAttribute("name");
            if (!name.equals("upscaling_names") && !name.equals("upscaling_values")) continue;
            NodeList items = array.getElementsByTagName("item");
            assertEquals(modes.length, items.getLength());
            for (int j = 0; j < items.getLength(); j++) {
                assertEquals((name.equals("upscaling_names") ? "@string/upscaling_" : "") + modes[j].value,
                        items.item(j).getTextContent());
            }
            checked++;
        }
        assertEquals(2, checked);
    }

    @Test
    public void onlyShippedLanguagesAreOfferedByBothLanguagePickers() throws Exception {
        Set<String> platformLanguages = new HashSet<>();
        NodeList locales = read("res/xml/locales_config.xml").getElementsByTagName("locale");
        for (int i = 0; i < locales.getLength(); i++) {
            platformLanguages.add(((Element) locales.item(i)).getAttributeNS(ANDROID, "name"));
        }
        Set<String> legacyLanguages = new HashSet<>();
        NodeList arrays = read("res/values/arrays.xml").getElementsByTagName("string-array");
        int labelCount = 0;
        for (int i = 0; i < arrays.getLength(); i++) {
            Element array = (Element) arrays.item(i);
            NodeList items = array.getElementsByTagName("item");
            if (array.getAttribute("name").equals("language_names")) {
                labelCount = items.getLength();
            } else if (array.getAttribute("name").equals("language_values")) {
                for (int j = 0; j < items.getLength(); j++) {
                    legacyLanguages.add(items.item(j).getTextContent());
                }
            }
        }
        assertEquals(labelCount, legacyLanguages.size());
        assertTrue(legacyLanguages.remove("default"));
        assertEquals(platformLanguages, legacyLanguages);

        Set<String> translated = new HashSet<>();
        File[] dirs = new File("src/main/res").listFiles((dir, name) ->
                name.matches("values-[a-z]{2}(-r[A-Z]{2})?") && new File(dir, name + "/strings.xml").exists());
        assertNotNull(dirs);
        for (File dir : dirs) {
            translated.add(dir.getName().substring("values-".length()).replace("-r", "-"));
        }
        translated.add("en");
        assertEquals(translated, platformLanguages);

        Matcher filters = Pattern.compile("localeFilters \\+= \\[([^\\]]*)\\]")
                .matcher(new String(Files.readAllBytes(new File("build.gradle").toPath()), StandardCharsets.UTF_8));
        assertTrue(filters.find());
        Set<String> packaged = new HashSet<>();
        for (String filter : filters.group(1).split(",")) {
            packaged.add(filter.trim().replace("'", "").replace("-r", "-"));
        }
        assertEquals(platformLanguages, packaged);
    }

    @Test
    public void everyMenuIconIsTintedWithTheThemeOnSurfaceColour() throws Exception {
        File[] menus = new File("src/main/res/menu").listFiles((dir, name) -> name.endsWith(".xml"));
        assertNotNull(menus);
        assertTrue(menus.length > 0);
        int icons = 0;
        for (File menu : menus) {
            NodeList items = read("res/menu/" + menu.getName()).getElementsByTagName("item");
            for (int i = 0; i < items.getLength(); i++) {
                Element item = (Element) items.item(i);
                if (!item.hasAttributeNS(ANDROID, "icon")) continue;
                icons++;
                assertEquals(menu.getName() + " " + item.getAttributeNS(ANDROID, "id"),
                        "?attr/colorOnSurface", item.getAttributeNS(APP, "iconTint"));
            }
        }
        assertTrue(icons >= 3);
    }

    @Test
    public void toolbarsTintTheirNavigationIconWithTheThemeOnSurfaceColour() throws Exception {
        NodeList styles = read("res/values/styles.xml").getElementsByTagName("style");
        String toolbarStyle = null;
        for (int i = 0; i < styles.getLength(); i++) {
            Element style = (Element) styles.item(i);
            // AppTheme inherits everything from Base.Theme.Nightjar.
            if (!style.getAttribute("name").equals("Base.Theme.Nightjar")) continue;
            NodeList items = style.getElementsByTagName("item");
            for (int j = 0; j < items.getLength(); j++) {
                Element item = (Element) items.item(j);
                if (item.getAttribute("name").equals("toolbarStyle")) toolbarStyle = item.getTextContent();
            }
        }
        assertEquals("@style/Widget.Nightjar.Toolbar", toolbarStyle);
        boolean tinted = false;
        for (int i = 0; i < styles.getLength(); i++) {
            Element style = (Element) styles.item(i);
            if (!style.getAttribute("name").equals("Widget.Nightjar.Toolbar")) continue;
            NodeList items = style.getElementsByTagName("item");
            for (int j = 0; j < items.getLength(); j++) {
                Element item = (Element) items.item(j);
                tinted |= item.getAttribute("name").equals("navigationIconTint") &&
                        item.getTextContent().equals("?attr/colorOnSurface");
            }
        }
        assertTrue(tinted);
    }

    @Test
    public void layoutsUseTheTypeScaleInsteadOfFixedTextSizes() throws Exception {
        File[] layouts = new File("src/main/res/layout").listFiles((dir, name) -> name.endsWith(".xml"));
        assertNotNull(layouts);
        for (File layout : layouts) {
            NodeList nodes = read("res/layout/" + layout.getName()).getElementsByTagName("*");
            for (int i = 0; i < nodes.getLength(); i++) {
                Element node = (Element) nodes.item(i);
                assertFalse(layout.getName() + " " + node.getTagName() + " sets a fixed textSize; use a TextAppearance.Nightjar style",
                        node.hasAttributeNS(ANDROID, "textSize"));
            }
        }
    }

    @Test
    public void themesLeaveTextSizeAndFontToTheTextAppearances() throws Exception {
        // A theme-level android:textSize or android:fontFamily overrides every textAppearance, which made
        // screen titles render at 16sp, smaller than 18sp rows.
        NodeList styles = read("res/values/styles.xml").getElementsByTagName("style");
        for (int i = 0; i < styles.getLength(); i++) {
            Element style = (Element) styles.item(i);
            if (!style.getAttribute("name").contains("Theme")) continue;
            NodeList items = style.getElementsByTagName("item");
            for (int j = 0; j < items.getLength(); j++) {
                String name = ((Element) items.item(j)).getAttribute("name");
                assertNotEquals(style.getAttribute("name"), "android:textSize", name);
                assertNotEquals(style.getAttribute("name"), "android:fontFamily", name);
            }
        }
    }

    @Test
    public void installLocationIsDeclaredOnTheManifest() throws Exception {
        Document manifest = read("AndroidManifest.xml");
        assertEquals("auto", manifest.getDocumentElement().getAttributeNS(ANDROID, "installLocation"));
        assertFalse(((Element) manifest.getElementsByTagName("application").item(0))
                .hasAttributeNS(ANDROID, "installLocation"));
    }
}
