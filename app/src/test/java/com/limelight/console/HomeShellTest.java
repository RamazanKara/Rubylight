package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Build;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.EditText;
import android.widget.LinearLayout;

import androidx.appcompat.app.AlertDialog;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.appbar.MaterialToolbar;
import com.google.android.material.button.MaterialButton;
import com.limelight.PcView;
import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.GlPreferences;
import com.limelight.ui.ActionSheet;
import com.limelight.ui.ShellBar;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/** Home under the shell bar, and the options of the PCs listed on it. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class HomeShellTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    /** Home without the computer manager service: tests report hosts themselves. */
    public static class Home extends ConsoleHomeActivity {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }
    }

    private Context context;
    private final List<String> renamed = new ArrayList<>();
    private final List<ComputerDetails> removed = new ArrayList<>();

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        // What a host's refresh leaves behind once it has shown its games.
        File directory = new File(context.getFilesDir(), "console-library");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        assertTrue(new File(directory, "tower.json").createNewFile());
    }

    @After
    public void tearDown() {
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    private ActivityController<Home> home() {
        return Robolectric.buildActivity(Home.class).setup().visible();
    }

    private static ComputerManagerService.ComputerManagerBinder binder(List<String> renamed, List<ComputerDetails> removed) {
        return new ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) { }
            @Override public void stopPolling() { }
            @Override public void renameComputer(String uuid, String alias) { renamed.add(uuid + "=" + alias); }
            @Override public void removeComputer(ComputerDetails computer) { removed.add(computer); }
            @Override public void invalidateStateForComputer(String uuid) { }
            @Override public String getUniqueId() { return null; }
            @Override public void waitForPollingStopped() { }
        };
    }

    /** Reports the hosts to Home's feed and paints the cards they would give, laid out in a 1080x1920 list. */
    private RecyclerView showHosts(Home home, ComputerDetails... hosts) throws Exception {
        HostFeed feed = ReflectionHelpers.getField(home, "feed");
        ReflectionHelpers.setField(feed, "binder", binder(renamed, removed));
        ConsoleRig rig = new ConsoleRig(temporary.newFolder());
        try {
            Set<String> online = new HashSet<>();
            for (ComputerDetails host : hosts) {
                feed.update(host);
                if (host.state == ComputerDetails.State.ONLINE) online.add(host.uuid);
                if (host.state == ComputerDetails.State.ONLINE) {
                    rig.transport.reply(host.uuid, "library", ok(library("r", game(host.uuid + "-g", 1, "Portal", null, false))));
                    rig.transport.reply(host.uuid, "session", ok(NativeApiFixtures.IDLE));
                }
            }
            ConsoleSnapshot snapshot = rig.repository.refresh(Arrays.asList(hosts));
            ConsoleHomeAdapter adapter = ReflectionHelpers.getField(home, "adapter");
            adapter.submit(HomeModel.of(snapshot, online));
        } finally {
            rig.close();
        }
        RecyclerView list = home.findViewById(R.id.console_list);
        list.measure(View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.EXACTLY));
        list.layout(0, 0, 1080, 1920);
        return list;
    }

    private static View hostCard(Home home, String uuid) {
        ConsoleHomeAdapter adapter = ReflectionHelpers.getField(home, "adapter");
        RecyclerView list = home.findViewById(R.id.console_list);
        int position = adapter.positionOfKey("host:" + uuid);
        assertTrue("a card for " + uuid, position >= 0);
        list.measure(View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.EXACTLY));
        list.layout(0, 0, 1080, 1920);
        RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(position);
        assertNotNull("laid out", holder);
        return holder.itemView;
    }

    private static List<String> labels(ActionSheet sheet) {
        List<String> actions = new ArrayList<>();
        collect(sheet.findViewById(android.R.id.content), actions);
        return actions;
    }

    private static void collect(View view, List<String> actions) {
        if (view instanceof MaterialButton) {
            actions.add(((MaterialButton) view).getText().toString());
        } else if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) collect(group.getChildAt(i), actions);
        }
    }

    private List<String> expected(Home home, int... ids) {
        List<String> labels = new ArrayList<>();
        for (int id : ids) labels.add(home.getString(id));
        return labels;
    }

    private static ActionSheet openSheet() {
        shadowOf(Looper.getMainLooper()).idle();
        android.app.Dialog dialog = ShadowDialog.getLatestDialog();
        return dialog instanceof ActionSheet && dialog.isShowing() ? (ActionSheet) dialog : null;
    }

    private static void closeSheet(ActionSheet sheet) {
        sheet.dismiss();
        shadowOf(Looper.getMainLooper()).idle();
    }

    private static boolean press(View card, int keyCode) {
        // A view group hands key events to its own listener only while it has focus.
        if (!card.hasFocus()) assertTrue(card.requestFocus());
        boolean down = card.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, keyCode));
        boolean up = card.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, keyCode));
        return down || up;
    }

    private static Intent nextStarted(android.app.Activity activity) {
        return shadowOf(activity).getNextStartedActivity();
    }

    @Test
    public void homeHasTheShellBarWithHomeCurrentAndNoMenuInItsToolbar() {
        ActivityController<Home> controller = home();
        try {
            ViewGroup bar = controller.get().findViewById(R.id.shell_bar);
            assertNotNull(bar);
            LinearLayout items = bar.findViewById(R.id.shell_bar_items);
            List<Integer> shown = new ArrayList<>();
            List<Integer> selected = new ArrayList<>();
            for (int i = 0; i < items.getChildCount(); i++) {
                View item = items.getChildAt(i);
                if (item.getVisibility() == View.VISIBLE) shown.add(item.getId());
                if (item.isSelected()) selected.add(item.getId());
            }
            assertEquals(Arrays.asList(R.id.shell_home, R.id.shell_pcs, R.id.shell_settings), shown);
            assertEquals(Collections.singletonList(R.id.shell_home), selected);
            MaterialToolbar toolbar = controller.get().findViewById(R.id.console_toolbar);
            assertEquals("Hosts and Settings left the toolbar for the bar", 0, toolbar.getMenu().size());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void thePcsButtonOpensTheListOnTopOfHomeAndSettingsOpensFromTheBar() {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            screen.findViewById(R.id.shell_pcs).performClick();
            Intent pcs = nextStarted(screen);
            assertEquals(PcView.class.getName(), pcs.getComponent().getClassName());
            assertTrue(pcs.getBooleanExtra(HomeSettings.EXTRA_STAY, false));
            assertFalse("Home is the root and stays under the list", screen.isFinishing());
            screen.findViewById(R.id.shell_settings).performClick();
            Intent settings = nextStarted(screen);
            assertTrue(settings.getBooleanExtra(ShellBar.EXTRA_FROM_SHELL, false));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void backFromHomeThatIsNotTheRootReturnsToTheListWithoutStacking() {
        PreferenceManager.getDefaultSharedPreferences(context).edit().putBoolean(HomeSettings.PREF_KEY, false).commit();
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            screen.getOnBackPressedDispatcher().onBackPressed();
            Intent pcs = nextStarted(screen);
            assertEquals(PcView.class.getName(), pcs.getComponent().getClassName());
            assertEquals(Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP,
                    pcs.getFlags() & (Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP));
            assertTrue(screen.isFinishing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void everyOpenerOfAHostCardsOptionsShowsTheSameListAsThePcsScreen() throws Exception {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            showHosts(screen, nativeHost("tower", "Tower", true));
            List<String> expected = expected(screen, R.string.library_open, R.string.pc_menu_settings,
                    R.string.host_menu_rename, R.string.pcview_menu_unpair_pc, R.string.pcview_menu_delete_pc);

            View card = hostCard(screen, "tower");
            card.findViewById(R.id.console_more).performClick();
            ActionSheet sheet = openSheet();
            assertNotNull("the three dots", sheet);
            assertEquals(expected, labels(sheet));
            closeSheet(sheet);

            assertTrue(card.performLongClick());
            sheet = openSheet();
            assertNotNull("a long press", sheet);
            assertEquals(expected, labels(sheet));
            closeSheet(sheet);

            for (int key : new int[]{KeyEvent.KEYCODE_MENU, KeyEvent.KEYCODE_BUTTON_X}) {
                assertTrue(press(card, key));
                sheet = openSheet();
                assertNotNull("key " + key, sheet);
                assertEquals(expected, labels(sheet));
                closeSheet(sheet);
            }

            assertTrue(card.dispatchKeyEvent(new KeyEvent(0, 0, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_F10, 0, KeyEvent.META_SHIFT_ON)));
            sheet = openSheet();
            assertNotNull("Shift+F10", sheet);
            closeSheet(sheet);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void anOfflineHostOffersWakeOnlyWithAMacAddress() throws Exception {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            ComputerDetails attic = nativeHost("attic", "Attic", false);
            attic.macAddress = "aa:bb:cc:dd:ee:ff";
            showHosts(screen, attic, nativeHost("loft", "Loft", false));
            hostCard(screen, "attic").findViewById(R.id.console_more).performClick();
            ActionSheet sheet = openSheet();
            assertEquals(expected(screen, R.string.pcview_menu_send_wol, R.string.pc_menu_settings,
                    R.string.host_menu_rename, R.string.pcview_menu_delete_pc), labels(sheet));
            closeSheet(sheet);
            hostCard(screen, "loft").findViewById(R.id.console_more).performClick();
            sheet = openSheet();
            assertEquals(expected(screen, R.string.pc_menu_settings, R.string.host_menu_rename,
                    R.string.pcview_menu_delete_pc), labels(sheet));
            closeSheet(sheet);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void theDpadMarksTheThreeDotsAtTheEndOfTheRowAndCenterOpensThem() throws Exception {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            showHosts(screen, nativeHost("tower", "Tower", true));
            View card = hostCard(screen, "tower");
            View more = card.findViewById(R.id.console_more);
            assertFalse("never a focus stop of its own", more.isFocusable());

            assertTrue(press(card, KeyEvent.KEYCODE_DPAD_RIGHT));
            assertTrue("Right marks the dots", more.isHovered());
            assertTrue(press(card, KeyEvent.KEYCODE_DPAD_LEFT));
            assertFalse("Left gives them back", more.isHovered());

            press(card, KeyEvent.KEYCODE_DPAD_RIGHT);
            assertTrue(more.isHovered());
            assertTrue(press(card, KeyEvent.KEYCODE_BACK));
            assertFalse("Back only unmarks", more.isHovered());
            assertNull(openSheet());

            press(card, KeyEvent.KEYCODE_DPAD_RIGHT);
            assertTrue(press(card, KeyEvent.KEYCODE_DPAD_CENTER));
            assertNotNull("Center opens the options", openSheet());
            assertFalse(more.isHovered());
            closeSheet(openSheet());

            // Without a mark, Center is the card's own tap.
            press(card, KeyEvent.KEYCODE_DPAD_CENTER);
            assertNull(openSheet());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void renameAndRemoveFromHomeReachTheComputerManager() throws Exception {
        ActivityController<Home> controller = home();
        try {
            Home screen = controller.get();
            ComputerDetails tower = nativeHost("tower", "Tower", true);
            showHosts(screen, tower);
            HostFeed feed = ReflectionHelpers.getField(screen, "feed");

            hostCard(screen, "tower").findViewById(R.id.console_more).performClick();
            ActionSheet sheet = openSheet();
            findButton(sheet, screen.getString(R.string.host_menu_rename)).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            EditText field = dialog.findViewById(R.id.rename_name);
            field.setText("Living room");
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            assertEquals(Collections.singletonList("tower=Living room"), renamed);
            assertEquals("Living room", feed.host("tower").displayName());

            hostCard(screen, "tower").findViewById(R.id.console_more).performClick();
            sheet = openSheet();
            findButton(sheet, screen.getString(R.string.pcview_menu_delete_pc)).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            ((AlertDialog) ShadowDialog.getLatestDialog()).getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            shadowOf(Looper.getMainLooper()).idle();
            assertEquals(1, removed.size());
            assertNull("the host leaves the feed at once", feed.host("tower"));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static MaterialButton findButton(ActionSheet sheet, String text) {
        MaterialButton[] found = new MaterialButton[1];
        find(sheet.findViewById(android.R.id.content), text, found);
        assertNotNull("a button " + text, found[0]);
        return found[0];
    }

    private static void find(View view, String text, MaterialButton[] found) {
        if (view instanceof MaterialButton && ((MaterialButton) view).getText().toString().equals(text)) {
            found[0] = (MaterialButton) view;
        } else if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) find(group.getChildAt(i), text, found);
        }
    }
}
