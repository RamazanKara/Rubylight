package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
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
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nvstream.http.ComputerDetails;
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
 * Microphone only in a PC's menu: only on a ready native PC whose capabilities, as last read, list
 * "session.mic_only" and let this device start sessions. Stock hosts never show it.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostMenuMicOnlyTest {
    private static ComputerDetails pc(String uuid) {
        ComputerDetails details = new ComputerDetails();
        details.uuid = uuid;
        details.name = "PC";
        details.state = ComputerDetails.State.ONLINE;
        details.caps = TestHosts.caps("<root status_code=\"200\"><RustHostVersion>3.0.0</RustHostVersion>"
                + "<PairStatus>1</PairStatus><NativeApiVersion>1</NativeApiVersion></root>");
        return details;
    }

    private static String capabilities(String features, boolean launch) {
        return "{\"ok\":{\"api_version\":1,\"host\":{\"name\":\"PC\",\"version\":\"3.0.0\",\"unique_id\":\"U\"},"
                + "\"features\":[" + features + "],\"permissions\":{\"list\":true,\"view\":true,\"launch\":" + launch
                + ",\"host_control\":false}}}";
    }

    /** Reads the capabilities the way the PC list's poll does, so the registry learns them. */
    private static void read(String uuid, String envelope) throws Exception {
        new NativeApiClient((host, op, argument) -> envelope,
                new HostEndpoint(uuid, "10.0.0.2", 47989, 47984, "pem", "unique", new byte[1], new byte[1]))
                .capabilities();
    }

    @Before
    public void forget() {
        KnownCapabilities.clear();
    }

    @Test
    public void aReadyPcOfferingItThatLetsThisDeviceLaunchShowsTheEntry() throws Exception {
        read("pc", capabilities("\"library\",\"session.mic_only\"", true));
        assertTrue(HostMenu.offersMicOnly(pc("pc"), PcGridAdapter.RowState.READY));
        assertTrue(HostMenu.offersMicOnly(pc("pc"), PcGridAdapter.RowState.RUNNING));
    }

    @Test
    public void withoutLaunchAppsOrTheFeatureItIsHidden() throws Exception {
        // Not read in this process: the entry waits for the poll rather than guess.
        assertFalse(HostMenu.offersMicOnly(pc("pc"), PcGridAdapter.RowState.READY));
        read("pc", capabilities("\"session.mic_only\"", false));
        assertFalse(HostMenu.offersMicOnly(pc("pc"), PcGridAdapter.RowState.READY));
        read("pc", capabilities("\"library\"", true));
        assertFalse(HostMenu.offersMicOnly(pc("pc"), PcGridAdapter.RowState.READY));
    }

    @Test
    public void anOfflineUnpairedOrStockPcDoesNot() throws Exception {
        read("pc", capabilities("\"session.mic_only\"", true));
        assertFalse(HostMenu.offersMicOnly(pc("pc"), PcGridAdapter.RowState.OFFLINE));
        assertFalse(HostMenu.offersMicOnly(pc("pc"), PcGridAdapter.RowState.NOT_PAIRED));
        read("sunshine", capabilities("\"session.mic_only\"", true));
        ComputerDetails sunshine = new ComputerDetails();
        sunshine.uuid = "sunshine";
        sunshine.caps = TestHosts.fixture("sunshine-serverinfo.xml");
        assertFalse(HostMenu.offersMicOnly(sunshine, PcGridAdapter.RowState.READY));
    }

    private static MaterialButton row(View view, String label) {
        if (view instanceof MaterialButton && ((MaterialButton) view).getText().toString().equals(label)) {
            return (MaterialButton) view;
        }
        if (view instanceof ViewGroup) {
            for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
                MaterialButton found = row(((ViewGroup) view).getChildAt(i), label);
                if (found != null) return found;
            }
        }
        return null;
    }

    @Test
    public void theEntryChoosesMicrophoneOnly() throws Exception {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            String label = screen.getString(R.string.mic_only_action);
            List<Integer> chosen = new ArrayList<>();
            ActionSheet without = HostMenu.build(screen, pc("pc"), PcGridAdapter.RowState.READY, null, chosen::add);
            without.show();
            assertNull(row(without.findViewById(android.R.id.content), label));
            without.dismiss();

            read("pc", capabilities("\"session.mic_only\"", true));
            ActionSheet menu = HostMenu.build(screen, pc("pc"), PcGridAdapter.RowState.READY, null, chosen::add);
            menu.show();
            MaterialButton entry = row(menu.findViewById(android.R.id.content), label);
            assertNotNull(entry);
            assertEquals("Microphone only", entry.getText().toString());
            entry.performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(List.of(HostMenu.MIC_ONLY), chosen);
        }
    }
}
