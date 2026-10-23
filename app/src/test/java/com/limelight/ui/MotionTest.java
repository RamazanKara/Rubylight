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
}
