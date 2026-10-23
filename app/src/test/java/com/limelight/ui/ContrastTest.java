package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.util.TypedValue;

import androidx.appcompat.view.ContextThemeWrapper;
import androidx.core.graphics.ColorUtils;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

import java.io.File;
import java.util.HashMap;
import java.util.Map;

import javax.xml.parsers.DocumentBuilderFactory;

/** High contrast holds body text to 7:1 and focus rings and outlines to 3:1, from the palette and from the themes. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@org.robolectric.annotation.GraphicsMode(org.robolectric.annotation.GraphicsMode.Mode.NATIVE)
@Config(sdk = 30, application = Application.class)
public class ContrastTest {
    static final double BODY = 7.0;
    static final double RING = 3.0;

    // The palette, read from the XML so the numbers in palette.xml's comment cannot drift from the colours.
    private static Map<String, String> colors() throws Exception {
        Map<String, String> colors = new HashMap<>();
        for (String file : new String[] {"palette.xml", "brand_colors.xml"}) {
            Document document = DocumentBuilderFactory.newInstance().newDocumentBuilder()
                    .parse(new File("src/main/res/values/" + file));
            NodeList list = document.getElementsByTagName("color");
            for (int i = 0; i < list.getLength(); i++) {
                Element color = (Element) list.item(i);
                colors.put(color.getAttribute("name"), color.getTextContent().trim());
            }
        }
        return colors;
    }

    private static int resolve(Map<String, String> colors, String name) {
        String value = colors.get(name);
        if (value == null) throw new AssertionError("No colour " + name);
        return value.startsWith("@color/") ? resolve(colors, value.substring(7)) : 0xFF000000 | (int) Long.parseLong(value.substring(1), 16);
    }

    private static void assertContrast(String what, double minimum, int foreground, int background) {
        double ratio = ColorUtils.calculateContrast(foreground, background);
        assertTrue(String.format("%s: %.2f:1 is under %.1f:1", what, ratio, minimum), ratio >= minimum);
    }

    @Test
    public void theNightPaletteOnBlackClearsHighContrast() throws Exception {
        Map<String, String> colors = colors();
        int black = 0xFF000000;
        assertContrast("on-surface on black", BODY, resolve(colors, "night_on_surface"), black);
        assertContrast("moon amber focus ring on black", RING, resolve(colors, "night_primary"), black);
        assertContrast("dark text on amber", BODY, resolve(colors, "night_on_primary"), resolve(colors, "night_primary"));
    }

    @Test
    public void thePalettesBodyTextClearsSevenToOneOnItsOwnSurfaces() throws Exception {
        Map<String, String> colors = colors();
        for (String set : new String[] {"night", "day"}) {
            int onSurface = resolve(colors, set + "_on_surface");
            for (String surface : new String[] {"surface", "surface_container_low", "surface_container", "surface_container_high"}) {
                assertContrast(set + " on-surface on " + surface, BODY, onSurface, resolve(colors, set + "_" + surface));
            }
            assertContrast(set + " focus ring on the surface", RING, resolve(colors, set + "_primary"), resolve(colors, set + "_surface"));
        }
    }

    private static int color(Context theme, int attr) {
        TypedValue value = new TypedValue();
        assertTrue(theme.getTheme().resolveAttribute(attr, value, true));
        return value.data;
    }

    private static Context highContrast(int base, boolean reducedMotion) {
        ContextThemeWrapper theme = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), base);
        theme.getTheme().applyStyle(R.style.ThemeOverlay_Nightjar_HighContrast, true);
        if (reducedMotion) theme.getTheme().applyStyle(R.style.ThemeOverlay_Nightjar_HighContrast_ReducedMotion, true);
        return theme;
    }

    private static void assertHighContrast(Context theme) {
        int[] surfaces = {
                com.google.android.material.R.attr.colorSurface,
                com.google.android.material.R.attr.colorSurfaceContainerLow,
                com.google.android.material.R.attr.colorSurfaceContainer,
                com.google.android.material.R.attr.colorSurfaceContainerHigh};
        int onSurface = color(theme, com.google.android.material.R.attr.colorOnSurface);
        int variant = color(theme, com.google.android.material.R.attr.colorOnSurfaceVariant);
        int outline = color(theme, com.google.android.material.R.attr.colorOutline);
        int outlineVariant = color(theme, com.google.android.material.R.attr.colorOutlineVariant);
        int primary = color(theme, androidx.appcompat.R.attr.colorPrimary);
        assertEquals("secondary text is body text", onSurface, variant);
        for (int surface : surfaces) {
            int background = color(theme, surface);
            assertContrast("body text", BODY, onSurface, background);
            assertContrast("secondary text", BODY, variant, background);
            assertContrast("outline", BODY, outline, background);
            assertContrast("card outline", BODY, outlineVariant, background);
            assertContrast("focus ring", RING, primary, background);
        }
        assertContrast("text on the amber button", BODY, color(theme, com.google.android.material.R.attr.colorOnPrimary), primary);
    }

    @Test
    @Config(qualifiers = "notnight")
    public void highContrastOnTheBlackThemeInSystemLightMode() {
        assertHighContrast(highContrast(R.style.OledBlackTheme, false));
        assertHighContrast(highContrast(R.style.OledBlackTheme, true));
    }

    @Test
    @Config(qualifiers = "night")
    public void highContrastOnTheBlackThemeInSystemDarkMode() {
        assertHighContrast(highContrast(R.style.OledBlackTheme, false));
    }

    @Test
    @Config(qualifiers = "night")
    public void theOverlayAloneLiftsTheAppThemesDimTextRoles() {
        Context theme = highContrast(R.style.AppTheme, false);
        assertEquals(color(theme, com.google.android.material.R.attr.colorOnSurface),
                color(theme, com.google.android.material.R.attr.colorOnSurfaceVariant));
    }

    @Test
    public void highContrastThickensEveryRing() {
        Context normal = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
        Context thick = highContrast(R.style.OledBlackTheme, false);
        for (int attr : new int[] {R.attr.nightjarFocusRingWidth, R.attr.nightjarCardFocusRingWidth}) {
            float before = dimension(normal, attr);
            float after = dimension(thick, attr);
            assertTrue(after + " dp after " + before + " dp", after > before);
        }
        assertEquals("the moon ring is 4 dp in high contrast", 4f, dimension(thick, R.attr.nightjarFocusRingWidth), 0.01f);
        assertEquals(4f, dimension(thick, R.attr.nightjarCardFocusRingWidth), 0.01f);
        assertEquals("cards get a 2 dp outline", 2f, dimension(thick, R.attr.nightjarCardStrokeWidth), 0.01f);
    }

    private static float dimension(Context theme, int attr) {
        TypedValue value = new TypedValue();
        assertTrue(theme.getTheme().resolveAttribute(attr, value, true));
        return value.getDimension(theme.getResources().getDisplayMetrics()) / theme.getResources().getDisplayMetrics().density;
    }

    private static com.google.android.material.button.MaterialButton tonalButton(Context theme) {
        return new com.google.android.material.button.MaterialButton(theme, null, R.attr.nightjarButtonTonalStyle);
    }

    private static int restStroke(com.google.android.material.button.MaterialButton button) {
        return button.getStrokeColor().getColorForState(new int[0], 0);
    }

    @Test
    @Config(qualifiers = "night")
    public void aTonalButtonHasAnOutlineAtRestOnlyInHighContrast() {
        Context normal = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.OledBlackTheme);
        assertEquals("no outline at rest normally", 0, android.graphics.Color.alpha(restStroke(tonalButton(normal))));

        Context high = highContrast(R.style.OledBlackTheme, false);
        com.google.android.material.button.MaterialButton button = tonalButton(high);
        assertContrast("tonal button outline at rest", BODY, restStroke(button), 0xFF000000);
        assertEquals("focused: the amber ring", color(high, androidx.appcompat.R.attr.colorPrimary),
                button.getStrokeColor().getColorForState(new int[] {android.R.attr.state_focused}, 0));
    }
}
