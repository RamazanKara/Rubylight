package com.limelight.ui;

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
import java.util.Map;
import java.util.TreeMap;

import javax.xml.parsers.DocumentBuilderFactory;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@org.robolectric.annotation.GraphicsMode(org.robolectric.annotation.GraphicsMode.Mode.NATIVE)
@Config(sdk = 30, application = Application.class)
public class ThemePaletteTest {
    private static final double TEXT = 4.5;
    private static final double NON_TEXT = 3.0;

    private static int color(Context context, int attr) {
        TypedValue value = new TypedValue();
        assertTrue(context.getTheme().resolveAttribute(attr, value, true));
        return value.data;
    }

    private static Context themed(int theme) {
        return new ContextThemeWrapper(RuntimeEnvironment.getApplication(), theme);
    }

    private static void assertReadable(Context theme) {
        int primary = color(theme, androidx.appcompat.R.attr.colorPrimary);
        int surface = color(theme, com.google.android.material.R.attr.colorSurface);
        int onSurface = color(theme, com.google.android.material.R.attr.colorOnSurface);
        int container = color(theme, com.google.android.material.R.attr.colorSurfaceContainerHigh);
        assertTrue("text on amber", ColorUtils.calculateContrast(
                color(theme, com.google.android.material.R.attr.colorOnPrimary), primary) >= TEXT);
        assertTrue("amber text and icons on the surface", ColorUtils.calculateContrast(primary, surface) >= TEXT);
        assertTrue("amber text in dialogs", ColorUtils.calculateContrast(primary, container) >= TEXT);
        assertTrue("body text", ColorUtils.calculateContrast(onSurface, surface) >= TEXT);
        assertTrue("secondary text", ColorUtils.calculateContrast(
                color(theme, com.google.android.material.R.attr.colorOnSurfaceVariant), container) >= TEXT);
        assertTrue("outlines", ColorUtils.calculateContrast(
                color(theme, com.google.android.material.R.attr.colorOutline), surface) >= NON_TEXT);
        assertTrue("tonal buttons", ColorUtils.calculateContrast(
                color(theme, com.google.android.material.R.attr.colorOnSecondaryContainer),
                color(theme, com.google.android.material.R.attr.colorSecondaryContainer)) >= TEXT);
        assertTrue("errors", ColorUtils.calculateContrast(
                color(theme, androidx.appcompat.R.attr.colorError), surface) >= TEXT);
        // Real preference rows and state selectors must use this context's palette too.
        android.view.View row = android.view.LayoutInflater.from(theme).inflate(R.layout.settings_preference, null);
        assertEquals(onSurface, ((android.widget.TextView) row.findViewById(android.R.id.title)).getCurrentTextColor());
        assertEquals(color(theme, com.google.android.material.R.attr.colorOnSurfaceVariant),
                ((android.widget.TextView) row.findViewById(android.R.id.summary)).getCurrentTextColor());
        assertEquals(primary, androidx.appcompat.content.res.AppCompatResources.getColorStateList(theme, R.color.card_stroke)
                .getColorForState(new int[] {android.R.attr.state_focused}, 0));
        assertEquals(color(theme, com.google.android.material.R.attr.colorOnPrimaryContainer),
                androidx.appcompat.content.res.AppCompatResources.getColorStateList(theme, R.color.chip_text)
                        .getColorForState(new int[] {android.R.attr.state_checked}, 0));
        android.graphics.Bitmap bitmap = android.graphics.Bitmap.createBitmap(64, 64, android.graphics.Bitmap.Config.ARGB_8888);
        android.graphics.drawable.Drawable icon = androidx.appcompat.content.res.AppCompatResources.getDrawable(theme, R.drawable.ic_gamepad);
        icon.setBounds(0, 0, 64, 64);
        icon.draw(new android.graphics.Canvas(bitmap));
        double iconContrast = 0;
        for (int y = 0; y < 64; y++) for (int x = 0; x < 64; x++) {
            int pixel = bitmap.getPixel(x, y);
            if (android.graphics.Color.alpha(pixel) == 255) {
                iconContrast = Math.max(iconContrast, ColorUtils.calculateContrast(pixel, surface));
            }
        }
        assertTrue("untinted icons", iconContrast >= NON_TEXT);
    }

    @Test
    @Config(qualifiers = "night")
    public void nightThemeIsAmberOnNightAndReadable() {
        Context app = themed(R.style.AppTheme);
        assertEquals(0xFF14161F, color(app, com.google.android.material.R.attr.colorSurface));
        assertEquals(0xFFF2A93B, color(app, androidx.appcompat.R.attr.colorPrimary));
        assertReadable(app);
    }

    @Test
    @Config(qualifiers = "notnight")
    public void dayThemeUsesADarkerBronzeThatStaysReadable() {
        Context app = themed(R.style.AppTheme);
        assertEquals(0xFF845500, color(app, androidx.appcompat.R.attr.colorPrimary));
        assertReadable(app);
    }

    @Test
    @Config(qualifiers = "notnight")
    public void streamThemeStaysDarkWhenTheSystemThemeIsLight() {
        Context stream = themed(R.style.StreamTheme);
        Context app = themed(R.style.AppTheme);
        assertEquals(0xFF14161F, color(stream, com.google.android.material.R.attr.colorSurface));
        assertEquals(0xFFF2A93B, color(stream, androidx.appcompat.R.attr.colorPrimary));
        assertNotEquals(color(app, com.google.android.material.R.attr.colorSurface),
                color(stream, com.google.android.material.R.attr.colorSurface));
        TypedValue light = new TypedValue();
        assertTrue(stream.getTheme().resolveAttribute(androidx.appcompat.R.attr.isLightTheme, light, true));
        assertEquals("sheets and dialogs over a game must be dark", 0, light.data);
        assertReadable(stream);
    }

    @Test
    @Config(qualifiers = "notnight")
    public void oledThemeMakesEverySurfaceBlackWithReadableNightColours() {
        Context oled = themed(R.style.OledBlackTheme);
        for (int attr : new int[] {android.R.attr.windowBackground, android.R.attr.colorBackground,
                com.google.android.material.R.attr.colorSurface,
                com.google.android.material.R.attr.colorSurfaceDim,
                com.google.android.material.R.attr.colorSurfaceBright,
                com.google.android.material.R.attr.colorSurfaceContainerLowest,
                com.google.android.material.R.attr.colorSurfaceContainerLow,
                com.google.android.material.R.attr.colorSurfaceContainer,
                com.google.android.material.R.attr.colorSurfaceContainerHigh,
                com.google.android.material.R.attr.colorSurfaceContainerHighest,
                com.google.android.material.R.attr.colorSurfaceVariant,
                com.google.android.material.R.attr.colorSurfaceInverse}) {
            assertEquals("surface " + attr, 0xff000000, color(oled, attr));
        }
        android.content.res.ColorStateList card = androidx.appcompat.content.res.AppCompatResources
                .getColorStateList(oled, R.color.card_surface);
        assertEquals(0xff000000, card.getDefaultColor());
        assertEquals(0xff000000, card.getColorForState(new int[] {android.R.attr.state_hovered}, 0));
        assertEquals(0xff000000, card.getColorForState(new int[] {android.R.attr.state_activated}, 0));
        assertTrue(ColorUtils.calculateContrast(color(oled, com.google.android.material.R.attr.colorOnSurfaceInverse),
                color(oled, com.google.android.material.R.attr.colorSurfaceInverse)) >= TEXT);
        assertReadable(oled);
    }

    private static Map<String, String> items(String theme) throws Exception {
        DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
        Document styles = factory.newDocumentBuilder().parse(new File("src/main/res/values/styles.xml"));
        NodeList list = styles.getElementsByTagName("style");
        for (int i = 0; i < list.getLength(); i++) {
            Element style = (Element) list.item(i);
            if (!style.getAttribute("name").equals(theme)) continue;
            Map<String, String> result = new TreeMap<>();
            NodeList entries = style.getElementsByTagName("item");
            for (int j = 0; j < entries.getLength(); j++) {
                Element item = (Element) entries.item(j);
                result.put(item.getAttribute("name"), item.getTextContent().trim());
            }
            return result;
        }
        throw new AssertionError("Missing theme " + theme);
    }

    @Test
    public void streamThemeMatchesTheAppThemeExceptForItsFixedNightColours() throws Exception {
        Map<String, String> app = items("Base.Theme.Nightjar");
        Map<String, String> stream = items("Theme.Nightjar.Stream");
        // The app's windows fade and settle on a TV; the stream window keeps the system's own animation.
        app.remove("android:windowAnimationStyle");
        assertEquals(app.keySet(), stream.keySet());
        for (Map.Entry<String, String> entry : app.entrySet()) {
            String name = entry.getKey();
            String appValue = entry.getValue();
            String streamValue = stream.get(name);
            if (name.equals("android:windowBackground")) continue;
            if (appValue.startsWith("@color/") && !appValue.contains("_fixed")) {
                assertEquals(name, "@color/night_" + appValue.substring("@color/".length()), streamValue);
            } else {
                assertEquals(name, appValue, streamValue);
            }
        }
    }
}
