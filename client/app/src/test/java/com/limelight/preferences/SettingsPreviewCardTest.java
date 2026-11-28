package com.limelight.preferences;

import android.app.Application;
import android.app.Dialog;
import android.content.SharedPreferences;
import android.preference.ListPreference;
import android.preference.Preference;
import android.preference.PreferenceManager;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ListView;
import android.widget.TextView;

import com.limelight.R;
import com.limelight.ui.Motion;
import com.limelight.ui.VideoScaling;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

/** The preview cards in Settings: which rows carry one, what it shows, how its picker picks and that search finds it. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class SettingsPreviewCardTest {
    @Before
    public void openMoreSettings() {
        StreamSettings.rememberRail(RuntimeEnvironment.getApplication(), true);
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
    }

    private static SettingsPreviewCard card(View row) {
        return row.findViewById(R.id.settings_preview_card);
    }

    private static void collect(View view, List<SettingsPreviewCard> cards) {
        if (view instanceof SettingsPreviewCard) {
            cards.add((SettingsPreviewCard) view);
        } else if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) collect(group.getChildAt(i), cards);
        }
    }

    private static List<SettingsPreviewCard> cards(Dialog picker) {
        List<SettingsPreviewCard> cards = new ArrayList<>();
        collect(picker.findViewById(R.id.settings_preview_grid), cards);
        return cards;
    }

    @Test
    public void onlyTheFourSettingsCarryAPicture() {
        assertEquals(SettingsPreviewCard.PACING, SettingsPreviews.kindOf("frame_pacing"));
        assertEquals(SettingsPreviewCard.TONE, SettingsPreviews.kindOf("checkbox_enable_hdr"));
        assertEquals(SettingsPreviewCard.EDGE, SettingsPreviews.kindOf("upscaling_mode"));
        assertEquals(SettingsPreviewCard.SCALING, SettingsPreviews.kindOf(VideoScaling.PREF));
        assertFalse(SettingsPreviews.has("upscaling_sharpness"));
        assertFalse(SettingsPreviews.has("search_result_frame_pacing"));
        assertFalse(SettingsPreviews.has(null));
    }

    @Test
    public void aRowShowsThePictureOfItsChoiceUnderItsTitleAndSaysItInWords() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment display = SettingsStructureTest.open(screen, "display");
            ListPreference pacing = (ListPreference) display.findPreference("frame_pacing");
            assertEquals(R.layout.settings_preference_preview, pacing.getLayoutResource());
            ListView list = new ListView(screen);
            View row = pacing.getView(null, list);
            SettingsPreviewCard card = card(row);
            assertEquals(SettingsPreviewCard.PACING, card.kind());
            assertEquals(PreviewPacing.LATENCY, card.state(), 0f);
            // A group's description stands for the text inside it, so it names the value before the help.
            assertEquals("the picture is said as the value and the setting's own help",
                    pacing.getEntry() + ". " + pacing.getDialogMessage(), card.getContentDescription().toString());
            TextView value = row.findViewById(android.R.id.summary);
            assertSame("the value is the card's caption", card, value.getParent());
            assertEquals(pacing.getEntry().toString(), value.getText().toString());
            assertEquals(View.VISIBLE, row.findViewById(R.id.settings_preview_chevron).getVisibility());

            pacing.setValue("smoothness");
            assertEquals(PreviewPacing.SMOOTHNESS, card(pacing.getView(null, list)).state(), 0f);

            Preference size = display.findPreference(VideoScaling.PREF);
            assertEquals(R.layout.settings_preference_preview, size.getLayoutResource());
            card = card(size.getView(null, list));
            assertEquals(SettingsPreviewCard.SCALING, card.kind());
            assertEquals(VideoScaling.FIT, card.state(), 0f);
            assertEquals(screen.getResources().getStringArray(R.array.video_scaling_modes)[VideoScaling.FIT] + ". "
                    + screen.getString(R.string.video_scaling_help), card.getContentDescription().toString());
        }
    }

    @Test
    public void aSwitchKeepsItsSwitchOnTheTitleLineAndThePictureFollowsIt() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            MaterialSwitchPreference hdr = new MaterialSwitchPreference(screen, null);
            hdr.setKey(SettingsPreviews.HDR);
            hdr.setTitle(R.string.title_enable_hdr);
            hdr.setLayoutResource(R.layout.settings_preference_preview);
            ListView list = new ListView(screen);
            View row = hdr.getView(null, list);
            assertEquals(SettingsPreviewCard.TONE, card(row).kind());
            assertEquals(PreviewTone.SDR, card(row).state(), 0f);
            assertNotNull(row.findViewById(R.id.preference_switch));
            assertEquals("a switch has no chevron", View.GONE, row.findViewById(R.id.settings_preview_chevron).getVisibility());
            hdr.setChecked(true);
            assertEquals(PreviewTone.HDR, card(hdr.getView(null, list)).state(), 0f);
        }
    }

    @Test
    public void aChangedChoiceSpringsToItsPictureAndCutsWithReducedMotion() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            prefs().edit().putString(Motion.REDUCE_MOTION, Motion.FULL).commit();
            SettingsPreviewCard card = new SettingsPreviewCard(screen, null);
            card.show("frame_pacing", SettingsPreviewCard.PACING, PreviewPacing.LATENCY);
            assertFalse("the first picture is simply there", card.moving());
            card.show("frame_pacing", SettingsPreviewCard.PACING, PreviewPacing.LATENCY);
            assertFalse("nothing moves at rest", card.moving());
            card.show("frame_pacing", SettingsPreviewCard.PACING, PreviewPacing.SMOOTHNESS);
            assertTrue(card.moving());
            assertEquals(PreviewPacing.SMOOTHNESS, card.state(), 0f);
            card.show("video_scaling", SettingsPreviewCard.SCALING, VideoScaling.FILL);
            assertFalse("another setting's picture cuts", card.moving());

            // The list builds a new row for each change: the list remembers what the setting showed.
            StreamSettings.SettingsFragment display = SettingsStructureTest.open(screen, "display");
            ListPreference pacing = (ListPreference) display.findPreference("frame_pacing");
            ListView list = new ListView(screen);
            assertFalse(card(pacing.getView(null, list)).moving());
            pacing.setValue("balanced");
            assertTrue(card(pacing.getView(null, list)).moving());
            assertFalse("another list starts with the picture as it is", card(pacing.getView(null, new ListView(screen))).moving());

            prefs().edit().putString(Motion.REDUCE_MOTION, Motion.REDUCE).commit();
            pacing.setValue("latency");
            SettingsPreviewCard still = card(pacing.getView(null, list));
            assertFalse(still.moving());
            assertEquals(PreviewPacing.LATENCY, still.state(), 0f);
        }
    }

    @Test
    public void aPickerShowsOneCardPerChoiceAndATapPicksIt() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            StreamSettings.SettingsFragment display = SettingsStructureTest.open(screen, "display");
            MaterialListPreference pacing = (MaterialListPreference) display.findPreference("frame_pacing");
            pacing.showDialog(null);
            Dialog picker = ShadowDialog.getLatestDialog();
            List<SettingsPreviewCard> cards = cards(picker);
            assertEquals(pacing.getEntries().length, cards.size());
            for (int i = 0; i < cards.size(); i++) {
                SettingsPreviewCard card = cards.get(i);
                assertEquals("only the stored choice wears the amber container", i == pacing.findIndexOfValue("latency"), card.chosen());
                assertEquals(PreviewPacing.choice(pacing.getEntryValues()[i].toString()), card.state(), 0f);
                assertEquals(pacing.getEntries()[i].toString(),
                        ((TextView) card.findViewById(R.id.settings_preview_name)).getText().toString());
                assertTrue(card.isFocusable() && card.isClickable());
            }
            cards.get(pacing.findIndexOfValue("smoothness")).performClick();
            assertEquals("smoothness", pacing.getValue());
            assertEquals("smoothness", FramePacingChoice.read(prefs()));
            assertFalse(picker.isShowing());

            Preference size = display.findPreference(VideoScaling.PREF);
            size.getOnPreferenceClickListener().onPreferenceClick(size);
            picker = ShadowDialog.getLatestDialog();
            cards = cards(picker);
            assertEquals(screen.getResources().getStringArray(R.array.video_scaling_modes).length, cards.size());
            assertTrue(cards.get(VideoScaling.FIT).chosen());
            cards.get(VideoScaling.STRETCH).performClick();
            assertEquals(VideoScaling.STRETCH, StreamSettingsSearchAndSummaries.storedVideoScaling(prefs()));
            assertFalse(picker.isShowing());
        }
    }

    @Test
    public void aPhoneHasTwoCardsBesideEachOtherAndAWideScreenOneRowOfUpToFour() {
        assertEquals(2, SettingsPreviews.columns(360, 6));
        assertEquals(2, SettingsPreviews.columns(360, 4));
        assertEquals(1, SettingsPreviews.columns(360, 1));
        assertEquals(4, SettingsPreviews.columns(960, 4));
        assertEquals(3, SettingsPreviews.columns(960, 6));
    }

    @Test
    public void aChoicesNameFindsItsSetting() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            StreamSettings screen = controller.get();
            SettingsStructureTest.settle(screen);
            String stretch = screen.getResources().getStringArray(R.array.video_scaling_modes)[VideoScaling.STRETCH];
            assertNotNull(stretch, SettingsStructureTest.search(screen, stretch).getPreferenceScreen()
                    .findPreference("search_result_" + VideoScaling.PREF));
            String title = screen.getString(R.string.title_frame_pacing);
            assertNotNull(title, SettingsStructureTest.search(screen, title).getPreferenceScreen()
                    .findPreference("search_result_frame_pacing"));
        }
    }

    private static void assertVideoSizePickerScrollsAndPicksLastChoice(StreamSettings screen, int maxViewportPx) {
        StreamSettings.SettingsFragment display = SettingsStructureTest.open(screen, "display");
        Preference size = display.findPreference(VideoScaling.PREF);
        size.getOnPreferenceClickListener().onPreferenceClick(size);
        Dialog picker = ShadowDialog.getLatestDialog();
        assertNotNull(picker);
        View grid = picker.findViewById(R.id.settings_preview_grid);
        assertNotNull(grid);
        assertTrue("grid lives directly in a ScrollView", grid.getParent() instanceof android.widget.ScrollView);
        android.widget.ScrollView scroll = (android.widget.ScrollView) grid.getParent();
        ViewGroup.LayoutParams lp = grid.getLayoutParams();
        assertEquals(ViewGroup.LayoutParams.WRAP_CONTENT, lp.height);

        scroll.measure(View.MeasureSpec.makeMeasureSpec(900, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(maxViewportPx, View.MeasureSpec.AT_MOST));
        scroll.layout(0, 0, scroll.getMeasuredWidth(), scroll.getMeasuredHeight());

        List<SettingsPreviewCard> cards = cards(picker);
        assertEquals(5, cards.size());
        SettingsPreviewCard last = cards.get(VideoScaling.SAFE_FIT);
        assertTrue("fifth choice card has wrap-sized non-zero height", last.getHeight() > 0 && grid.getHeight() > 0);
        if (grid.getHeight() > scroll.getHeight()) {
            assertTrue("ScrollView can scroll down when content exceeds short viewport", scroll.canScrollVertically(1));
            scroll.scrollTo(0, grid.getHeight() - scroll.getHeight());
            assertTrue("scrolling to bottom reaches the fifth card", scroll.getScrollY() > 0);
        }
        last.performClick();
        assertEquals(VideoScaling.SAFE_FIT, StreamSettingsSearchAndSummaries.storedVideoScaling(prefs()));
        assertFalse(picker.isShowing());
    }

    @Test
    @Config(qualifiers = "w640dp-h360dp-land")
    public void onShortLandscapeScreenTheVideoSizePickerScrollsToTheFifthChoice() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            assertVideoSizePickerScrollsAndPicksLastChoice(controller.get(), 220);
        }
    }

    @Test
    @Config(qualifiers = "w322dp-h792dp-port", fontScale = 1.4f)
    public void onFoldCoverWithLargeFontScaleTheVideoSizePickerScrollsToAllFiveChoices() {
        try (ActivityController<StreamSettings> controller = Robolectric.buildActivity(StreamSettings.class).setup()) {
            assertVideoSizePickerScrollsAndPicksLastChoice(controller.get(), 260);
        }
    }
}
