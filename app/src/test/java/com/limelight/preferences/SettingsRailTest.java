package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Intent;
import android.graphics.Rect;
import android.preference.PreferenceManager;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ImageView;
import android.widget.ListView;

import com.limelight.R;
import com.limelight.ui.Motion;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.TimeUnit;

/** TV Settings' rail: the amber bar on the current section, the icons of its rows, and the right pane coming in. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, qualifiers = "w960dp-h540dp-land-television-xhdpi",
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SettingsRailTest {
    private ActivityController<StreamSettings> controller;

    @After
    public void tearDown() {
        if (controller != null) controller.pause().stop().destroy();
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
    }

    private StreamSettings open(String motion) {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putString(Motion.REDUCE_MOTION, motion).commit();
        controller = Robolectric.buildActivity(StreamSettings.class).setup().visible();
        settle(controller.get());
        return controller.get();
    }

    private static void settle(StreamSettings screen) {
        ShadowLooper.idleMainLooper(300, TimeUnit.MILLISECONDS);
        screen.getFragmentManager().executePendingTransactions();
        layOut(screen);
    }

    /** Measures, lays out and pre-draws the screen at the display's size without moving the clock. */
    private static void layOut(StreamSettings screen) {
        View root = screen.findViewById(android.R.id.content);
        int width = screen.getResources().getDisplayMetrics().widthPixels;
        int height = screen.getResources().getDisplayMetrics().heightPixels;
        root.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, width, height);
        View rail = screen.findViewById(R.id.settings_rail);
        if (rail != null) rail.getViewTreeObserver().dispatchOnPreDraw();
    }

    private static ListView sections(StreamSettings screen) {
        return (ListView) screen.getFragmentManager().findFragmentById(R.id.settings_sections).getView()
                .findViewById(android.R.id.list);
    }

    /** Scrolls the rail so the section's row is on screen, as the D-pad does on its way to it, and lays the screen out. */
    private static void reveal(StreamSettings screen, String key) {
        ListView list = sections(screen);
        screen.getFragmentManager().executePendingTransactions();
        list.setSelectionFromTop(StreamSettings.RootFragment.positionOf(list, key), list.getHeight() / 4);
        layOut(screen);
    }

    private static SettingsRail rail(StreamSettings screen) {
        SettingsRail rail = ReflectionHelpers.getField(screen, "rail");
        assertNotNull("the TV layout has the rail", rail);
        return rail;
    }

    /** The middle of the section's row, where the bar's middle belongs, in the rail's coordinates. */
    private static float middleOf(StreamSettings screen, String key) {
        ListView list = sections(screen);
        View row = list.getChildAt(StreamSettings.RootFragment.positionOf(list, key) - list.getFirstVisiblePosition());
        assertNotNull(row);
        Rect rect = new Rect(0, 0, row.getWidth(), row.getHeight());
        ((ViewGroup) screen.findViewById(R.id.settings_rail)).offsetDescendantRectToMyCoords(row, rect);
        return rect.exactCenterY();
    }

    private static float barMiddle(StreamSettings screen) {
        View bar = screen.findViewById(R.id.settings_rail_bar);
        return bar.getTranslationY() + bar.getHeight() / 2f;
    }

    @Test
    public void theBarMarksTheShownSectionFromTheStartWithNoSlide() {
        StreamSettings screen = open(Motion.FULL);
        View bar = screen.findViewById(R.id.settings_rail_bar);
        assertEquals(View.VISIBLE, bar.getVisibility());
        assertEquals("a TV opens on Input", middleOf(screen, "input"), barMiddle(screen), 1f);
        assertFalse(rail(screen).sliding());
        android.graphics.drawable.Drawable canvas = screen.getWindow().getDecorView().getBackground();
        assertTrue("the window is the still radial light", canvas instanceof android.graphics.drawable.GradientDrawable
                && ((android.graphics.drawable.GradientDrawable) canvas).getGradientType()
                == android.graphics.drawable.GradientDrawable.RADIAL_GRADIENT);
    }

    @Test
    public void anotherSectionSlidesTheBarAndBringsThePaneIn() {
        StreamSettings screen = open(Motion.FULL);
        screen.showSection("audio");
        View pane = screen.findViewById(R.id.stream_settings);
        assertTrue("the pane starts faded", pane.getAlpha() < 1f);
        assertTrue("and a little to the side", pane.getTranslationX() > 0f);
        reveal(screen, "audio");
        SettingsRail rail = rail(screen);
        assertEquals(StreamSettings.RootFragment.positionOf(sections(screen), "audio"), rail.position());
        assertTrue("the bar slides to the new row", rail.sliding());
        ShadowLooper.idleMainLooper(SettingsRail.BAR_MS + 50, TimeUnit.MILLISECONDS);
        assertFalse(rail.sliding());
        assertEquals("and stops on it", middleOf(screen, "audio"), barMiddle(screen), 1f);
        assertEquals(1f, pane.getAlpha(), 0f);
    }

    @Test
    public void withReducedMotionTheBarJumpsAndThePaneIsThereAtOnce() {
        StreamSettings screen = open(Motion.REDUCE);
        screen.showSection("audio");
        View pane = screen.findViewById(R.id.stream_settings);
        assertEquals(1f, pane.getAlpha(), 0f);
        assertEquals(0f, pane.getTranslationX(), 0f);
        reveal(screen, "audio");
        assertFalse(rail(screen).sliding());
        assertEquals(middleOf(screen, "audio"), barMiddle(screen), 1f);
        assertEquals(View.VISIBLE, screen.findViewById(R.id.settings_rail_bar).getVisibility());
    }

    @Test
    public void theRailsRowsHaveTheirIcons() {
        StreamSettings screen = open(Motion.FULL);
        ListView list = sections(screen);
        for (String key : new String[] {"input", "video", "display", "audio", "streaming", "app", "advanced"}) {
            reveal(screen, key);
            View row = list.getChildAt(StreamSettings.RootFragment.positionOf(list, key) - list.getFirstVisiblePosition());
            assertNotNull(key, row);
            ImageView icon = row.findViewById(android.R.id.icon);
            assertNotNull(key, icon.getDrawable());
            assertNotNull(key + " is tinted with the rail's colour", androidx.core.widget.ImageViewCompat.getImageTintList(icon));
        }
        reveal(screen, "input");
        assertTrue("the shown section's row is the filled pill", list.getChildAt(
                StreamSettings.RootFragment.positionOf(list, "input") - list.getFirstVisiblePosition()).isActivated());
    }

    @Test
    @Config(qualifiers = "w1280dp-h720dp-land-mdpi")
    public void aWideScreenThatIsNotATvHasNoRail() {
        StreamSettings screen = open(Motion.FULL);
        assertNull(screen.findViewById(R.id.settings_rail_bar));
        assertNull(ReflectionHelpers.getField(screen, "rail"));
    }

    private static java.util.List<Integer> headingLayouts(android.preference.PreferenceGroup group, java.util.List<Integer> into) {
        for (int i = 0; i < group.getPreferenceCount(); i++) {
            android.preference.Preference preference = group.getPreference(i);
            if (preference instanceof android.preference.PreferenceCategory && !android.text.TextUtils.isEmpty(preference.getTitle())) {
                into.add(preference.getLayoutResource());
            }
            if (preference instanceof android.preference.PreferenceGroup) {
                headingLayouts((android.preference.PreferenceGroup) preference, into);
            }
        }
        return into;
    }

    @Test
    public void theRailsPaneHeadsItsGroupsInSentenceCaseAndTheHostPagesKeepTheirHeadings() {
        StreamSettings screen = open(Motion.FULL);
        screen.showSection("display");
        settle(screen);
        android.preference.PreferenceFragment pane =
                (android.preference.PreferenceFragment) screen.getFragmentManager().findFragmentById(R.id.stream_settings);
        java.util.List<Integer> layouts = headingLayouts(pane.getPreferenceScreen(), new java.util.ArrayList<>());
        assertTrue("the display section has headed groups", !layouts.isEmpty());
        for (int layout : layouts) assertEquals(R.layout.settings_category_tv, layout);

        android.widget.TextView heading = (android.widget.TextView) screen.getLayoutInflater()
                .inflate(R.layout.settings_category_tv, null, false);
        assertNull("no capitals", heading.getTransformationMethod());
        assertEquals(0f, heading.getLetterSpacing(), 0f);
        assertEquals(16 * screen.getResources().getDisplayMetrics().scaledDensity, heading.getTextSize(), 0.5f);

        Intent intent = new Intent(screen, HostStreamSettings.class).putExtra("host_uuid", "den").putExtra("host_name", "Den PC");
        ActivityController<HostStreamSettings> host = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            android.preference.PreferenceFragment settings = (android.preference.PreferenceFragment)
                    host.get().getFragmentManager().findFragmentById(R.id.host_profile_settings);
            java.util.List<Integer> hostLayouts = headingLayouts(settings.getPreferenceScreen(), new java.util.ArrayList<>());
            assertTrue(!hostLayouts.isEmpty());
            for (int layout : hostLayouts) assertEquals(R.layout.settings_category, layout);
        } finally {
            host.pause().stop().destroy();
        }
    }
}
