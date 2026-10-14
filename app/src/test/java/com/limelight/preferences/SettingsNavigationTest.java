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
                    ((android.widget.TextView) screen.findViewById(R.id.settings_title)).getText().toString());
            screen.getOnBackPressedDispatcher().onBackPressed();
            settle(screen);
            assertEquals("bitrate", search.getQuery().toString());
            screen.getOnBackPressedDispatcher().onBackPressed();
            settle(screen);
            assertEquals("", search.getQuery().toString());
            assertEquals(screen.getString(R.string.settings_section_input),
                    ((android.widget.TextView) screen.findViewById(R.id.settings_title)).getText().toString());
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
