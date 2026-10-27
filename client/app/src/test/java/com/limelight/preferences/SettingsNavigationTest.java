package com.limelight.preferences;

import android.app.Application;
import android.preference.Preference;
import android.preference.PreferenceGroup;
import androidx.appcompat.widget.SearchView;
import com.limelight.R;
import java.util.concurrent.TimeUnit;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SettingsNavigationTest {
    @Test
    public void preferenceHelpKeysSurviveTheFrameworksInitialListBinding() {
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup().visible();
        try {
            StreamSettings screen = controller.get();
            settle(screen);
            android.view.View root = screen.findViewById(android.R.id.content);
            root.measure(android.view.View.MeasureSpec.makeMeasureSpec(1080, android.view.View.MeasureSpec.EXACTLY),
                    android.view.View.MeasureSpec.makeMeasureSpec(2400, android.view.View.MeasureSpec.EXACTLY));
            root.layout(0, 0, 1080, 2400);
            root.getViewTreeObserver().dispatchOnGlobalLayout();
            android.widget.ListView list = screen.findViewById(android.R.id.list);
            list.requestFocusFromTouch();
            list.setSelection(0);
            root.measure(android.view.View.MeasureSpec.makeMeasureSpec(1080, android.view.View.MeasureSpec.EXACTLY),
                    android.view.View.MeasureSpec.makeMeasureSpec(2400, android.view.View.MeasureSpec.EXACTLY));
            root.layout(0, 0, 1080, 2400);
            assertTrue(list.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN,
                    android.view.KeyEvent.KEYCODE_DPAD_RIGHT)));
            assertTrue(list.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_UP,
                    android.view.KeyEvent.KEYCODE_DPAD_RIGHT)));
            androidx.appcompat.app.AlertDialog help = (androidx.appcompat.app.AlertDialog)
                    org.robolectric.shadows.ShadowDialog.getLatestDialog();
            assertNotNull(help);
            assertEquals(screen.getString(R.string.stream_presets_help),
                    ((android.widget.TextView) help.findViewById(android.R.id.message)).getText().toString());
            help.dismiss();
        } finally { controller.pause().stop().destroy(); }
    }

    @Test
    public void presetChildrenCannotKeepFocusWhenAnotherSectionIsSelected() {
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup().visible();
        try {
            StreamSettings screen = controller.get();
            settle(screen);
            android.view.View root = screen.findViewById(android.R.id.content);
            root.measure(android.view.View.MeasureSpec.makeMeasureSpec(1080, android.view.View.MeasureSpec.EXACTLY),
                    android.view.View.MeasureSpec.makeMeasureSpec(2400, android.view.View.MeasureSpec.EXACTLY));
            root.layout(0, 0, 1080, 2400);
            android.widget.ListView list = screen.findViewById(android.R.id.list);
            android.view.View preset = list.getChildAt(0);
            assertFalse("Preset chips/help must not steal focus from the preference list", preset.hasFocusable());
            assertNotNull("Preset help stays available with D-pad right", preset.getTag(R.id.preference_help));
            assertTrue(list.requestFocusFromTouch());
            list.setSelection(3); // Controls after Presets, Video, Display.
            root.layout(0, 0, 1080, 2400);
            assertFalse(screen.findViewById(R.id.preset_help).hasFocus());
            assertFalse(preset.findViewById(R.id.preset_help).requestFocus());
            assertEquals("input", ((Preference) list.getSelectedItem()).getKey());
        } finally { controller.pause().stop().destroy(); }
    }

    @Test
    public void openNamesTheSectionAndTheSettingToLandOn() {
        android.content.Context context = org.robolectric.RuntimeEnvironment.getApplication();
        android.content.Intent both = StreamSettings.intent(context, "audio", "list_audio_config");
        assertEquals(StreamSettings.class.getName(), both.getComponent().getClassName());
        assertEquals("audio", both.getStringExtra(StreamSettings.EXTRA_SECTION));
        assertEquals("list_audio_config", both.getStringExtra(StreamSettings.EXTRA_FOCUS_KEY));
        // The section follows from the key when the caller only knows the setting.
        assertEquals("video", StreamSettings.intent(context, null, "checkbox_enable_hdr").getStringExtra(StreamSettings.EXTRA_SECTION));
        assertEquals("audio", StreamSettings.intent(context, null, "list_audio_config").getStringExtra(StreamSettings.EXTRA_SECTION));
        android.content.Intent plain = StreamSettings.intent(context, null, null);
        assertFalse(plain.hasExtra(StreamSettings.EXTRA_SECTION));
        assertFalse(plain.hasExtra(StreamSettings.EXTRA_FOCUS_KEY));
        assertNull(StreamSettings.sectionOfKey(context, "no_such_setting"));
    }

    @Test
    public void openStartsSettingsAndDeepLinkScrollsToTheSetting() {
        android.content.Context context = org.robolectric.RuntimeEnvironment.getApplication();
        StreamSettings.open(context, "audio", "list_audio_config");
        android.content.Intent started = org.robolectric.Shadows.shadowOf((Application) context).getNextStartedActivity();
        assertEquals("audio", started.getStringExtra(StreamSettings.EXTRA_SECTION));
        assertTrue("from a non-activity context", (started.getFlags() & android.content.Intent.FLAG_ACTIVITY_NEW_TASK) != 0);
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class, started).setup().visible();
        try {
            StreamSettings screen = controller.get();
            settle(screen);
            settle(screen);
            StreamSettings.SettingsFragment fragment = (StreamSettings.SettingsFragment)
                    screen.getFragmentManager().findFragmentById(R.id.stream_settings);
            assertEquals("audio", fragment.getArguments().getString("section"));
            assertEquals(screen.getString(R.string.settings_section_audio),
                    ((com.google.android.material.appbar.MaterialToolbar) screen.findViewById(R.id.settings_toolbar)).getTitle().toString());
            android.widget.ListView list = (android.widget.ListView) fragment.getView().findViewById(android.R.id.list);
            assertEquals("list_audio_config", ((Preference) list.getSelectedItem()).getKey());
        } finally { controller.pause().stop().destroy(); }
    }

    @Test
    public void anUnknownSectionWithAnUnknownSettingOpensTheList() {
        android.content.Intent intent = new android.content.Intent(
                (android.content.Context) org.robolectric.RuntimeEnvironment.getApplication(), StreamSettings.class)
                .putExtra(StreamSettings.EXTRA_SECTION, "nope").putExtra(StreamSettings.EXTRA_FOCUS_KEY, "no_such_setting");
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class, intent).setup().visible();
        try {
            settle(controller.get());
            assertTrue(controller.get().getFragmentManager().findFragmentById(R.id.stream_settings)
                    instanceof StreamSettings.RootFragment);
        } finally { controller.pause().stop().destroy(); }
    }

    @Test
    public void anUnknownSectionWithAKnownSettingOpensTheSectionOfTheSetting() {
        android.content.Intent intent = new android.content.Intent(
                (android.content.Context) org.robolectric.RuntimeEnvironment.getApplication(), StreamSettings.class)
                .putExtra(StreamSettings.EXTRA_SECTION, "nope").putExtra(StreamSettings.EXTRA_FOCUS_KEY, "list_audio_config");
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class, intent).setup().visible();
        try {
            settle(controller.get());
            android.app.Fragment shown = controller.get().getFragmentManager().findFragmentById(R.id.stream_settings);
            assertTrue(shown instanceof StreamSettings.SettingsFragment);
            assertEquals("audio", shown.getArguments().getString("section"));
        } finally { controller.pause().stop().destroy(); }
        assertEquals("audio", StreamSettings.intent(org.robolectric.RuntimeEnvironment.getApplication(), "nope", "list_audio_config")
                .getStringExtra(StreamSettings.EXTRA_SECTION));
    }

    private static void settle(StreamSettings screen) {
        ShadowLooper.idleMainLooper(150, TimeUnit.MILLISECONDS);
        screen.getFragmentManager().executePendingTransactions();
    }

    @Test
    public void returningFromAResultRestoresSearchAndThenItsOrigin() {
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup();
        try {
            StreamSettings screen = controller.get();
            screen.showSection("input");
            settle(screen);
            SearchView search = screen.findViewById(R.id.settings_search);
            search.setQuery("bitrate", false);
            settle(screen);
            StreamSettings.SettingsFragment fragment = (StreamSettings.SettingsFragment)
                    screen.getFragmentManager().findFragmentById(R.id.stream_settings);
            Preference result = firstResult(fragment.getPreferenceScreen());
            assertNotNull(result);
            result.getOnPreferenceClickListener().onPreferenceClick(result);
            settle(screen);
            assertEquals("", search.getQuery().toString());
            assertEquals(screen.getString(R.string.settings_section_video),
                    ((com.google.android.material.appbar.MaterialToolbar) screen.findViewById(R.id.settings_toolbar)).getTitle().toString());
            screen.getOnBackPressedDispatcher().onBackPressed();
            settle(screen);
            assertEquals("bitrate", search.getQuery().toString());
            screen.getOnBackPressedDispatcher().onBackPressed();
            settle(screen);
            assertEquals("", search.getQuery().toString());
            assertEquals(screen.getString(R.string.settings_section_input),
                    ((com.google.android.material.appbar.MaterialToolbar) screen.findViewById(R.id.settings_toolbar)).getTitle().toString());
            assertFalse(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void lastKeystrokeSurvivesRecreationWithoutReplacingRestoredFragmentOnAttach() {
        ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup();
        try {
            SearchView search = controller.get().findViewById(R.id.settings_search);
            search.setQuery("bitrate", false);
            // Deliberately recreate before the debounced search runs.
            controller.recreate();
            StreamSettings screen = controller.get();
            settle(screen);
            assertEquals("bitrate", ((SearchView) screen.findViewById(R.id.settings_search)).getQuery().toString());
            android.app.Fragment restored = screen.getFragmentManager().findFragmentById(R.id.stream_settings);
            assertTrue(restored instanceof StreamSettings.SettingsFragment);
            screen.onAttachedToWindow();
            settle(screen);
            assertSame(restored, screen.getFragmentManager().findFragmentById(R.id.stream_settings));
            assertNotNull(firstResult(((StreamSettings.SettingsFragment) restored).getPreferenceScreen()));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static Preference firstResult(PreferenceGroup group) {
        for (int i = 0; i < group.getPreferenceCount(); i++) {
            Preference preference = group.getPreference(i);
            if (preference instanceof PreferenceGroup) {
                Preference found = firstResult((PreferenceGroup) preference);
                if (found != null) return found;
            } else if (("search_result_" + PreferenceConfiguration.BITRATE_PREF_STRING).equals(preference.getKey())) return preference;
        }
        return null;
    }
}
