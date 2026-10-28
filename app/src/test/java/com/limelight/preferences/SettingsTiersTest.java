package com.limelight.preferences;

import android.app.Application;
import android.content.Context;
import android.content.pm.PackageManager;
import android.content.res.XmlResourceParser;
import android.preference.Preference;
import android.preference.PreferenceGroup;
import android.preference.PreferenceManager;
import android.preference.PreferenceScreen;

import com.limelight.R;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.xmlpull.v1.XmlPullParser;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import static com.limelight.preferences.SettingsStructureTest.search;
import static com.limelight.preferences.SettingsStructureTest.settle;
import static com.limelight.preferences.SettingsStructureTest.shown;
import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/** Each section opens on its main rows (settings_tiers.xml); its More row unfolds the rest in place. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, qualifiers = "en", application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SettingsTiersTest {
    private static final String ANDROID = "http://schemas.android.com/apk/res/android";

    private static Context context() {
        return RuntimeEnvironment.getApplication();
    }

    @After
    public void clear() {
        PreferenceManager.getDefaultSharedPreferences(context()).edit().clear().commit();
    }

    /** preferences.xml: each section's keys in order, with the key each row depends on. */
    private static Map<String, List<String>> sectionKeys(Map<String, String> dependencies) throws Exception {
        Map<String, List<String>> sections = new LinkedHashMap<>();
        String section = null;
        try (XmlResourceParser parser = context().getResources().getXml(R.xml.preferences)) {
            for (int event = parser.getEventType(); event != XmlPullParser.END_DOCUMENT; event = parser.next()) {
                if (event != XmlPullParser.START_TAG) continue;
                String key = parser.getAttributeValue(ANDROID, "key");
                if (key == null) continue;
                if (parser.getName().equals("PreferenceCategory")) {
                    section = StreamSettings.sectionOf(key);
                    assertNotNull(key + " belongs to a section", section);
                    continue;
                }
                sections.computeIfAbsent(section, s -> new ArrayList<>()).add(key);
                String dependency = parser.getAttributeValue(ANDROID, "dependency");
                if (dependency != null) dependencies.put(key, dependency);
            }
        }
        return sections;
    }

    /** The keys of the rows a section's list shows now, More row included. */
    private static List<String> visible(StreamSettings.SettingsFragment fragment) {
        List<String> keys = new ArrayList<>();
        PreferenceScreen screen = fragment.getPreferenceScreen();
        for (int i = 0; i < screen.getPreferenceCount(); i++) {
            PreferenceGroup group = (PreferenceGroup) screen.getPreference(i);
            assertTrue(group.getKey() + " shows with no row", group.getPreferenceCount() > 0);
            keys.addAll(SettingsStructureTest.keys(group));
        }
        return keys;
    }

    private static StreamSettings.SettingsFragment openFolded(StreamSettings screen, String section) {
        screen.moreOpen.remove(section);
        screen.showSection(section);
        settle(screen);
        return (StreamSettings.SettingsFragment) shown(screen);
    }

    private static void click(Preference preference) {
        preference.getOnPreferenceClickListener().onPreferenceClick(preference);
    }

    @Test
    public void everyMainKeyIsARowOfItsSectionAndNoSectionHasMoreThanEight() throws Exception {
        Map<String, List<String>> sections = sectionKeys(new LinkedHashMap<>());
        for (String section : StreamSettings.SECTIONS.keySet()) {
            List<String> main = StreamSettings.mainKeys(context(), section);
            assertTrue(section + " has at most 8 main rows", main.size() <= 8);
            assertFalse(section + " has main rows", main.isEmpty());
            assertEquals(section + " lists a key twice", main.size(), new HashSet<>(main).size());
            for (String key : main) {
                assertTrue(key + " is a row of " + section, sections.get(section).contains(key));
            }
            assertTrue("the More row ends " + section, sections.get(section).get(sections.get(section).size() - 1)
                    .equals(StreamSettings.MORE_PREFIX + section));
        }
    }

    @Test
    public void aMainRowNeverDependsOnAFoldedOne() throws Exception {
        Map<String, String> dependencies = new LinkedHashMap<>();
        Map<String, List<String>> sections = sectionKeys(dependencies);
        for (Map.Entry<String, List<String>> section : sections.entrySet()) {
            for (String key : section.getValue()) {
                String dependency = dependencies.get(key);
                if (dependency == null || StreamSettings.foldsInSection(context(), section.getKey(), key)) continue;
                assertFalse(key + " shows while " + dependency + " folds",
                        StreamSettings.foldsInSection(context(), section.getKey(), dependency));
            }
        }
    }

    @Test
    public void eachSectionOpensOnItsMainRowsAndItsMoreRowShowsEveryOtherInPlace() throws Exception {
        shadowOf(context().getPackageManager()).setSystemFeature(PackageManager.FEATURE_TOUCHSCREEN, true);
        Map<String, List<String>> sections = sectionKeys(new LinkedHashMap<>());
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            for (String section : StreamSettings.SECTIONS.keySet()) {
                String moreKey = StreamSettings.MORE_PREFIX + section;
                StreamSettings.SettingsFragment fragment = openFolded(screen, section);
                List<String> folded = visible(fragment);
                Preference more = fragment.findPreference(moreKey);
                assertEquals(section + " ends on its More row", moreKey, folded.get(folded.size() - 1));
                assertTrue("a D-pad reaches the More row", more.isSelectable() && more.isEnabled());
                String closedTitle = String.valueOf(more.getTitle());
                for (String key : folded) {
                    assertFalse(key + " folds in " + section, StreamSettings.foldsInSection(context(), section, key));
                }

                click(more);
                settle(screen);
                List<String> open = visible(fragment);
                // Nothing is lost: every row of the section is main or behind the More row (or behind Customize, or
                // does not apply on this device or with these values).
                List<String> unfolded = new ArrayList<>(open);
                unfolded.removeAll(folded);
                assertFalse(section + " unfolds rows", unfolded.isEmpty());
                for (String key : unfolded) {
                    assertTrue(key + " was folded in " + section, StreamSettings.foldsInSection(context(), section, key));
                }
                String title = screen.getString(StreamSettings.sectionTitle(section));
                assertEquals(screen.getString(R.string.settings_more_count, title, unfolded.size()), closedTitle);
                assertEquals(screen.getString(R.string.settings_fewer, title), String.valueOf(more.getTitle()));
                // In place: the rows keep the order of preferences.xml.
                List<String> order = new ArrayList<>(sections.get(section));
                order.retainAll(open);
                assertEquals(section + " keeps its order", order, open);

                click(more);
                settle(screen);
                assertEquals(section + " folds again", folded, visible(fragment));
            }
        }
    }

    @Test
    public void theMoreRowCountsTheRowsItHolds() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment fragment = openFolded(screen, "audio");
            Preference more = fragment.findPreference("more_audio");
            String closed = String.valueOf(more.getTitle());
            int before = visible(fragment).size();
            click(more);
            settle(screen);
            int shownRows = visible(fragment).size() - before;
            assertEquals(screen.getString(R.string.settings_more_count, screen.getString(R.string.settings_section_audio),
                    shownRows), closed);
            assertEquals("More Audio settings (" + shownRows + ")", closed);
        }
    }

    @Test
    public void searchFindsAFoldedRowAndOpeningItUnfoldsItsSection() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            String key = "list_network_grace";
            assertTrue(StreamSettings.foldsInSection(screen, "streaming", key));
            assertFalse(visible(openFolded(screen, "streaming")).contains(key));

            StreamSettings.SettingsFragment results = search(screen, "Ride out network drops");
            Preference result = results.getPreferenceScreen().findPreference("search_result_" + key);
            assertNotNull("search finds the folded row", result);
            assertNull("search lists no More row", results.getPreferenceScreen().findPreference("search_result_more_streaming"));
            click(result);
            settle(screen);
            StreamSettings.SettingsFragment section = (StreamSettings.SettingsFragment) shown(screen);
            assertTrue("the result's row shows", visible(section).contains(key));
            assertEquals(screen.getString(R.string.settings_fewer, screen.getString(R.string.settings_section_streaming)),
                    String.valueOf(section.findPreference("more_streaming").getTitle()));
        }
    }

    @Test
    public void aLinkToAFoldedRowOpensItsSectionUnfolded() {
        android.content.Intent link = StreamSettings.intent(context(), null, "checkbox_tell_host_wireless");
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class, link).setup()) {
            StreamSettings screen = controller.get();
            settle(screen);
            StreamSettings.SettingsFragment section = (StreamSettings.SettingsFragment) shown(screen);
            assertTrue(visible(section).contains("checkbox_tell_host_wireless"));
            assertTrue(screen.moreOpen.contains("advanced"));
        }
    }

    @Test
    public void theFoldSurvivesARecreate() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment fragment = openFolded(screen, "audio");
            click(fragment.findPreference("more_audio"));
            controller.recreate();
            StreamSettings again = controller.get();
            settle(again);
            assertTrue(again.moreOpen.contains("audio"));
        }
    }

    @Test
    public void tellThePcAndTheLinkTestSitInAdvancedDiagnostics() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            for (String key : new String[] {"checkbox_tell_host_wireless", "checkbox_tell_host_link_speed",
                    "checkbox_link_probe"}) {
                assertEquals(key, "advanced", StreamSettings.sectionOfKey(screen, key));
            }
            screen.moreOpen.add("advanced");
            screen.showSection("advanced");
            settle(screen);
            PreferenceGroup diagnostics = (PreferenceGroup) ((StreamSettings.SettingsFragment) shown(screen))
                    .getPreferenceScreen().findPreference("category_advanced_diagnostics");
            assertEquals(java.util.Arrays.asList("checkbox_tell_host_wireless", "checkbox_tell_host_link_speed",
                    "checkbox_link_probe"), SettingsStructureTest.keys(diagnostics).subList(0, 3));
            assertEquals("Link test before the first bitrate", String.valueOf(diagnostics.findPreference("checkbox_link_probe").getTitle()));
            assertEquals("USB Switch Pro driver (Experimental)", screen.getString(R.string.title_usb_switch_pro));
        }
    }

    @Test
    public void automaticBitrateSaysThePcAdaptsWhileItIsOff() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            String line = "Off: the PC adapts the bitrate (recommended)";
            StreamSettings.SettingsFragment video = openFolded(screen, "video");
            android.preference.SwitchPreference auto =
                    (android.preference.SwitchPreference) video.findPreference("checkbox_auto_bitrate");
            assertFalse(auto.isChecked());
            // The framework shows android:summaryOff only in the bound row; getSummary(), which search results copy,
            // stayed null.
            assertEquals(line, String.valueOf(auto.getSummary()));
            assertEquals(line, rowSummary(video, "checkbox_auto_bitrate"));
            auto.setChecked(true);
            assertNull("on, the switch has no short line", auto.getSummary());
            assertNull(rowSummary(video, "checkbox_auto_bitrate"));
            auto.setChecked(false);

            Preference result = search(screen, "Device controls bitrate").getPreferenceScreen()
                    .findPreference("search_result_checkbox_auto_bitrate");
            assertNotNull(result);
            assertEquals(line, String.valueOf(result.getSummary()));
        }
    }

    /** The summary line a row of the section's list shows, or null when it shows none. */
    private static String rowSummary(StreamSettings.SettingsFragment fragment, String key) {
        android.widget.ListView list = fragment.getView().findViewById(android.R.id.list);
        int position = StreamSettings.RootFragment.positionOf(list, key);
        assertTrue(key + " is listed", position >= 0);
        android.view.View row = list.getAdapter().getView(position, null, list);
        android.widget.TextView summary = row.findViewById(android.R.id.summary);
        return summary == null || summary.getVisibility() != android.view.View.VISIBLE ||
                summary.getText().length() == 0 ? null : summary.getText().toString();
    }

    @Test
    public void germanLabelsSayExperimentellAndTheLinkTestHasNone() {
        Context german = context().createConfigurationContext(configuration("de"));
        assertEquals("Switch Pro per USB (Experimentell)", german.getString(R.string.title_usb_switch_pro));
        assertFalse(german.getString(R.string.title_link_probe).contains("xperiment"));
        for (int id : new int[] {R.string.title_client_roaming, R.string.title_usb_forwarding, R.string.files_menu}) {
            assertTrue(german.getString(id), german.getString(id).endsWith("(Experimentell)"));
        }
    }

    private static android.content.res.Configuration configuration(String language) {
        android.content.res.Configuration configuration = new android.content.res.Configuration(
                context().getResources().getConfiguration());
        configuration.setLocale(new java.util.Locale(language));
        return configuration;
    }

    @Test
    public void searchFindsAFoldedRowOfVideo() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment results = search(screen, screen.getString(R.string.title_enable_sdr10));
            assertNotNull(results.getPreferenceScreen().findPreference("search_result_checkbox_enable_sdr_10bit"));
        }
    }
}
