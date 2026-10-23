package com.limelight.preferences;

import android.app.Application;
import android.preference.Preference;
import android.view.KeyEvent;
import android.view.View;
import android.widget.ListView;

import androidx.appcompat.widget.SearchView;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;

import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

/** Settings shows the sections beside the chosen section on a TV and from 840 dp wide; a phone keeps one pane. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SettingsPanesTest {
    private static final String WIDE = "w1280dp-h720dp-land-mdpi";
    private static final String TV = "w960dp-h540dp-land-television-xhdpi";

    private static void settle(StreamSettings screen) {
        ShadowLooper.idleMainLooper(300, TimeUnit.MILLISECONDS);
        screen.getFragmentManager().executePendingTransactions();
        layout(screen);
    }

    private static void layout(StreamSettings screen) {
        View root = screen.findViewById(android.R.id.content);
        int width = screen.getResources().getDisplayMetrics().widthPixels;
        int height = screen.getResources().getDisplayMetrics().heightPixels;
        root.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, width, height);
    }

    private static ActivityController<StreamSettings> open() {
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup().visible();
        settle(controller.get());
        return controller;
    }

    private static ListView sections(StreamSettings screen) {
        return (ListView) screen.getFragmentManager().findFragmentById(R.id.settings_sections).getView()
                .findViewById(android.R.id.list);
    }

    private static ListView content(StreamSettings screen) {
        return (ListView) screen.getFragmentManager().findFragmentById(R.id.stream_settings).getView()
                .findViewById(android.R.id.list);
    }

    private static String shownSection(StreamSettings screen) {
        android.app.Fragment fragment = screen.getFragmentManager().findFragmentById(R.id.stream_settings);
        assertTrue(fragment instanceof StreamSettings.SettingsFragment);
        return fragment.getArguments().getString("section");
    }

    private static int row(ListView list, String key) {
        int position = StreamSettings.RootFragment.positionOf(list, key);
        assertTrue("no row " + key, position >= 0);
        return position;
    }

    private static void key(android.app.Activity target, int keyCode) {
        target.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, keyCode));
        target.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, keyCode));
    }

    private static boolean within(View container, View view) {
        for (android.view.ViewParent p = view == null ? null : view.getParent(); p != null; p = p.getParent()) {
            if (p == container) return true;
        }
        return false;
    }

    @Test
    public void aPhoneKeepsOnePane() {
        ActivityController<StreamSettings> controller = open();
        try {
            assertNull(controller.get().findViewById(R.id.settings_sections));
            assertTrue(controller.get().getFragmentManager().findFragmentById(R.id.stream_settings)
                    instanceof StreamSettings.RootFragment);
            assertNull("no search row without a D-pad or controller",
                    findRow(controller.get(), StreamSettings.SEARCH_ROW_KEY));
        } finally { controller.pause().stop().destroy(); }
    }

    private static Preference findRow(StreamSettings screen, String key) {
        ListView list = (ListView) screen.getFragmentManager().findFragmentById(R.id.stream_settings).getView()
                .findViewById(android.R.id.list);
        int position = StreamSettings.RootFragment.positionOf(list, key);
        return position < 0 ? null : (Preference) list.getItemAtPosition(position);
    }

    @Test @Config(qualifiers = WIDE)
    public void aWideScreenShowsTheSectionsAndTheFirstSection() {
        ActivityController<StreamSettings> controller = open();
        try {
            StreamSettings screen = controller.get();
            assertTrue(screen.getFragmentManager().findFragmentById(R.id.settings_sections)
                    instanceof StreamSettings.RootFragment);
            assertEquals("video", shownSection(screen));
            assertEquals(screen.getString(R.string.settings),
                    ((com.google.android.material.appbar.MaterialToolbar) screen.findViewById(R.id.settings_toolbar))
                            .getTitle().toString());
            assertEquals(View.VISIBLE, screen.findViewById(R.id.shell_bar).getVisibility());
            ListView list = sections(screen);
            assertTrue("the section on the right is marked on the left", list.isItemChecked(row(list, "video")));
        } finally { controller.pause().stop().destroy(); }
    }

    @Test @Config(qualifiers = TV)
    public void aTelevisionShowsControlsFirstAndFocusesTheSections() {
        ActivityController<StreamSettings> controller = open();
        try {
            StreamSettings screen = controller.get();
            assertEquals("input", shownSection(screen));
            ListView list = sections(screen);
            assertTrue("first focus is on content, not the toolbar", list.hasFocus());
            assertFalse(content(screen).hasFocus());
        } finally { controller.pause().stop().destroy(); }
    }

    @Test @Config(qualifiers = WIDE)
    public void focusOnASectionRowShowsItOnTheRight() {
        ActivityController<StreamSettings> controller = open();
        try {
            StreamSettings screen = controller.get();
            ListView list = sections(screen);
            assertTrue(list.requestFocusFromTouch());
            list.setSelection(row(list, "audio"));
            settle(screen);
            assertEquals("audio", shownSection(screen));
            assertTrue(list.hasFocus());
            assertTrue(list.isItemChecked(row(list, "audio")));
            list.setSelection(row(list, "advanced"));
            settle(screen);
            assertEquals("advanced", shownSection(screen));
        } finally { controller.pause().stop().destroy(); }
    }

    @Test @Config(qualifiers = WIDE)
    public void aTouchOnASectionRowShowsItWithoutMovingFocus() {
        ActivityController<StreamSettings> controller = open();
        try {
            StreamSettings screen = controller.get();
            ListView list = sections(screen);
            Preference streaming = (Preference) list.getItemAtPosition(row(list, "streaming"));
            assertTrue(streaming.getOnPreferenceClickListener().onPreferenceClick(streaming));
            settle(screen);
            assertEquals("streaming", shownSection(screen));
            assertFalse(content(screen).hasFocus());
        } finally { controller.pause().stop().destroy(); }
    }

    @Test @Config(qualifiers = TV)
    public void centerEntersThePaneAndLeftReturnsToTheSection() {
        ActivityController<StreamSettings> controller = open();
        try {
            StreamSettings screen = controller.get();
            ListView list = sections(screen);
            list.setSelection(row(list, "audio"));
            settle(screen);
            assertEquals("audio", shownSection(screen));
            key(screen, KeyEvent.KEYCODE_DPAD_CENTER);
            settle(screen);
            assertTrue("Center enters the pane", content(screen).hasFocus());
            assertFalse(sections(screen).hasFocus());
            key(screen, KeyEvent.KEYCODE_DPAD_LEFT);
            settle(screen);
            assertTrue("Left returns to the list", sections(screen).hasFocus());
            assertEquals("audio", ((Preference) sections(screen).getSelectedItem()).getKey());
            assertEquals("the pane did not change", "audio", shownSection(screen));
        } finally { controller.pause().stop().destroy(); }
    }

    @Test @Config(qualifiers = TV)
    public void backInTheContentPaneReturnsToTheListBeforeLeaving() {
        ActivityController<StreamSettings> controller = open();
        try {
            StreamSettings screen = controller.get();
            ListView list = sections(screen);
            list.setSelection(row(list, "audio"));
            settle(screen);
            key(screen, KeyEvent.KEYCODE_DPAD_CENTER);
            settle(screen);
            assertTrue(content(screen).hasFocus());
            screen.getOnBackPressedDispatcher().onBackPressed();
            settle(screen);
            assertTrue(sections(screen).hasFocus());
            assertFalse(screen.isFinishing());
            screen.getOnBackPressedDispatcher().onBackPressed();
            settle(screen);
            assertTrue(screen.isFinishing());
        } finally { controller.pause().stop().destroy(); }
    }

    @Test @Config(qualifiers = WIDE)
    public void theSearchRowOpensSearchAndResultsFillTheRightPane() {
        ActivityController<StreamSettings> controller = open();
        try {
            StreamSettings screen = controller.get();
            ListView list = sections(screen);
            assertEquals("Search settings is the first row", StreamSettings.SEARCH_ROW_KEY,
                    ((Preference) list.getItemAtPosition(0)).getKey());
            Preference search = (Preference) list.getItemAtPosition(0);
            assertEquals(screen.getString(R.string.settings_search_hint), search.getTitle().toString());
            assertTrue(search.getOnPreferenceClickListener().onPreferenceClick(search));
            SearchView view = screen.findViewById(R.id.settings_search);
            assertEquals(View.VISIBLE, view.getVisibility());
            view.setQuery("bitrate", false);
            settle(screen);
            android.app.Fragment results = screen.getFragmentManager().findFragmentById(R.id.stream_settings);
            assertTrue(results instanceof StreamSettings.SettingsFragment);
            assertTrue("the list of sections stays", screen.getFragmentManager()
                    .findFragmentById(R.id.settings_sections) instanceof StreamSettings.RootFragment);
            // A result opens its setting in its section and Back returns to the results.
            android.preference.PreferenceScreen results0 = ((StreamSettings.SettingsFragment) results).getPreferenceScreen();
            Preference result = firstLeaf(results0);
            assertNotNull(result);
            result.getOnPreferenceClickListener().onPreferenceClick(result);
            settle(screen);
            assertEquals("video", shownSection(screen));
            assertEquals("", view.getQuery().toString());
        } finally { controller.pause().stop().destroy(); }
    }

    @Test @Config(qualifiers = WIDE)
    public void pickingASectionEndsTheSearch() {
        ActivityController<StreamSettings> controller = open();
        try {
            StreamSettings screen = controller.get();
            SearchView view = screen.findViewById(R.id.settings_search);
            screen.openSearch();
            view.setQuery("bitrate", false);
            settle(screen);
            ListView list = sections(screen);
            Preference audio = (Preference) list.getItemAtPosition(row(list, "audio"));
            audio.getOnPreferenceClickListener().onPreferenceClick(audio);
            settle(screen);
            assertEquals("", view.getQuery().toString());
            assertEquals("audio", shownSection(screen));
        } finally { controller.pause().stop().destroy(); }
    }

    @Test @Config(qualifiers = WIDE)
    public void aDeepLinkOpensItsSectionWithTheSettingSelectedAndFocused() {
        android.content.Intent intent = StreamSettings.intent(
                org.robolectric.RuntimeEnvironment.getApplication(), "audio", "list_audio_config");
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class, intent).setup().visible();
        try {
            StreamSettings screen = controller.get();
            settle(screen);
            settle(screen);
            assertEquals("audio", shownSection(screen));
            ListView list = content(screen);
            assertEquals("list_audio_config", ((Preference) list.getSelectedItem()).getKey());
            assertTrue(list.hasFocus());
        } finally { controller.pause().stop().destroy(); }
    }

    @Test
    public void aPhoneSectionKeepsItsSectionAndGainsTheListWhenTheScreenTurnsWide() {
        ActivityController<StreamSettings> phone = open();
        android.os.Bundle state = new android.os.Bundle();
        try {
            StreamSettings screen = phone.get();
            screen.showSection("advanced");
            settle(screen);
            phone.saveInstanceState(state);
        } finally { phone.pause().stop().destroy(); }
        org.robolectric.RuntimeEnvironment.setQualifiers("+" + WIDE);
        ActivityController<StreamSettings> wide = Robolectric.buildActivity(StreamSettings.class).setup(state).visible();
        try {
            StreamSettings screen = wide.get();
            settle(screen);
            assertEquals("advanced", shownSection(screen));
            assertNotNull("the list of sections is back", screen.getFragmentManager().findFragmentById(R.id.settings_sections));
            ListView list = sections(screen);
            assertTrue(list.isItemChecked(row(list, "advanced")));
        } finally { wide.pause().stop().destroy(); }
    }

    private static Preference firstLeaf(android.preference.PreferenceGroup group) {
        for (int i = 0; i < group.getPreferenceCount(); i++) {
            Preference preference = group.getPreference(i);
            if (preference instanceof android.preference.PreferenceGroup) {
                Preference found = firstLeaf((android.preference.PreferenceGroup) preference);
                if (found != null) return found;
            } else if (preference.getOnPreferenceClickListener() != null) {
                return preference;
            }
        }
        return null;
    }
}
