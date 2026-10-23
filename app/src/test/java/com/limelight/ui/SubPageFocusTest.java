package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.os.Looper;
import android.view.View;
import android.view.ViewGroup;

import com.google.android.material.appbar.MaterialToolbar;
import com.limelight.LatencyBenchmarkActivity;
import com.limelight.R;
import com.limelight.binding.video.GpuDriverStore;
import com.limelight.preferences.ControllerMappingActivity;
import com.limelight.preferences.GpuDriverSettings;
import com.limelight.preferences.KeyboardShortcutsActivity;
import com.limelight.preferences.StreamProfilesActivity;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;

import static org.junit.Assert.*;

/** On a TV every Settings sub-page starts with focus on its content, and each focusable shows the same outline. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, qualifiers = "w960dp-h540dp-land-television-xhdpi",
        shadows = {com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class, SubPageFocusTest.SupportedDriverStore.class},
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
public class SubPageFocusTest {
    @Implements(value = GpuDriverStore.class, isInAndroidSdk = false)
    public static class SupportedDriverStore {
        @Implementation
        protected static boolean isSupported() {
            return true;
        }
    }

    private static boolean within(Class<?> type, View view) {
        for (Object p = view; p instanceof View; p = ((View) p).getParent()) {
            if (type.isInstance(p)) return true;
        }
        return false;
    }

    private static void collect(View view, java.util.List<View> out) {
        if (view.isFocusable() && view.getVisibility() == View.VISIBLE && !(view instanceof android.widget.AbsListView)
                && !(view instanceof android.widget.EditText) && !within(MaterialToolbar.class, view)
                && !(view instanceof android.widget.ScrollView) && !(view instanceof androidx.appcompat.widget.SearchView)) {
            out.add(view);
        }
        if (view instanceof ViewGroup && !(view instanceof android.widget.AbsListView)) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) collect(group.getChildAt(i), out);
        }
    }

    private static <T extends Activity> void check(Class<T> page) {
        ActivityController<T> controller = Robolectric.buildActivity(page).setup().visible();
        try {
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            Activity screen = controller.get();
            // getCurrentFocus() needs a focused window, which Robolectric does not give; the view tree holds the focus.
            View focus = screen.findViewById(android.R.id.content).findFocus();
            assertNotNull(page.getSimpleName() + " starts with focus", focus);
            assertFalse(page.getSimpleName() + " does not start on the toolbar", within(MaterialToolbar.class, focus));
            java.util.List<View> focusables = new java.util.ArrayList<>();
            collect(screen.findViewById(android.R.id.content), focusables);
            for (View view : focusables) {
                assertNotNull(page.getSimpleName() + " outlines " + view, view.getTag(R.id.focus_ring));
                assertNotNull(page.getSimpleName() + " outlines " + view, view.getForeground());
            }
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test public void theLatencyBenchmarkStartsOnItsContent() { check(LatencyBenchmarkActivity.class); }

    @Test public void theControllerPageStartsOnItsContent() { check(ControllerMappingActivity.class); }

    @Test public void theShortcutsPageStartsOnItsContent() { check(KeyboardShortcutsActivity.class); }

    @Test public void theGpuDriverPageStartsOnItsContent() { check(GpuDriverSettings.class); }

    @Test public void theProfilesPageStartsOnItsContent() { check(StreamProfilesActivity.class); }

    @Test public void decoratedButtonsAndRowsGetTheOutline() {
        ActivityController<KeyboardShortcutsActivity> controller =
                Robolectric.buildActivity(KeyboardShortcutsActivity.class).setup().visible();
        try {
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            java.util.List<View> focusables = new java.util.ArrayList<>();
            collect(controller.get().findViewById(R.id.page_content), focusables);
            assertFalse("the page has focusable rows", focusables.isEmpty());
            for (View view : focusables) {
                assertNotNull("outline on " + view, view.getTag(R.id.focus_ring));
                assertNotNull(view.getForeground());
            }
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static View focusOf(Activity screen) {
        return screen.findViewById(android.R.id.content).findFocus();
    }

    private static <T extends Activity> void refocusesAfterARebuild(Class<T> page, int contentId) throws Exception {
        ActivityController<T> controller = Robolectric.buildActivity(page).setup().visible();
        try {
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            Activity screen = controller.get();
            View content = screen.findViewById(contentId);
            assertNotNull(focusOf(screen));
            java.lang.reflect.Method refresh = page.getDeclaredMethod("refresh");
            refresh.setAccessible(true);
            // A rebuild removes the view that had focus.
            refresh.invoke(screen);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            View after = focusOf(screen);
            assertNotNull(page.getSimpleName() + " keeps focus after a rebuild", after);
            assertTrue(page.getSimpleName() + " keeps focus on its content", within(content, after));
            // And focus that fell to the toolbar comes back.
            java.util.ArrayList<View> bar = new java.util.ArrayList<>();
            findToolbar(screen.findViewById(android.R.id.content)).addFocusables(bar, View.FOCUS_DOWN, View.FOCUSABLES_ALL);
            assertFalse("the toolbar has a focusable", bar.isEmpty());
            assertTrue(bar.get(0).requestFocus());
            assertTrue(within(MaterialToolbar.class, focusOf(screen)));
            refresh.invoke(screen);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertTrue(page.getSimpleName() + " leaves the toolbar after a rebuild", within(content, focusOf(screen)));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static View findToolbar(View view) {
        if (view instanceof MaterialToolbar) return view;
        if (view instanceof ViewGroup) {
            for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
                View found = findToolbar(((ViewGroup) view).getChildAt(i));
                if (found != null) return found;
            }
        }
        return null;
    }

    private static boolean within(View container, View view) {
        for (Object p = view; p instanceof View; p = ((View) p).getParent()) {
            if (p == container) return true;
        }
        return false;
    }

    @Test public void theShortcutsPageRefocusesAfterARebuild() throws Exception {
        refocusesAfterARebuild(KeyboardShortcutsActivity.class, R.id.page_content);
    }

    @Test public void theControllerPageRefocusesAfterARebuild() throws Exception {
        refocusesAfterARebuild(ControllerMappingActivity.class, R.id.page_content);
    }
}
