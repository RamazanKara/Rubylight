package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.os.Looper;
import android.view.View;
import android.view.ViewGroup;

import com.limelight.R;
import com.limelight.grid.PcGridAdapter;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.KnownCapabilities;
import com.limelight.nativeapi.KnownUpdates;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.ui.ActionSheet;
import com.limelight.ui.ToolbarIconTintTest;

import com.google.android.material.button.MaterialButton;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;

/**
 * Restart PC, Shut down PC and Update the PC's host in a PC's menu: only where the host's capabilities, as last read,
 * list power.restart, power.shutdown and update. Without the grant, Restart PC alone is shown, dimmed, with the
 * reason, as on desktop; the update only with Host control and a newer release to install.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostMenuPcPowerTest {
    private static final HostCapabilities.Feature RESTART = HostCapabilities.Feature.PC_RESTART;
    private static final HostCapabilities.Feature SHUTDOWN = HostCapabilities.Feature.PC_SHUTDOWN;
    private static final PcGridAdapter.RowState READY = PcGridAdapter.RowState.READY;

    private static ComputerDetails pc() {
        ComputerDetails details = new ComputerDetails();
        details.uuid = "pc";
        details.name = "PC";
        details.state = ComputerDetails.State.ONLINE;
        details.caps = TestHosts.caps("<root status_code=\"200\"><RustHostVersion>3.3.0</RustHostVersion>"
                + "<PairStatus>1</PairStatus><NativeApiVersion>1</NativeApiVersion></root>");
        return details;
    }

    private static String capabilities(String features, boolean restartShutdown, boolean hostControl) {
        return "{\"ok\":{\"api_version\":1,\"host\":{\"name\":\"PC\",\"version\":\"3.3.0\",\"unique_id\":\"U\"},"
                + "\"features\":[" + features + "],\"permissions\":{\"list\":true,\"view\":true,\"launch\":true,"
                + "\"host_control\":" + hostControl + ",\"restart_shutdown\":" + restartShutdown + "}}}";
    }

    private static NativeApiClient api(String envelope) {
        return new NativeApiClient((host, op, argument) -> envelope,
                new HostEndpoint("pc", "10.0.0.2", 47989, 47984, "pem", "unique", new byte[1], new byte[1]));
    }

    /** Reads the capabilities the way the PC list's poll does, so the menu learns them. */
    private static void read(String envelope) throws Exception {
        api(envelope).capabilities();
    }

    private static void readUpdate(String latest, boolean supported) throws Exception {
        api("{\"ok\":{\"current_version\":\"3.3.0\",\"latest_version\":" + (latest == null ? "null" : "\"" + latest + "\"")
                + ",\"phase\":\"idle\",\"install_supported\":" + supported + ",\"error\":null}}").hostUpdate();
    }

    @Before
    public void forget() {
        KnownCapabilities.clear();
        KnownUpdates.clear();
    }

    @Test
    public void aHostOfferingThemToThisDeviceShowsBoth() throws Exception {
        read(capabilities("\"power.restart\",\"power.shutdown\"", true, false));
        assertEquals(HostMenu.SleepEntry.ENABLED, HostMenu.pcPowerEntry(pc(), READY, RESTART));
        assertEquals(HostMenu.SleepEntry.ENABLED, HostMenu.pcPowerEntry(pc(), PcGridAdapter.RowState.RUNNING, SHUTDOWN));
    }

    @Test
    public void withoutTheGrantBothAreWithheldWithTheReason() throws Exception {
        read(capabilities("\"power.restart\",\"power.shutdown\"", false, true));
        assertEquals(HostMenu.SleepEntry.DISABLED, HostMenu.pcPowerEntry(pc(), READY, RESTART));
        assertEquals(HostMenu.SleepEntry.DISABLED, HostMenu.pcPowerEntry(pc(), READY, SHUTDOWN));
    }

    @Test
    public void anUnreadOfflineOrOlderHostShowsNeither() throws Exception {
        // Not read in this process: the entries wait for the poll rather than guess.
        assertEquals(HostMenu.SleepEntry.HIDDEN, HostMenu.pcPowerEntry(pc(), READY, RESTART));
        read(capabilities("\"power.restart\",\"power.shutdown\"", true, true));
        assertEquals(HostMenu.SleepEntry.HIDDEN, HostMenu.pcPowerEntry(pc(), PcGridAdapter.RowState.OFFLINE, RESTART));
        assertEquals(HostMenu.SleepEntry.HIDDEN, HostMenu.pcPowerEntry(pc(), PcGridAdapter.RowState.NOT_PAIRED, SHUTDOWN));
        // A host before them lists neither feature; its grant field is missing too.
        read(capabilities("\"library\"", false, true));
        assertEquals(HostMenu.SleepEntry.HIDDEN, HostMenu.pcPowerEntry(pc(), READY, RESTART));
        assertEquals(HostMenu.SleepEntry.HIDDEN, HostMenu.pcPowerEntry(pc(), READY, SHUTDOWN));
    }

    @Test
    public void theUpdateNeedsTheFeatureHostControlAndANewerRelease() throws Exception {
        readUpdate("v3.4.0", true);
        read(capabilities("\"update\"", false, true));
        assertEquals("3.4.0", HostMenu.updateOffer(pc(), READY));
        assertNull(HostMenu.updateOffer(pc(), PcGridAdapter.RowState.OFFLINE));
        read(capabilities("\"update\"", false, false));
        assertNull("Installing takes Host control", HostMenu.updateOffer(pc(), READY));
        read(capabilities("\"library\"", false, true));
        assertNull(HostMenu.updateOffer(pc(), READY));
        read(capabilities("\"update\"", false, true));
        readUpdate("3.3.0", true);
        assertNull("Nothing newer", HostMenu.updateOffer(pc(), READY));
        readUpdate("v3.4.0", false);
        assertNull("Not an installed service", HostMenu.updateOffer(pc(), READY));
        readUpdate(null, true);
        assertNull(HostMenu.updateOffer(pc(), READY));
    }

    private static List<MaterialButton> rows(View view, List<MaterialButton> into) {
        if (view instanceof MaterialButton) {
            into.add((MaterialButton) view);
        } else if (view instanceof ViewGroup) {
            for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
                rows(((ViewGroup) view).getChildAt(i), into);
            }
        }
        return into;
    }

    private static MaterialButton row(ActionSheet sheet, String label) {
        for (MaterialButton button : rows(sheet.findViewById(android.R.id.content), new ArrayList<>())) {
            if (button.getText().toString().startsWith(label)) return button;
        }
        return null;
    }

    @Test
    public void theMenuListsThemAndEachChoosesItsAction() throws Exception {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            String restart = screen.getString(R.string.host_menu_restart);
            String shutdown = screen.getString(R.string.host_menu_shutdown);
            String update = screen.getString(R.string.host_menu_update);
            List<Integer> chosen = new ArrayList<>();

            read(capabilities("\"power.restart\",\"power.shutdown\",\"update\"", true, true));
            readUpdate("v3.4.0", true);
            ActionSheet menu = HostMenu.build(screen, pc(), READY, null, chosen::add);
            menu.show();
            MaterialButton[] entries = {row(menu, restart), row(menu, shutdown), row(menu, update)};
            for (MaterialButton entry : entries) {
                assertNotNull(entry);
                assertEquals(1f, entry.getAlpha(), 0f);
            }
            assertEquals("Update the PC’s host", entries[2].getText().toString());
            for (MaterialButton entry : entries) {
                entry.performClick();
            }
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(List.of(HostMenu.RESTART, HostMenu.SHUT_DOWN, HostMenu.UPDATE_HOST), chosen);
            menu.dismiss();
        }
    }

    @Test
    public void withoutTheGrantOnlyADimmedRestartWithTheReasonIsListed() throws Exception {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            List<Integer> chosen = new ArrayList<>();
            read(capabilities("\"power.restart\",\"power.shutdown\"", false, false));
            ActionSheet menu = HostMenu.build(screen, pc(), READY, null, chosen::add);
            menu.show();
            MaterialButton restart = row(menu, screen.getString(R.string.host_menu_restart));
            assertNotNull(restart);
            assertNull(row(menu, screen.getString(R.string.host_menu_shutdown)));
            assertEquals(HostMenu.DISABLED_ALPHA, restart.getAlpha(), 0f);
            String reason = screen.getString(R.string.host_feature_reason_restart_shutdown_permission);
            assertTrue("The reason is shown", containsText(menu.findViewById(android.R.id.content), reason));
            // Still reachable: choosing it asks the host again.
            restart.performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(List.of(HostMenu.RESTART), chosen);
        }
    }

    private static boolean containsText(View view, String text) {
        if (view instanceof android.widget.TextView
                && ((android.widget.TextView) view).getText().toString().contains(text)) {
            return true;
        }
        if (view instanceof ViewGroup) {
            for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
                if (containsText(((ViewGroup) view).getChildAt(i), text)) return true;
            }
        }
        return false;
    }
}
