package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
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
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }
    }

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
            @Override public void startPolling(ComputerManagerListener listener) { }
            @Override public void stopPolling() { }
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

    /** The PC sheet's actions as "label" or "label (disabled)", for one PC with these capabilities. */
    private java.util.List<String> pcSheet(com.limelight.nvstream.http.HostCapabilities caps) {
        ActivityController<Library> controller = pairingScreen(new CountDownLatch(0), 47989);
        try {
            com.limelight.grid.PcGridAdapter adapter = ReflectionHelpers.getField(controller.get(), "pcGridAdapter");
            PcView.ComputerObject computer = (PcView.ComputerObject) adapter.getItem(0);
            computer.details.caps = caps;
            ReflectionHelpers.callInstanceMethod(controller.get(), "showComputerMenu",
                    ReflectionHelpers.ClassParameter.from(PcView.ComputerObject.class, computer));
            com.limelight.ui.ActionSheet sheet = ReflectionHelpers.getField(controller.get(), "pcMenu");
            java.util.List<String> actions = new java.util.ArrayList<>();
            collectActions(sheet.findViewById(android.R.id.content), actions);
            sheet.dismiss();
            return actions;
        } finally {
            controller.pause().stop().destroy();
        }
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

    @Test
    public void pcSheetOffersHostFeaturesOnlyWhereThePcHasThem() {
        android.content.Context context = RuntimeEnvironment.getApplication();
        String display = context.getString(R.string.host_display_menu);
        String otp = context.getString(R.string.pair_otp_title);
        String webUi = context.getString(R.string.host_administration_menu);
        java.util.List<String> sunshine = pcSheet(com.limelight.nvstream.http.TestHosts.fixture("sunshine-serverinfo.xml"));
        assertFalse(sunshine.toString(), sunshine.stream().anyMatch(action -> action.startsWith(display)));
        assertFalse(sunshine.contains(otp));
        assertTrue(sunshine.toString(), sunshine.contains(webUi));
        java.util.List<String> apollo = pcSheet(com.limelight.nvstream.http.TestHosts.fixture("apollo-http-serverinfo.xml"));
        assertTrue(apollo.toString(), apollo.contains(otp));
        assertFalse(apollo.stream().anyMatch(action -> action.startsWith(display)));
        java.util.List<String> gameStream = pcSheet(com.limelight.nvstream.http.TestHosts.fixture("gamestream-serverinfo.xml"));
        assertFalse(gameStream.stream().anyMatch(action -> action.startsWith(display)));
        assertFalse(gameStream.contains(webUi));
        assertTrue(pcSheet(com.limelight.nvstream.http.TestHosts.rubylight()).contains(display));
        java.util.List<String> older = pcSheet(com.limelight.nvstream.http.TestHosts.fixture("butterpollo-serverinfo.xml"));
        assertTrue(older.toString(), older.contains(
                display + "\n" + context.getString(R.string.host_feature_reason_update) + " (disabled)"));
        assertTrue(pcSheet(com.limelight.nvstream.http.HostCapabilities.UNKNOWN).contains(
                display + "\n" + context.getString(R.string.host_feature_unknown)));
    }

    @Test
    public void hostAdministrationExplainsLoginThenOpensTheCurrentHostsConsole() {
        ActivityController<Library> controller = pairingScreen(new CountDownLatch(0), 47989);
        try {
            ComputerDetails computer = new ComputerDetails();
            computer.state = ComputerDetails.State.ONLINE;
            computer.caps = com.limelight.nvstream.http.TestHosts.rubylight();
            computer.activeAddress = new ComputerDetails.AddressTuple("2001:db8::1", 50000);
            computer.httpsPort = 49995;
            ReflectionHelpers.callInstanceMethod(controller.get(), "showHostAdministration",
                    ReflectionHelpers.ClassParameter.from(ComputerDetails.class, computer));
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            TextView message = dialog.findViewById(android.R.id.message);
            assertEquals(controller.get().getString(R.string.host_administration_help,
                    "https://[2001:db8::1]:50001/"), message.getText().toString());
            assertTrue(message.isTextSelectable());
            assertNull(Shadows.shadowOf(controller.get()).getNextStartedActivity());
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            Intent browser = Shadows.shadowOf(controller.get()).getNextStartedActivity();
            assertEquals(Intent.ACTION_VIEW, browser.getAction());
            assertEquals("https://[2001:db8::1]:50001/", browser.getDataString());
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
            worker = ReflectionHelpers.getField(screen, "pairingThread");
            AlertDialog dialog = ReflectionHelpers.getField(screen, "pairingDialog");
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
            assertNull(ReflectionHelpers.getField(screen, "pairingDialog"));
            assertNull(ReflectionHelpers.getField(screen, "pairingRequest"));
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
                worker = ReflectionHelpers.getField(screen, "pairingThread");
                AlertDialog pinDialog;
                try (Socket connection = host.accept()) {
                    Shadows.shadowOf(Looper.getMainLooper()).idle();
                    pinDialog = ReflectionHelpers.getField(screen, "pairingDialog");
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
                worker = ReflectionHelpers.getField(screen, "pairingThread");
                try (Socket connection = host.accept()) {
                    AlertDialog retry = ReflectionHelpers.getField(screen, "pairingDialog");
                    assertNotNull(retry);
                    assertTrue(retry.isShowing());
                    controller.pause();
                    worker.join(5000);
                    Shadows.shadowOf(Looper.getMainLooper()).idle();
                    assertFalse(worker.isAlive());
                    assertFalse(retry.isShowing());
                    assertNull(ReflectionHelpers.getField(screen, "pairingDialog"));
                    assertNull(ReflectionHelpers.getField(screen, "pairingRequest"));
                    controller.stop().destroy();
                    controller = null;
                }
            } finally {
                if (controller != null) controller.pause().stop().destroy();
                if (worker != null) worker.join(5000);
            }
        }
    }

    @Test
    public void frontendExportRetainsItsHostAndFirstFolderAcrossRecreation() {
        Application application = RuntimeEnvironment.getApplication();
        GlPreferences gl = GlPreferences.readPreferences(application);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        application.getSharedPreferences("FirstRun", 0).edit().putBoolean("pairing_guide_shown", true).commit();
        ActivityController<Library> first = Robolectric.buildActivity(Library.class).create().start().resume();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "host-uuid";
        computer.name = "PC";
        Uri folder = Uri.parse("content://documents/tree/roms");
        ReflectionHelpers.setField(first.get(), "exportComputer", computer);
        ReflectionHelpers.setField(first.get(), "exportRomsTree", folder);
        Bundle saved = new Bundle();
        first.pause().saveInstanceState(saved).stop().destroy();

        ActivityController<Library> restored = Robolectric.buildActivity(Library.class).create(saved).start().resume();
        try {
            ComputerDetails restoredComputer = ReflectionHelpers.getField(restored.get(), "exportComputer");
            assertNotNull(restoredComputer);
            assertEquals(computer.uuid, restoredComputer.uuid);
            assertEquals(computer.name, restoredComputer.name);
            assertEquals(folder, ReflectionHelpers.getField(restored.get(), "exportRomsTree"));
        } finally {
            restored.pause().stop().destroy();
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
