package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Build;
import android.os.Looper;
import android.widget.GridView;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.GlPreferences;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.shadows.ShadowDialog;

import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.CountDownLatch;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class PcViewLifecycleTest {
    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class NoNativeMoonBridge {
        @Implementation protected static void __staticInitializer__() { }
    }

    public static class Library extends PcView {
        final java.util.List<ComputerDetails> woken = new java.util.ArrayList<>();

        // The update check would reach for the network.
        @Override protected void checkForUpdates() { }

        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }

        // Tests record the wake instead of broadcasting a magic packet.
        @Override protected void doWakeOnLan(ComputerDetails computer) {
            woken.add(computer);
        }
    }

    private final ComputerManagerListener[] polling = new ComputerManagerListener[1];
    private final java.util.Map<String, String> renamed = new java.util.LinkedHashMap<>();
    private final java.util.List<ComputerDetails> removed = new java.util.ArrayList<>();

    private ActivityController<Library> pairingScreen(CountDownLatch pollingStopped, int port) {
        Application application = RuntimeEnvironment.getApplication();
        GlPreferences gl = GlPreferences.readPreferences(application);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        application.getSharedPreferences("FirstRun", 0).edit().putBoolean("pairing_guide_shown", true).commit();
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class).setup().visible();
        ComputerManagerService manager = new ComputerManagerService();
        ReflectionHelpers.setField(controller.get(), "managerBinder", manager.new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) { polling[0] = listener; }
            @Override public void stopPolling() { }
            @Override public void renameComputer(String uuid, String alias) { renamed.put(uuid, alias); }
            @Override public void removeComputer(ComputerDetails computer) { removed.add(computer); }
            @Override public String getUniqueId() { return "client"; }
            @Override public void waitForPollingStopped() {
                try {
                    pollingStopped.await();
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
            }
        });
        ReflectionHelpers.callInstanceMethod(controller.get(), "startComputerUpdates");
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "host-uuid";
        computer.name = "PC";
        computer.state = ComputerDetails.State.ONLINE;
        computer.pairState = PairingManager.PairState.NOT_PAIRED;
        computer.activeAddress = new ComputerDetails.AddressTuple("127.0.0.1", port);
        ReflectionHelpers.callInstanceMethod(controller.get(), "updateComputer",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, computer));
        return controller;
    }

    private static ComputerDetails pc(String uuid, String name, ComputerDetails.State state, PairingManager.PairState pairing) {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = uuid;
        computer.name = name;
        computer.state = state;
        computer.pairState = pairing;
        computer.activeAddress = state == ComputerDetails.State.ONLINE ?
                new ComputerDetails.AddressTuple("127.0.0.1", 47989) : null;
        return computer;
    }

    /** Reports a PC the way the computer manager does and waits for the home screen to show it. */
    private void report(ComputerDetails computer) {
        polling[0].notifyComputerUpdated(computer);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
    }

    /** The sheet's actions, top to bottom, for the PC named {@code name}. */
    private static java.util.List<String> sheetFor(Library screen, String name) {
        GridView grid = screen.findViewById(R.id.fragmentView);
        for (int i = 0; i < grid.getCount(); i++) {
            PcView.ComputerObject computer = (PcView.ComputerObject) grid.getItemAtPosition(i);
            if (computer.details.name.equals(name)) {
                screen.showComputerMenu(computer);
                com.limelight.ui.ActionSheet sheet = (com.limelight.ui.ActionSheet) ShadowDialog.getLatestDialog();
                java.util.List<String> actions = new java.util.ArrayList<>();
                collectActions(sheet.findViewById(android.R.id.content), actions);
                sheet.dismiss();
                Shadows.shadowOf(Looper.getMainLooper()).idle();
                return actions;
            }
        }
        throw new AssertionError("No row for " + name);
    }

    private static void collectActions(android.view.View view, java.util.List<String> actions) {
        if (view instanceof com.google.android.material.button.MaterialButton) {
            com.google.android.material.button.MaterialButton button = (com.google.android.material.button.MaterialButton) view;
            actions.add(button.getText() + (button.isEnabled() ? "" : " (disabled)"));
        } else if (view instanceof android.view.ViewGroup) {
            android.view.ViewGroup group = (android.view.ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                collectActions(group.getChildAt(i), actions);
            }
        }
    }

    private static java.util.List<String> labels(Library screen, int... ids) {
        java.util.List<String> labels = new java.util.ArrayList<>();
        for (int id : ids) labels.add(screen.getString(id));
        return labels;
    }

    @Test
    public void eachPcStateOffersItsOwnShortMenu() {
        ActivityController<Library> controller = pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            ComputerDetails running = pc("running", "Den", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED);
            running.runningGameId = 7;
            report(running);
            ComputerDetails ready = pc("ready", "Office", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED);
            report(ready);
            ComputerDetails apollo = pc("apollo", "Apollo", ComputerDetails.State.ONLINE, PairingManager.PairState.NOT_PAIRED);
            apollo.caps = com.limelight.nvstream.http.TestHosts.fixture("apollo-http-serverinfo.xml");
            report(apollo);
            ComputerDetails asleep = pc("asleep", "Attic", ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED);
            asleep.macAddress = "02:00:00:00:00:01";
            report(asleep);

            assertEquals(labels(screen, R.string.library_open, R.string.applist_menu_resume, R.string.applist_menu_quit,
                    R.string.pc_menu_settings, R.string.host_menu_rename, R.string.pcview_menu_unpair_pc,
                    R.string.pcview_menu_delete_pc), sheetFor(screen, "Den"));
            assertEquals(labels(screen, R.string.library_open, R.string.pc_menu_settings, R.string.host_menu_rename,
                    R.string.pcview_menu_unpair_pc, R.string.pcview_menu_delete_pc), sheetFor(screen, "Office"));
            assertEquals(labels(screen, R.string.pcview_menu_pair_pc, R.string.pair_otp_title, R.string.pc_menu_settings,
                    R.string.host_menu_rename, R.string.discovery_help, R.string.pcview_menu_delete_pc),
                    sheetFor(screen, "Apollo"));
            // A PC this device has never read offers no one-time PIN.
            assertEquals(labels(screen, R.string.pcview_menu_pair_pc, R.string.pc_menu_settings, R.string.host_menu_rename,
                    R.string.discovery_help, R.string.pcview_menu_delete_pc), sheetFor(screen, "PC"));
            assertEquals(labels(screen, R.string.pcview_menu_send_wol, R.string.pc_menu_settings, R.string.host_menu_rename,
                    R.string.pcview_menu_delete_pc), sheetFor(screen, "Attic"));
            // Without a MAC address Wake is hidden and the sheet says how to get one.
            report(pc("lost", "Loft", ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED));
            assertEquals(labels(screen, R.string.pc_menu_settings, R.string.host_menu_rename,
                    R.string.pcview_menu_delete_pc), sheetFor(screen, "Loft"));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void rowsListRunningThenReadyThenUnpairedThenOfflinePcs() {
        ActivityController<Library> controller = pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            report(pc("b-off", "Basement", ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED));
            report(pc("a-off", "Attic", ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED));
            report(pc("ready", "Office", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            ComputerDetails running = pc("running", "Zen", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED);
            running.runningGameUuid = "apollo-input-only";
            report(running);
            GridView grid = screen.findViewById(R.id.fragmentView);
            java.util.List<String> order = new java.util.ArrayList<>();
            java.util.List<String> status = new java.util.ArrayList<>();
            for (int i = 0; i < grid.getCount(); i++) {
                PcView.ComputerObject computer = (PcView.ComputerObject) grid.getItemAtPosition(i);
                order.add(computer.details.name);
                status.add(com.limelight.grid.PcGridAdapter.statusText(screen, computer));
            }
            assertEquals(java.util.List.of("Zen", "Office", "PC", "Attic", "Basement"), order);
            assertEquals(java.util.List.of(screen.getString(R.string.pc_status_playing_unknown),
                    screen.getString(R.string.pc_status_ready), screen.getString(R.string.pc_needs_pairing),
                    screen.getString(R.string.pc_offline), screen.getString(R.string.pc_offline)), status);

            // The running game is named once the PC's app list has it.
            running.rawAppList = "<root status_code=\"200\"><App><AppTitle>Racing Game</AppTitle><ID>2</ID>" +
                    "<UUID>apollo-input-only</UUID></App></root>";
            running.runningGameUuid = "apollo-input-only";
            ComputerDetails update = new ComputerDetails(running);
            update.runningGameId = 2;
            report(update);
            for (int i = 0; i < 50 && ((PcView.ComputerObject) grid.getItemAtPosition(0)).runningGame == null; i++) {
                Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(20));
                try { Thread.sleep(20); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
            }
            assertEquals(screen.getString(R.string.pc_status_playing, "Racing Game"),
                    com.limelight.grid.PcGridAdapter.statusText(screen, (PcView.ComputerObject) grid.getItemAtPosition(0)));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void tappingAnOfflinePcWithAMacWakesItAndTheRowSaysSo() {
        ActivityController<Library> controller = pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            ComputerDetails asleep = pc("asleep", "Attic", ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED);
            asleep.macAddress = "02:00:00:00:00:01";
            report(asleep);
            GridView grid = screen.findViewById(R.id.fragmentView);
            int row = grid.getCount() - 1;
            PcView.ComputerObject computer = (PcView.ComputerObject) grid.getItemAtPosition(row);
            assertSame(asleep, computer.details);
            assertEquals(screen.getString(R.string.pcview_menu_send_wol),
                    screen.getString(com.limelight.grid.PcGridAdapter.mainActionLabel(computer)));
            grid.performItemClick(null, row, row);
            assertEquals(java.util.List.of(asleep), screen.woken);
            assertNull("Waking is the tap's action, not a menu", ShadowDialog.getLatestDialog());

            computer.wakeSentAt = android.os.SystemClock.elapsedRealtime();
            assertEquals(screen.getString(R.string.pc_status_waking),
                    com.limelight.grid.PcGridAdapter.statusText(screen, computer));
            // The PC answers, then sleeps again: it is offline, no longer waking.
            ComputerDetails awake = new ComputerDetails(asleep);
            awake.state = ComputerDetails.State.ONLINE;
            report(awake);
            assertEquals(screen.getString(R.string.pc_status_ready),
                    com.limelight.grid.PcGridAdapter.statusText(screen, computer));
            report(asleep);
            assertEquals(screen.getString(R.string.pc_offline),
                    com.limelight.grid.PcGridAdapter.statusText(screen, computer));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void tappingUnpairedPcShowsPinBeforePollingStopsAndCanBeCancelled() throws Exception {
        ActivityController<Library> controller = pairingScreen(new CountDownLatch(1), 47989);
        Thread worker = null;
        try {
            Library screen = controller.get();
            GridView grid = screen.findViewById(R.id.fragmentView);
            grid.performItemClick(null, 0, 0);
            worker = ReflectionHelpers.getField(screen.hostActions(), "pairingThread");
            AlertDialog dialog = ReflectionHelpers.getField(screen.hostActions(), "pairingDialog");
            assertNotNull(dialog);
            assertTrue(dialog.isShowing());
            TextView message = dialog.findViewById(android.R.id.message);
            // One instruction, then the PIN.
            String text = message.getText().toString();
            assertTrue(text, text.matches("(?s)" + java.util.regex.Pattern.quote(
                    screen.getString(R.string.pair_pairing_help)) + "\\n\\n[0-9]{4}"));
            assertFalse(ReflectionHelpers.<Boolean>getField(screen, "runningPolling"));

            dialog.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            worker.join(5000);
            assertFalse(worker.isAlive());
            assertFalse(dialog.isShowing());
            assertNull(ReflectionHelpers.getField(screen.hostActions(), "pairingDialog"));
            assertNull(ReflectionHelpers.getField(screen.hostActions(), "pairingRequest"));
            assertTrue(ReflectionHelpers.<Boolean>getField(screen, "runningPolling"));
        } finally {
            controller.pause().stop().destroy();
            if (worker != null) worker.join(5000);
        }
    }

    @Test
    public void pairingShowsPinDuringHostCheckAndSupportsRetryAfterFailure() throws Exception {
        try (ServerSocket host = new ServerSocket(0, 1, InetAddress.getByName("127.0.0.1"))) {
            host.setSoTimeout(5000);
            ActivityController<Library> controller = pairingScreen(new CountDownLatch(0), host.getLocalPort());
            Thread worker = null;
            try {
                Library screen = controller.get();
                GridView grid = screen.findViewById(R.id.fragmentView);
                grid.performItemClick(null, 0, 0);
                worker = ReflectionHelpers.getField(screen.hostActions(), "pairingThread");
                AlertDialog pinDialog;
                try (Socket connection = host.accept()) {
                    Shadows.shadowOf(Looper.getMainLooper()).idle();
                    pinDialog = ReflectionHelpers.getField(screen.hostActions(), "pairingDialog");
                    assertNotNull(pinDialog);
                    assertTrue(pinDialog.isShowing());
                    connection.getOutputStream().write(("HTTP/1.1 503 Service Unavailable\r\n" +
                            "Content-Length: 0\r\nConnection: close\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
                    worker.join(5000);
                    assertFalse(worker.isAlive());
                }
                Shadows.shadowOf(Looper.getMainLooper()).idle();
                assertFalse(pinDialog.isShowing());
                AlertDialog failure = (AlertDialog) ShadowDialog.getLatestDialog();
                assertTrue(failure.isShowing());
                assertEquals(screen.getString(R.string.pair_connection_fix),
                        ((TextView) failure.findViewById(android.R.id.message)).getText().toString());
                assertTrue(ReflectionHelpers.<Boolean>getField(screen, "runningPolling"));

                failure.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
                Shadows.shadowOf(Looper.getMainLooper()).idle();
                worker = ReflectionHelpers.getField(screen.hostActions(), "pairingThread");
                try (Socket connection = host.accept()) {
                    AlertDialog retry = ReflectionHelpers.getField(screen.hostActions(), "pairingDialog");
                    assertNotNull(retry);
                    assertTrue(retry.isShowing());
                    // Leaving to type the PIN into the host's web page on this phone keeps pairing going.
                    controller.pause();
                    assertTrue(worker.isAlive());
                    assertSame(retry, ReflectionHelpers.getField(screen.hostActions(), "pairingDialog"));
                    assertNotNull(ReflectionHelpers.getField(screen.hostActions(), "pairingRequest"));
                    connection.getOutputStream().write(("HTTP/1.1 503 Service Unavailable\r\n" +
                            "Content-Length: 0\r\nConnection: close\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
                    worker.join(5000);
                    assertFalse(worker.isAlive());
                }
                // The result waits for the return to the PC list.
                Shadows.shadowOf(Looper.getMainLooper()).idle();
                AlertDialog retry = ReflectionHelpers.getField(screen.hostActions(), "pairingDialog");
                assertTrue(retry.isShowing());
                assertFalse(ReflectionHelpers.<Boolean>getField(screen, "runningPolling"));
                controller.resume();
                Shadows.shadowOf(Looper.getMainLooper()).idle();
                assertFalse(retry.isShowing());
                AlertDialog again = (AlertDialog) ShadowDialog.getLatestDialog();
                assertNotSame(failure, again);
                assertTrue(again.isShowing());
                assertEquals(screen.getString(R.string.pair_connection_fix),
                        ((TextView) again.findViewById(android.R.id.message)).getText().toString());
                assertNull(ReflectionHelpers.getField(screen.hostActions(), "pairingRequest"));
                assertTrue(ReflectionHelpers.<Boolean>getField(screen, "runningPolling"));
            } finally {
                if (controller != null) controller.pause().stop().destroy();
                if (worker != null) worker.join(5000);
            }
        }
    }

    private enum Input { TOUCH_MORE, LONG_PRESS, MOUSE_RIGHT_CLICK, DPAD_RIGHT_THEN_CENTER, MENU_KEY, SHIFT_F10, BUTTON_X }

    private static void press(android.view.View view, int code, int meta) {
        long now = android.os.SystemClock.uptimeMillis();
        view.dispatchKeyEvent(new android.view.KeyEvent(now, now, android.view.KeyEvent.ACTION_DOWN, code, 0, meta));
        view.dispatchKeyEvent(new android.view.KeyEvent(now, now, android.view.KeyEvent.ACTION_UP, code, 0, meta));
    }

    /** A mouse's right button pressed and released over {@code card}. */
    private static void rightClick(GridView grid, android.view.View card) {
        float x = card.getLeft() + card.getWidth() / 2f;
        float y = card.getTop() + card.getHeight() / 2f;
        long now = android.os.SystemClock.uptimeMillis();
        for (int action : new int[] {android.view.MotionEvent.ACTION_DOWN, android.view.MotionEvent.ACTION_UP}) {
            android.view.MotionEvent.PointerProperties properties = new android.view.MotionEvent.PointerProperties();
            properties.id = 0;
            properties.toolType = android.view.MotionEvent.TOOL_TYPE_MOUSE;
            android.view.MotionEvent.PointerCoords coords = new android.view.MotionEvent.PointerCoords();
            coords.x = x;
            coords.y = y;
            android.view.MotionEvent event = android.view.MotionEvent.obtain(now, now, action, 1,
                    new android.view.MotionEvent.PointerProperties[] {properties},
                    new android.view.MotionEvent.PointerCoords[] {coords}, 0,
                    action == android.view.MotionEvent.ACTION_DOWN ? android.view.MotionEvent.BUTTON_SECONDARY : 0,
                    1, 1, 0, 0, android.view.InputDevice.SOURCE_MOUSE, 0);
            assertTrue(grid.dispatchTouchEvent(event));
            event.recycle();
        }
    }

    /** Opens the options of {@code row} the way {@code input} does and returns the sheet, or null when nothing opened. */
    private static com.limelight.ui.ActionSheet openOptionsWith(Library screen, int row, Input input) {
        GridView grid = screen.findViewById(R.id.fragmentView);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        android.view.View card = grid.getChildAt(row - grid.getFirstVisiblePosition());
        assertNotNull("Row " + row + " is on screen", card);
        switch (input) {
            case TOUCH_MORE:
                assertTrue(card.findViewById(R.id.grid_more).performClick());
                break;
            case LONG_PRESS:
                assertTrue(grid.getOnItemLongClickListener().onItemLongClick(grid, card, row, grid.getItemIdAtPosition(row)));
                break;
            case MOUSE_RIGHT_CLICK:
                rightClick(grid, card);
                break;
            default:
                grid.requestFocus();
                grid.setSelection(row);
                Shadows.shadowOf(Looper.getMainLooper()).idle();
                assertEquals(row, grid.getSelectedItemPosition());
                if (input == Input.DPAD_RIGHT_THEN_CENTER) {
                    // Every row here is the last in its line, so Right marks the three dots and Center presses them.
                    press(grid, android.view.KeyEvent.KEYCODE_DPAD_RIGHT, 0);
                    press(grid, android.view.KeyEvent.KEYCODE_DPAD_CENTER, 0);
                } else if (input == Input.MENU_KEY) {
                    press(grid, android.view.KeyEvent.KEYCODE_MENU, 0);
                } else if (input == Input.SHIFT_F10) {
                    press(grid, android.view.KeyEvent.KEYCODE_F10, android.view.KeyEvent.META_SHIFT_ON);
                } else {
                    press(grid, android.view.KeyEvent.KEYCODE_BUTTON_X, 0);
                }
        }
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        android.app.Dialog dialog = ShadowDialog.getLatestDialog();
        return dialog instanceof com.limelight.ui.ActionSheet && dialog.isShowing() ? (com.limelight.ui.ActionSheet) dialog : null;
    }

    @Test
    public void everyInputOpensTheOptionsOfEveryPcState() {
        ActivityController<Library> controller = pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            ComputerDetails running = pc("running", "Den", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED);
            running.runningGameId = 7;
            report(running);
            report(pc("ready", "Office", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            ComputerDetails asleep = pc("asleep", "Attic", ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED);
            asleep.macAddress = "02:00:00:00:00:01";
            report(asleep);
            report(pc("lost", "Loft", ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED));
            GridView grid = screen.findViewById(R.id.fragmentView);
            assertEquals(5, grid.getCount());
            for (int row = 0; row < grid.getCount(); row++) {
                for (Input input : Input.values()) {
                    String where = ((PcView.ComputerObject) grid.getItemAtPosition(row)).details.name + " by " + input;
                    com.limelight.ui.ActionSheet sheet = openOptionsWith(screen, row, input);
                    assertNotNull(where, sheet);
                    java.util.List<String> actions = new java.util.ArrayList<>();
                    collectActions(sheet.findViewById(android.R.id.content), actions);
                    assertTrue(where + ": " + actions, actions.contains(screen.getString(R.string.host_menu_rename)));
                    assertTrue(where + ": " + actions, actions.contains(screen.getString(R.string.pcview_menu_delete_pc)));
                    sheet.dismiss();
                    Shadows.shadowOf(Looper.getMainLooper()).idle();
                    assertFalse(where + " must not also act on the card", screen.hostActions().isPairing());
                }
            }
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static void saveName(Library screen, ComputerDetails computer, String typed) {
        ReflectionHelpers.callInstanceMethod(screen.hostActions(), "saveName",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, computer),
                ReflectionHelpers.ClassParameter.from(String.class, typed));
    }

    private static void showResult(com.limelight.computers.HostActions actions, int generation, boolean paired,
                                   String failure, ComputerDetails computer) {
        ReflectionHelpers.callInstanceMethod(actions, "showPairingResult",
                ReflectionHelpers.ClassParameter.from(int.class, generation),
                ReflectionHelpers.ClassParameter.from(boolean.class, paired),
                ReflectionHelpers.ClassParameter.from(String.class, failure),
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, computer),
                ReflectionHelpers.ClassParameter.from(String.class, null));
    }

    private static void findByText(android.view.View view, String text, java.util.List<android.view.View> found) {
        if (view instanceof TextView && text.contentEquals(((TextView) view).getText())) {
            found.add(view);
        } else if (view instanceof android.view.ViewGroup) {
            android.view.ViewGroup group = (android.view.ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) findByText(group.getChildAt(i), text, found);
        }
    }

    @Test
    public void renamingShowsTheNameEverywhereAndEmptyBringsBackThePcsOwn() {
        ActivityController<Library> controller = pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            report(pc("ready", "Office", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            GridView grid = screen.findViewById(R.id.fragmentView);
            PcView.ComputerObject computer = (PcView.ComputerObject) grid.getItemAtPosition(0);
            assertEquals("Office", computer.details.name);

            com.limelight.ui.ActionSheet sheet = openOptionsWith(screen, 0, Input.LONG_PRESS);
            java.util.List<android.view.View> buttons = new java.util.ArrayList<>();
            findByText(sheet.findViewById(android.R.id.content), screen.getString(R.string.host_menu_rename), buttons);
            assertEquals(1, buttons.size());
            buttons.get(0).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            android.widget.EditText field = dialog.findViewById(R.id.rename_name);
            assertEquals("Office", field.getText().toString());
            field.setText("  Living room PC  ");
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();

            assertEquals("Living room PC", renamed.get("ready"));
            assertEquals("Office", computer.details.name);
            assertEquals("Living room PC", computer.details.displayName());
            java.util.List<android.view.View> labels = new java.util.ArrayList<>();
            findByText(grid.getChildAt(0), "Living room PC", labels);
            assertFalse("The row shows the new name", labels.isEmpty());

            // The empty name, and the PC's own name, both mean no alias.
            saveName(screen, computer.details, "   ");
            assertTrue(renamed.containsKey("ready"));
            assertNull(renamed.get("ready"));
            assertEquals("Office", computer.details.displayName());
            saveName(screen, computer.details, "Den");
            assertEquals("Den", renamed.get("ready"));
            saveName(screen, computer.details, " Office ");
            assertNull(renamed.get("ready"));
            assertEquals("Office", computer.details.displayName());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void removeAsksFirstAndForgetsThePcOnlyAfterTheAnswer() {
        ActivityController<Library> controller = pairingScreen(new CountDownLatch(0), 47989);
        try {
            Library screen = controller.get();
            report(pc("ready", "Office", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED));
            GridView grid = screen.findViewById(R.id.fragmentView);
            PcView.ComputerObject computer = (PcView.ComputerObject) grid.getItemAtPosition(0);
            computer.details.alias = "Den";

            screen.hostActions().confirmRemove(computer.details);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            assertEquals(screen.getString(R.string.delete_pc_msg),
                    ((TextView) dialog.findViewById(android.R.id.message)).getText().toString());
            assertEquals(screen.getString(R.string.pcview_menu_delete_pc),
                    dialog.getButton(AlertDialog.BUTTON_POSITIVE).getText().toString());
            dialog.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            assertTrue(removed.isEmpty());
            assertEquals(2, grid.getCount());

            screen.hostActions().confirmRemove(computer.details);
            ((AlertDialog) ShadowDialog.getLatestDialog()).getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(java.util.List.of(computer.details), removed);
            assertEquals(1, grid.getCount());
            assertEquals("PC", ((PcView.ComputerObject) grid.getItemAtPosition(0)).details.name);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void aCancelledPairingNeverShowsItsResult() throws Exception {
        ActivityController<Library> controller = pairingScreen(new CountDownLatch(1), 47989);
        Thread worker = null;
        try {
            Library screen = controller.get();
            com.limelight.computers.HostActions actions = screen.hostActions();
            GridView grid = screen.findViewById(R.id.fragmentView);
            ComputerDetails computer = ((PcView.ComputerObject) grid.getItemAtPosition(0)).details;
            grid.performItemClick(null, 0, 0);
            worker = ReflectionHelpers.getField(actions, "pairingThread");
            int generation = ReflectionHelpers.getField(actions, "pairingGeneration");
            assertTrue(actions.isPairing());

            actions.cancelPairing();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            worker.join(5000);
            assertFalse(actions.isPairing());
            android.app.Dialog before = ShadowDialog.getLatestDialog();

            // The cancelled attempt reports late, on screen and while the screen is paused.
            showResult(actions, generation, true, null, computer);
            showResult(actions, generation, false, "late failure", computer);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertSame(before, ShadowDialog.getLatestDialog());
            controller.pause();
            showResult(actions, generation, false, "late failure", computer);
            assertFalse(actions.deliverPendingResult());
            controller.resume();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertSame(before, ShadowDialog.getLatestDialog());
            assertNull(ReflectionHelpers.getField(actions, "pendingPairingResult"));
        } finally {
            controller.pause().stop().destroy();
            if (worker != null) worker.join(5000);
        }
    }

    @Test
    public void resizingKeepsTheLibraryViewsAndKeyboardFocus() {
        Application application = RuntimeEnvironment.getApplication();
        GlPreferences gl = GlPreferences.readPreferences(application);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        application.getSharedPreferences("FirstRun", 0).edit().putBoolean("pairing_guide_shown", true).commit();
        ActivityController<Library> controller = Robolectric.buildActivity(Library.class).setup();
        try {
            android.view.View add = controller.get().findViewById(R.id.discovery_add);
            add.requestFocus();
            android.content.res.Configuration config = new android.content.res.Configuration(application.getResources().getConfiguration());
            config.orientation = android.content.res.Configuration.ORIENTATION_LANDSCAPE;
            controller.get().onConfigurationChanged(config);
            assertSame(add, controller.get().findViewById(R.id.discovery_add));
            assertTrue(add.hasFocus());
        } finally {
            controller.pause().stop().destroy();
        }
    }
}
