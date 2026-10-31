package com.limelight.console;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Build;
import android.preference.PreferenceManager;
import android.view.View;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.StreamSettings;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.io.File;
import java.util.Collections;

/** A game's page under the shell bar: it belongs to Home, so Home in the bar is the page it came from. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameDetailShellTest {
    public static class Detail extends GameDetailActivity {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }
    }

    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private Context context;

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        File directory = new File(context.getFilesDir(), "console-library");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        assertTrue(new File(directory, "tower.json").createNewFile());
    }

    @After
    public void tearDown() {
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    private ActivityController<Detail> detail() {
        Intent intent = new Intent(context, Detail.class)
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, "tower")
                .putExtra(GameDetailActivity.EXTRA_APP_ID, 11)
                .putExtra(GameDetailActivity.EXTRA_TITLE, "Portal")
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, "Tower");
        return Robolectric.buildActivity(Detail.class, intent).setup().visible();
    }

    @Test
    public void theBarListsHomeAsTheCurrentDestination() {
        ActivityController<Detail> controller = detail();
        try {
            Detail page = controller.get();
            assertEquals(View.VISIBLE, page.findViewById(R.id.shell_home).getVisibility());
            assertTrue(page.findViewById(R.id.shell_home).isSelected());
            assertFalse(page.findViewById(R.id.shell_pcs).isSelected());
            assertFalse(page.findViewById(R.id.shell_settings).isSelected());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void homeInTheBarClosesThePageInsteadOfStackingAnotherHome() {
        ActivityController<Detail> controller = detail();
        try {
            Detail page = controller.get();
            page.findViewById(R.id.shell_home).performClick();
            assertTrue(page.isFinishing());
            assertNull("Home is already underneath", shadowOf(page).getNextStartedActivity());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void settingsFromThePageIsTheSettingsTheBarOpensAnywhere() {
        ActivityController<Detail> controller = detail();
        try {
            Detail page = controller.get();
            View tab = page.findViewById(R.id.shell_settings);
            tab.performClick();
            assertEquals("a tab answers the tap (nightjar#44)", android.view.HapticFeedbackConstants.VIRTUAL_KEY,
                    shadowOf(tab).lastHapticFeedbackPerformed());
            Intent settings = shadowOf(page).getNextStartedActivity();
            assertEquals(StreamSettings.class.getName(), settings.getComponent().getClassName());
            assertTrue(settings.getBooleanExtra(com.limelight.ui.ShellBar.EXTRA_FROM_SHELL, false));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void pcsFromThePageGoesBackToHomeWhenHomeIsTheRoot() {
        ActivityController<Detail> controller = detail();
        ActivityController<PcViewLifecycleTest.Library> pcs = null;
        try {
            Detail page = controller.get();
            View tab = page.findViewById(R.id.shell_pcs);
            tab.performClick();
            assertEquals(android.view.HapticFeedbackConstants.VIRTUAL_KEY, shadowOf(tab).lastHapticFeedbackPerformed());
            Intent opened = shadowOf(page).getNextStartedActivity();
            assertEquals(com.limelight.PcView.class.getName(), opened.getComponent().getClassName());
            assertFalse("the page stays underneath", page.isFinishing());

            // Same extras, the test subclass in place of PcView; Back from it is the Back of PcView.
            Intent forTest = new Intent(opened).setClass(context, PcViewLifecycleTest.Library.class);
            pcs = Robolectric.buildActivity(PcViewLifecycleTest.Library.class, forTest).setup().visible();
            PcViewLifecycleTest.Library list = pcs.get();
            list.getOnBackPressedDispatcher().onBackPressed();
            Intent home = shadowOf(list).getNextStartedActivity();
            assertEquals(ConsoleHomeActivity.class.getName(), home.getComponent().getClassName());
            assertEquals(Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP,
                    home.getFlags() & (Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP));
            assertTrue(list.isFinishing());
        } finally {
            if (pcs != null) pcs.pause().stop().destroy();
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void onTelevisionTheDpadWalksTheBarThenPlayThenTheStar() {
        ActivityController<Detail> controller = detail();
        try {
            Detail page = controller.get();
            View play = page.findViewById(R.id.detail_play);
            play.setEnabled(true);
            View home = page.findViewById(R.id.shell_home);
            View pcs = page.findViewById(R.id.shell_pcs);
            View settings = page.findViewById(R.id.shell_settings);
            assertSame(pcs, HomeShellTest.focusAfter(page, home, View.FOCUS_RIGHT));
            assertSame(settings, HomeShellTest.focusAfter(page, pcs, View.FOCUS_RIGHT));
            View below = HomeShellTest.focusAfter(page, home, View.FOCUS_DOWN);
            assertSame("down from the bar reaches Play first", play, below);
            assertSame("then the star, beside Play in the TV action row", page.findViewById(R.id.detail_favourite),
                    HomeShellTest.focusAfter(page, play, View.FOCUS_RIGHT));
            assertSame("and back", play, HomeShellTest.focusAfter(page, page.findViewById(R.id.detail_favourite), View.FOCUS_LEFT));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void onTelevisionTheHostLinesIconIsTheSizeOfItsText() {
        ActivityController<Detail> controller = detail();
        try {
            Detail page = controller.get();
            android.widget.TextView host = page.findViewById(R.id.detail_host);
            android.graphics.drawable.Drawable icon = host.getCompoundDrawablesRelative()[0];
            float density = page.getResources().getDisplayMetrics().density;
            assertTrue("ic_computer itself is 512dp", icon.getIntrinsicWidth() <= Math.round(24 * density));
            assertTrue(icon.getIntrinsicHeight() <= Math.round(24 * density));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void onTelevisionAScreenReaderHearsTheChipsAsOneLine() {
        ActivityController<Detail> controller = detail();
        try {
            Detail page = controller.get();
            android.view.ViewGroup chips = page.findViewById(R.id.detail_chips);
            assertEquals(android.view.View.IMPORTANT_FOR_ACCESSIBILITY_YES, chips.getImportantForAccessibility());
            android.widget.TextView status = page.findViewById(R.id.detail_status);
            android.widget.TextView installed = page.findViewById(R.id.detail_installed);
            android.widget.TextView hdr = page.findViewById(R.id.detail_hdr);
            status.setText("Running on Tower");
            status.setVisibility(android.view.View.VISIBLE);
            installed.setText(R.string.console_detail_installed);
            installed.setVisibility(android.view.View.VISIBLE);
            hdr.setVisibility(android.view.View.GONE);
            assertEquals("Running on Tower, " + page.getString(R.string.console_detail_installed),
                    String.valueOf(GameDetailActivity.chipsDescription(chips)));
            hdr.setVisibility(android.view.View.VISIBLE);
            assertTrue(String.valueOf(GameDetailActivity.chipsDescription(chips))
                    .endsWith(", " + page.getString(R.string.host_profile_hdr)));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /** Tower's library with Portal carrying {@code fields} (store, installed), read the way the page reads it. */
    private GameDetailModel portal(String fields) throws Exception {
        ConsoleRig rig = new ConsoleRig(temporary.newFolder());
        try {
            rig.transport.reply("tower", "library", NativeApiFixtures.ok(NativeApiFixtures.library("t1",
                    "{\"install_id\":\"t-portal\",\"app_id\":11,\"title\":\"Portal\"" + fields + "}")));
            rig.transport.reply("tower", "session", NativeApiFixtures.ok(NativeApiFixtures.IDLE));
            ConsoleSnapshot snapshot = rig.repository.refresh(
                    Collections.singletonList(NativeApiFixtures.nativeHost("tower", "Tower", true)));
            return GameDetailModel.of(snapshot, "tower", 11, "Portal", "Tower", true, true, true);
        } finally {
            rig.close();
        }
    }

    /** The page as its first reload leaves it: the cover loaders it makes once it knows this device's id. */
    private static Detail loaded(ActivityController<Detail> controller) {
        Detail page = controller.get();
        org.robolectric.util.ReflectionHelpers.setField(page, "covers", new CoverLoaders(page, "test-device", 200));
        return page;
    }

    private static ComputerDetails tower() {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "tower";
        computer.name = "Tower";
        return computer;
    }

    @Test
    public void aGameNotInstalledOnTheHostSaysSoUnderAnUnavailablePlay() throws Exception {
        GameDetailModel model = portal(",\"installed\":false");
        ActivityController<Detail> controller = detail();
        try {
            Detail page = loaded(controller);
            page.render(model, tower());
            android.widget.TextView note = page.findViewById(R.id.detail_note);
            assertEquals(View.VISIBLE, note.getVisibility());
            assertEquals("Not installed on Tower", note.getText().toString());
            assertFalse(page.findViewById(R.id.detail_play).isEnabled());

            page.render(portal(",\"installed\":true"), tower());
            assertEquals(View.GONE, note.getVisibility());
            assertTrue(page.findViewById(R.id.detail_play).isEnabled());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void outsideTvAStoreGameNotInstalledRendersWithItsNoteAndNoStoreChip() throws Exception {
        // The handheld page has no chip line: a store-owned game must still render and say why Play is off.
        ActivityController<Detail> controller = detail();
        try {
            Detail page = loaded(controller);
            assertNull(page.findViewById(R.id.detail_chips));
            assertNull(page.findViewById(R.id.detail_store));
            page.render(portal(",\"installed\":false,\"store\":{\"source\":\"steam\",\"id\":\"400\"}"), tower());
            android.widget.TextView note = page.findViewById(R.id.detail_note);
            assertEquals(View.VISIBLE, note.getVisibility());
            assertEquals("Not installed on Tower", note.getText().toString());
            assertFalse(page.findViewById(R.id.detail_play).isEnabled());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    @Config(qualifiers = "television")
    public void onTelevisionTheChipLineNamesTheStoreWithItsGlyph() throws Exception {
        ActivityController<Detail> controller = detail();
        try {
            Detail page = loaded(controller);
            android.widget.TextView store = page.findViewById(R.id.detail_store);
            page.render(portal(",\"store\":{\"source\":\"steam\",\"id\":\"400\"}"), tower());
            assertEquals(View.VISIBLE, store.getVisibility());
            assertEquals("Steam", store.getText().toString());
            android.graphics.drawable.Drawable glyph = store.getCompoundDrawablesRelative()[0];
            assertTrue(glyph != null);
            assertEquals(Math.round(store.getTextSize()), glyph.getBounds().width());
            assertEquals(View.VISIBLE, page.findViewById(R.id.detail_chips).getVisibility());
            assertTrue(String.valueOf(page.findViewById(R.id.detail_chips).getContentDescription()).contains("Steam"));

            page.render(portal(""), tower());
            assertEquals(View.GONE, store.getVisibility());
            assertNull(store.getCompoundDrawablesRelative()[0]);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /**
     * Parity with desktop's game page: the game's own "Quit the game when I leave the stream", until now only in the
     * library's long-press menu, is a switch row here. The row flips it, says what leaving now does, and reads as a switch.
     */
    @Test
    public void theGamesOwnQuitOnLeaveIsASwitchRowOnItsPage() throws Exception {
        context.getSharedPreferences("AppQuitOnLeave", Context.MODE_PRIVATE).edit().clear().commit();
        GameDetailModel model = portal(",\"installed\":true");
        ActivityController<Detail> controller = detail();
        try {
            Detail page = loaded(controller);
            page.render(model, tower());
            View row = page.findViewById(R.id.detail_quit_on_leave);
            com.google.android.material.materialswitch.MaterialSwitch toggle =
                    page.findViewById(R.id.detail_quit_on_leave_switch);
            android.widget.TextView note = page.findViewById(R.id.detail_quit_on_leave_note);
            assertEquals(View.VISIBLE, row.getVisibility());
            assertFalse(toggle.isChecked());
            assertEquals(page.getString(R.string.library_quit_on_leave_off), note.getText().toString());
            android.view.accessibility.AccessibilityNodeInfo info = row.createAccessibilityNodeInfo();
            assertEquals(android.widget.Switch.class.getName(), String.valueOf(info.getClassName()));
            assertTrue(info.isCheckable());

            row.performClick();
            assertTrue(toggle.isChecked());
            assertTrue(com.limelight.preferences.AppQuitOnLeave.isEnabled(page, "tower", model.source.appUuid, 11));
            assertEquals(page.getString(R.string.library_quit_on_leave_on), note.getText().toString());
            assertTrue(row.createAccessibilityNodeInfo().isChecked());

            row.performClick();
            assertFalse(com.limelight.preferences.AppQuitOnLeave.isEnabled(page, "tower", model.source.appUuid, 11));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    /**
     * Android 7.1 and older take a next-focus target even when it is gone, and focus then stays put: Down from Favourite
     * must reach the quit-on-leave row while Close game is hidden, and Close game when it shows.
     */
    @Test
    @Config(sdk = 25)
    public void downFromFavouriteReachesTheQuitOnLeaveRowWhileTheEndButtonIsGone() throws Exception {
        GameDetailModel model = portal(",\"installed\":true");
        ActivityController<Detail> controller = detail();
        try {
            Detail page = loaded(controller);
            page.render(model, tower());
            View favourite = page.findViewById(R.id.detail_favourite);
            View end = page.findViewById(R.id.detail_end);
            View row = page.findViewById(R.id.detail_quit_on_leave);
            assertEquals(View.GONE, end.getVisibility());
            assertEquals(View.VISIBLE, row.getVisibility());
            assertEquals(View.NO_ID, favourite.getNextFocusDownId());
            page.getWindow().getDecorView().measure(View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(4000, View.MeasureSpec.EXACTLY));
            page.getWindow().getDecorView().layout(0, 0, 1080, 4000);
            View next = favourite.focusSearch(View.FOCUS_DOWN);
            assertSame(row, next);

            end.setVisibility(View.VISIBLE);
            page.getWindow().getDecorView().measure(View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(4000, View.MeasureSpec.EXACTLY));
            page.getWindow().getDecorView().layout(0, 0, 1080, 4000);
            assertSame(end, favourite.focusSearch(View.FOCUS_DOWN));
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
