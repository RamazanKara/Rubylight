package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.preference.PreferenceManager;
import android.provider.Settings;
import android.util.TypedValue;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.R;
import com.limelight.utils.ServerHelper;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

import java.io.File;
import java.util.HashMap;
import java.util.Map;

import javax.xml.parsers.DocumentBuilderFactory;

/** Windows open and close with a short fade and settle on a TV, a game grows out of its poster, and Reduce motion does none of it. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class TvTransitionsTest {
    @Before
    public void reset() {
        Settings.Global.putFloat(RuntimeEnvironment.getApplication().getContentResolver(),
                Settings.Global.ANIMATOR_DURATION_SCALE, 1f);
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
    }

    private static Element parse(String path) throws Exception {
        return DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(new File(path)).getDocumentElement();
    }

    private static Map<String, String> styleItems(String file, String style) throws Exception {
        NodeList styles = parse(file).getElementsByTagName("style");
        for (int i = 0; i < styles.getLength(); i++) {
            Element element = (Element) styles.item(i);
            if (!element.getAttribute("name").equals(style)) continue;
            Map<String, String> items = new HashMap<>();
            NodeList list = element.getElementsByTagName("item");
            for (int j = 0; j < list.getLength(); j++) {
                Element item = (Element) list.item(j);
                items.put(item.getAttribute("name"), item.getTextContent().trim());
            }
            return items;
        }
        throw new AssertionError(style + " is not in " + file);
    }

    private static int windowAnimationStyle(Context context) {
        // The screens call this from onCreate; the plain test screen does not.
        if (context instanceof android.app.Activity) AppTheme.apply((android.app.Activity) context);
        TypedValue value = new TypedValue();
        assertTrue(context.getTheme().resolveAttribute(android.R.attr.windowAnimationStyle, value, true));
        return value.resourceId;
    }

    @Test
    public void aTvWindowFadesAndSettlesInTwoHundredTwentyMilliseconds() throws Exception {
        Map<String, String> tv = styleItems("src/main/res/values-television/tv_motion.xml", "NightjarWindowAnimation");
        assertEquals("@anim/tv_open_enter", tv.get("android:activityOpenEnterAnimation"));
        assertEquals("@anim/tv_open_exit", tv.get("android:activityOpenExitAnimation"));
        assertEquals("@anim/tv_close_enter", tv.get("android:activityCloseEnterAnimation"));
        assertEquals("@anim/tv_close_exit", tv.get("android:activityCloseExitAnimation"));
        for (String name : new String[] {"tv_open_enter", "tv_open_exit", "tv_close_enter", "tv_close_exit"}) {
            Element set = parse("src/main/res/anim/" + name + ".xml");
            NodeList tweens = set.getChildNodes();
            int count = 0;
            for (int i = 0; i < tweens.getLength(); i++) {
                if (!(tweens.item(i) instanceof Element)) continue;
                count++;
                String duration = ((Element) tweens.item(i)).getAttribute("android:duration");
                assertTrue(name + " lasts " + duration + " ms", Integer.parseInt(duration) <= 220);
            }
            assertTrue(name, count > 0);
        }
    }

    @Test
    public void withReduceMotionATvWindowOpensAndClosesWithoutAnimation() throws Exception {
        Map<String, String> still = styleItems("src/main/res/values-television/tv_motion.xml", "NightjarWindowAnimationStill");
        assertEquals(4, still.size());
        for (Map.Entry<String, String> item : still.entrySet()) {
            assertEquals(item.getKey(), "@null", item.getValue());
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void theThemeUsesTheMovingStyleAndReduceMotionSwapsInTheStillOne() {
        try (var controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            assertEquals(R.style.NightjarWindowAnimation, windowAnimationStyle(controller.get()));
        }
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putString(Motion.REDUCE_MOTION, Motion.REDUCE).commit();
        try (var controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            assertEquals(R.style.NightjarWindowAnimationStill, windowAnimationStyle(controller.get()));
        }
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putBoolean(AppTheme.HIGH_CONTRAST, true).commit();
        try (var controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            assertEquals("also in high contrast", R.style.NightjarWindowAnimationStill, windowAnimationStyle(controller.get()));
        }
    }

    @Test
    public void theStreamKeepsItsOwnWindowAnimation() throws Exception {
        Map<String, String> stream = styleItems("src/main/res/values/styles.xml", "Theme.Nightjar.Stream");
        assertFalse("the stream's theme names no window animation of its own", stream.containsKey("android:windowAnimationStyle"));
    }

    private View poster(UiNavigationTest.Screen screen, int width, int height) {
        FrameLayout cell = new FrameLayout(screen);
        View card = new View(screen);
        card.setId(R.id.grid_card);
        cell.addView(card, new FrameLayout.LayoutParams(width, height));
        cell.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        cell.layout(0, 0, width, height);
        return cell;
    }

    @Test
    @Config(qualifiers = "television")
    public void aGameGrowsOutOfThePosterThatWasChosen() {
        try (var controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            assertNotNull(ServerHelper.launchOptions(controller.get(), poster(controller.get(), 300, 450)));
            assertNull("no poster to start from", ServerHelper.launchOptions(controller.get(), null));
            assertNull("a poster that is not laid out", ServerHelper.launchOptions(controller.get(), new View(controller.get())));
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void withReduceMotionAGameStartsWithoutTheGrow() {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putString(Motion.REDUCE_MOTION, Motion.REDUCE).commit();
        try (var controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            assertNull(ServerHelper.launchOptions(controller.get(), poster(controller.get(), 300, 450)));
        }
    }

    @Test
    public void onAPhoneAGameStartsTheUsualWay() {
        try (var controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup()) {
            assertNull(ServerHelper.launchOptions(controller.get(), poster(controller.get(), 300, 450)));
        }
    }

    /** The Home screen's list draws its rows inside its own bounds, so a header scrolled away does not show under the tab bar. */
    @Test
    public void homeClipsItsRowsToTheListSoNothingShowsUnderTheTabBar() throws Exception {
        NodeList lists = parse("src/main/res/layout/activity_console_home.xml").getElementsByTagName("androidx.recyclerview.widget.RecyclerView");
        assertEquals(1, lists.getLength());
        assertEquals("true", ((Element) lists.item(0)).getAttribute("android:clipChildren"));
    }
}
