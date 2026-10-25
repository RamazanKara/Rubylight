package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.os.Bundle;
import android.preference.PreferenceManager;
import android.provider.Settings;
import android.util.TypedValue;
import android.view.LayoutInflater;
import android.widget.FrameLayout;

import com.google.android.material.card.MaterialCardView;
import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

import java.io.File;

import javax.xml.parsers.DocumentBuilderFactory;

/** Reduced motion: one decision, and the focus animators that follow it. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class)
public class MotionTest {
    private static void set(String setting) {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication())
                .edit().putString(Motion.REDUCE_MOTION, setting).commit();
    }

    private static void animatorScale(float scale) {
        Settings.Global.putFloat(RuntimeEnvironment.getApplication().getContentResolver(),
                Settings.Global.ANIMATOR_DURATION_SCALE, scale);
    }

    @Test
    public void followSystemReducesOnlyWhenTheSystemTurnsAnimationOff() {
        assertFalse(Motion.decide(Motion.FOLLOW_SYSTEM, 1f, true));
        assertFalse(Motion.decide(Motion.FOLLOW_SYSTEM, 0.5f, true));
        assertFalse(Motion.decide(Motion.FOLLOW_SYSTEM, 10f, true));
        assertTrue(Motion.decide(Motion.FOLLOW_SYSTEM, 0f, true));
        assertTrue("animators off for the app (battery saver)", Motion.decide(Motion.FOLLOW_SYSTEM, 1f, false));
    }

    @Test
    public void reduceAndFullIgnoreTheSystem() {
        for (float scale : new float[] {0f, 1f}) {
            for (boolean enabled : new boolean[] {false, true}) {
                assertTrue(Motion.decide(Motion.REDUCE, scale, enabled));
                assertFalse(Motion.decide(Motion.FULL, scale, enabled));
            }
        }
    }

    @Test
    public void anUnknownSettingFollowsTheSystem() {
        set("sideways");
        assertEquals(Motion.FOLLOW_SYSTEM, Motion.setting(RuntimeEnvironment.getApplication()));
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
        assertEquals(Motion.FOLLOW_SYSTEM, Motion.setting(RuntimeEnvironment.getApplication()));
    }

    @Test
    public void theSettingAndTheSystemAnimatorScaleDecideForAContext() {
        Context context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        animatorScale(1f);
        assertFalse(Motion.reduced(context));
        animatorScale(0f);
        assertTrue("Follow system with the animator scale at 0", Motion.reduced(context));
        set(Motion.FULL);
        assertFalse(Motion.reduced(context));
        animatorScale(1f);
        set(Motion.REDUCE);
        assertTrue(Motion.reduced(context));
    }

    /** The animators a reduced screen uses never zoom and never wait. */
    @Test
    public void staticAnimatorsHaveNoScaleAndNoDuration() throws Exception {
        for (String name : new String[] {"tv_card_focus_static", "tv_control_focus_static",
                "tv_card_focus_hc_static", "tv_control_focus_hc_static"}) {
            Document document = DocumentBuilderFactory.newInstance().newDocumentBuilder()
                    .parse(new File("src/main/res/animator/" + name + ".xml"));
            NodeList animators = document.getElementsByTagName("objectAnimator");
            assertTrue(name, animators.getLength() > 0);
            for (int i = 0; i < animators.getLength(); i++) {
                Element animator = (Element) animators.item(i);
                String property = animator.getAttribute("android:propertyName");
                assertTrue(name + " animates " + property, property.equals("strokeWidth"));
                assertEquals(name, "0", animator.getAttribute("android:duration"));
            }
        }
    }

    /** The full animators do zoom, so the checks above and below are not vacuous. */
    @Test
    public void fullAnimatorsZoom() throws Exception {
        for (String name : new String[] {"tv_card_focus", "tv_control_focus", "tv_card_focus_hc", "tv_control_focus_hc"}) {
            Document document = DocumentBuilderFactory.newInstance().newDocumentBuilder()
                    .parse(new File("src/main/res/animator/" + name + ".xml"));
            NodeList animators = document.getElementsByTagName("objectAnimator");
            boolean zooms = false;
            for (int i = 0; i < animators.getLength(); i++) {
                zooms |= ((Element) animators.item(i)).getAttribute("android:propertyName").equals("scaleX");
            }
            assertTrue(name, zooms);
        }
    }

    private static Element parse(String path) throws Exception {
        return DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(new File(path)).getDocumentElement();
    }

    /** The poster tweens are the spec's: 200 ms to the focus scale, 150 ms back, 70 ms to the press scale; nothing else moves. */
    @Test
    public void posterAnimatorsFollowTheMotionSpec() throws Exception {
        for (String[] spec : new String[][] {{"tv_poster_focus", "1.08"}, {"tv_poster_focus_small", "1.10"}}) {
            NodeList items = parse("src/main/res/animator/" + spec[0] + ".xml").getElementsByTagName("item");
            boolean pressed = false, focused = false, rest = false;
            for (int i = 0; i < items.getLength(); i++) {
                Element item = (Element) items.item(i);
                NodeList tweens = item.getElementsByTagName("objectAnimator");
                for (int t = 0; t < tweens.getLength(); t++) {
                    String property = ((Element) tweens.item(t)).getAttribute("android:propertyName");
                    assertTrue(spec[0] + " animates " + property, property.matches("scaleX|scaleY|translationZ"));
                }
                Element first = (Element) tweens.item(0);
                if (item.hasAttribute("android:state_pressed")) {
                    pressed = true;
                    assertEquals("0.96", first.getAttribute("android:valueTo"));
                    assertEquals("70", first.getAttribute("android:duration"));
                } else if (item.hasAttribute("android:state_selected")) {
                    focused = true;
                    assertEquals(spec[1], first.getAttribute("android:valueTo"));
                    assertEquals("200", first.getAttribute("android:duration"));
                } else if (!item.hasAttribute("android:state_focused") && !item.hasAttribute("android:state_hovered")) {
                    rest = true;
                    assertEquals("1", first.getAttribute("android:valueTo"));
                    assertEquals("150", first.getAttribute("android:duration"));
                }
            }
            assertTrue(spec[0], pressed && focused && rest);
        }
        NodeList still = parse("src/main/res/animator/tv_poster_focus_static.xml").getElementsByTagName("objectAnimator");
        java.util.Set<String> reset = new java.util.HashSet<>();
        for (int i = 0; i < still.getLength(); i++) {
            Element tween = (Element) still.item(i);
            assertEquals("0", tween.getAttribute("android:duration"));
            String property = tween.getAttribute("android:propertyName");
            reset.add(property);
            assertEquals(property, property.equals("translationZ") ? "0dp" : "1", tween.getAttribute("android:valueTo"));
        }
        assertEquals("the still animator undoes everything the full one moves",
                new java.util.HashSet<>(java.util.Arrays.asList("scaleX", "scaleY", "translationZ")), reset);
    }

    /** What a poster does when it is selected, run on the inflated layout: the scale and lift the XML promises. */
    private static void assertPosterFocus(Context context, int layout, float scale, float lift) {
        android.view.View card = LayoutInflater.from(context).inflate(layout, null);
        card.setSelected(true);
        FastTravel.settle(card);
        assertEquals(scale, card.getScaleX(), 0.001f);
        assertEquals(scale, card.getScaleY(), 0.001f);
        assertEquals(lift, card.getTranslationZ(), 0.01f);
        card.setSelected(false);
        FastTravel.settle(card);
        assertEquals(1f, card.getScaleX(), 0.001f);
        assertEquals(0f, card.getTranslationZ(), 0.01f);
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-television-xhdpi")
    public void aSelectedPosterReachesItsFocusScaleAndLift() {
        animatorScale(1f);
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            float lift = 6 * controller.get().getResources().getDisplayMetrics().density;
            assertPosterFocus(controller.get(), R.layout.app_grid_item, 1.08f, lift);
            assertPosterFocus(controller.get(), R.layout.app_grid_item_small, 1.10f, lift);
            assertPosterFocus(controller.get(), R.layout.console_card_cover, 1.08f, lift);
        }
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-television-xhdpi")
    public void withReducedMotionASelectedPosterStaysPutAndKeepsItsRing() {
        animatorScale(1f);
        set(Motion.REDUCE);
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            android.view.View card = LayoutInflater.from(controller.get()).inflate(R.layout.app_grid_item, null);
            card.setSelected(true);
            FastTravel.settle(card);
            assertEquals(1f, card.getScaleX(), 0f);
            assertEquals(1f, card.getScaleY(), 0f);
            assertEquals(0f, card.getTranslationZ(), 0f);
            android.view.View ring = card.findViewById(R.id.grid_ring);
            assertTrue("the ring shows the selection at once", java.util.Arrays.binarySearch(
                    sorted(ring.getDrawableState()), android.R.attr.state_selected) >= 0);
            assertEquals(R.drawable.tv_poster_ring_static, resolved(controller.get(), R.attr.nightjarPosterRing));
        }
    }

    /** The ring fades in 120 ms and out in 90, the glow 160 and 120; with reduced motion neither fades. */
    @Test
    public void posterRingAndGlowFadeAsSpecifiedAndNotAtAllWhenReduced() throws Exception {
        String[][] fades = {{"tv_poster_ring", "120", "90"}, {"tv_poster_ring_hc", "120", "90"},
                {"tv_focus_glow_fade", "160", "120"}, {"tv_poster_ring_static", "", ""},
                {"tv_poster_ring_hc_static", "", ""}, {"tv_focus_glow_static", "", ""}};
        for (String[] fade : fades) {
            Element selector = parse("src/main/res/drawable/" + fade[0] + ".xml");
            assertEquals(fade[0], fade[1], selector.getAttribute("android:enterFadeDuration"));
            assertEquals(fade[0], fade[2], selector.getAttribute("android:exitFadeDuration"));
        }
        // The thick high-contrast ring is 4 dp, the normal one 3 dp.
        assertEquals("3dp", ((Element) parse("src/main/res/drawable/tv_poster_ring_shape.xml")
                .getElementsByTagName("stroke").item(0)).getAttribute("android:width"));
        assertEquals("4dp", ((Element) parse("src/main/res/drawable/tv_poster_ring_shape_hc.xml")
                .getElementsByTagName("stroke").item(0)).getAttribute("android:width"));
    }

    public static class Screen extends NightjarActivity {
        @Override protected void onCreate(Bundle state) {
            super.onCreate(state);
            setContentView(new FrameLayout(this));
        }
    }

    private static int cardAnimator(Context context) {
        TypedValue value = new TypedValue();
        assertTrue(context.getTheme().resolveAttribute(R.attr.nightjarCardFocusAnimator, value, true));
        return value.resourceId;
    }

    @Test
    @Config(qualifiers = "television")
    public void withTheAnimatorScaleAtZeroATelevisionCardDoesNotZoomOnFocus() {
        animatorScale(0f);
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen screen = controller.get();
            assertEquals(R.animator.tv_card_focus_static, cardAnimator(screen));
            MaterialCardView card = (MaterialCardView) LayoutInflater.from(screen).inflate(R.layout.pc_grid_item, null);
            card.setSelected(true);
            card.getStateListAnimator().jumpToCurrentState();
            assertEquals(1f, card.getScaleX(), 0f);
            assertEquals(1f, card.getScaleY(), 0f);
            assertTrue("the ring still shows", card.getStrokeWidth() > 0);
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void withFullMotionATelevisionCardZoomsOnFocus() {
        animatorScale(1f);
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen screen = controller.get();
            assertEquals(R.animator.tv_card_focus, cardAnimator(screen));
            MaterialCardView card = (MaterialCardView) LayoutInflater.from(screen).inflate(R.layout.pc_grid_item, null);
            card.setSelected(true);
            card.getStateListAnimator().jumpToCurrentState();
            assertNotEquals(1f, card.getScaleX(), 0.001f);
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void reduceMotionOverridesAFullSystemAnimatorScale() {
        animatorScale(1f);
        set(Motion.REDUCE);
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            assertEquals(R.animator.tv_card_focus_static, cardAnimator(controller.get()));
            ActionSheet sheet = new ActionSheet(controller.get(), "Options");
            assertNotEquals("no TV slide for the sheet", R.style.TvSheetAnimation,
                    sheet.getWindow().getAttributes().windowAnimations);
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void fullMotionKeepsTheSheetSlideOnTelevision() {
        animatorScale(1f);
        set(Motion.FULL);
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            ActionSheet sheet = new ActionSheet(controller.get(), "Options");
            assertEquals(R.style.TvSheetAnimation, sheet.getWindow().getAttributes().windowAnimations);
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void sheetRowsUseTheStillFocusAnimatorWhenMotionIsReduced() {
        animatorScale(0f);
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            ActionSheet sheet = new ActionSheet(controller.get(), "Options");
            com.google.android.material.button.MaterialButton row = sheet.addAction(R.drawable.ic_settings, "Settings");
            row.setSelected(true);
            row.getStateListAnimator().jumpToCurrentState();
            assertEquals(1f, row.getScaleX(), 0f);
        }
    }

    @Test
    public void highContrastWithReducedMotionPicksTheThickStillRing() {
        animatorScale(0f);
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putBoolean(AppTheme.HIGH_CONTRAST, true).commit();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            assertEquals(R.animator.tv_card_focus_hc_static, cardAnimator(controller.get()));
        }
    }

    @Test
    public void highContrastAloneKeepsTheZoomAndThickensTheRing() {
        animatorScale(1f);
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putBoolean(AppTheme.HIGH_CONTRAST, true).commit();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            assertEquals(R.animator.tv_card_focus_hc, cardAnimator(controller.get()));
        }
    }

    @Test
    public void theStreamActivityKeepsItsOwnLook() {
        animatorScale(0f);
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putBoolean(AppTheme.HIGH_CONTRAST, true).commit();
        com.limelight.Game game = Robolectric.buildActivity(com.limelight.Game.class).get();
        AppTheme.apply(game);
        assertEquals("no overlays over a stream", R.animator.tv_card_focus, cardAnimator(game));
    }

    private static int[] sorted(int[] values) {
        int[] copy = values.clone();
        java.util.Arrays.sort(copy);
        return copy;
    }

    private static int resolved(Context context, int attr) {
        TypedValue value = new TypedValue();
        assertTrue(context.getTheme().resolveAttribute(attr, value, true));
        return value.resourceId;
    }

    private static android.view.View poster(Screen screen, int layout, boolean selected) {
        android.view.View poster = LayoutInflater.from(screen).inflate(layout, null);
        poster.setSelected(selected);
        poster.jumpDrawablesToCurrentState();
        poster.getStateListAnimator().jumpToCurrentState();
        return poster;
    }

    @Test
    @Config(qualifiers = "television")
    public void withFullMotionAPosterPopsAndLiftsAndItsRingAndGlowFade() {
        animatorScale(1f);
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen screen = controller.get();
            assertEquals(R.drawable.tv_poster_ring, resolved(screen, R.attr.nightjarPosterRing));
            assertEquals(R.drawable.tv_focus_glow_fade, resolved(screen, R.attr.nightjarPosterGlow));
            float density = screen.getResources().getDisplayMetrics().density;
            android.view.View large = poster(screen, R.layout.app_grid_item, true);
            assertEquals(1.08f, large.getScaleX(), 0.001f);
            assertEquals(1.08f, large.getScaleY(), 0.001f);
            assertEquals("lifted over its neighbours", 6 * density, large.getTranslationZ(), 0.01f);
            android.view.View small = poster(screen, R.layout.app_grid_item_small, true);
            assertEquals("a small poster pops a little more", 1.10f, small.getScaleX(), 0.001f);
            android.view.View rest = poster(screen, R.layout.app_grid_item, false);
            assertEquals(1f, rest.getScaleX(), 0f);
            assertEquals(0f, rest.getTranslationZ(), 0f);
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void withReducedMotionAPosterStaysPutAndItsRingAndGlowShowAtOnce() {
        set(Motion.REDUCE);
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen screen = controller.get();
            assertEquals(R.animator.tv_poster_focus_static, resolved(screen, R.attr.nightjarPosterFocusAnimator));
            assertEquals(R.animator.tv_poster_focus_static, resolved(screen, R.attr.nightjarPosterFocusAnimatorSmall));
            assertEquals(R.drawable.tv_poster_ring_static, resolved(screen, R.attr.nightjarPosterRing));
            assertEquals(R.drawable.tv_focus_glow_static, resolved(screen, R.attr.nightjarPosterGlow));
            android.view.View poster = poster(screen, R.layout.app_grid_item, true);
            assertEquals(1f, poster.getScaleX(), 0f);
            assertEquals(0f, poster.getTranslationZ(), 0f);
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void highContrastThickensThePosterRingAndDropsItsGlow() {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putBoolean(AppTheme.HIGH_CONTRAST, true).commit();
        animatorScale(1f);
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            assertEquals(R.drawable.tv_poster_ring_hc, resolved(controller.get(), R.attr.nightjarPosterRing));
            assertEquals(R.drawable.tv_focus_glow_none, resolved(controller.get(), R.attr.nightjarPosterGlow));
            assertEquals("the zoom stays", R.animator.tv_poster_focus,
                    resolved(controller.get(), R.attr.nightjarPosterFocusAnimator));
        }
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putString(Motion.REDUCE_MOTION, Motion.REDUCE).commit();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            assertEquals(R.drawable.tv_poster_ring_hc_static, resolved(controller.get(), R.attr.nightjarPosterRing));
            assertEquals(R.animator.tv_poster_focus_static, resolved(controller.get(), R.attr.nightjarPosterFocusAnimator));
        }
    }
}
