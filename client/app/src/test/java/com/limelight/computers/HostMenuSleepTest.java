package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.os.Looper;
import android.view.View;
import android.view.ViewGroup;

import com.limelight.R;
import com.limelight.computers.HostMenu.SleepEntry;
import com.limelight.grid.PcGridAdapter;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.ui.ActionSheet;
import com.limelight.ui.ToolbarIconTintTest;

import com.google.android.material.button.MaterialButton;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;

/** When a PC's menu offers Sleep: by the host's advertised power actions, never by its brand. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostMenuSleepTest {
    private static final String HEAD = "<root status_code=\"200\"><hostname>VP</hostname><appversion>7.1.431.-1</appversion>"
            + "<uniqueid>vp</uniqueid><HttpsPort>47984</HttpsPort><PairStatus>1</PairStatus><currentgame>0</currentgame>"
            + "<state>SUNSHINE_SERVER_FREE</state>";

    private static String power(int supported, int allowed) {
        return HEAD + "<PowerActionVersion>1</PowerActionVersion><SleepSupported>" + supported
                + "</SleepSupported><SleepAllowed>" + allowed + "</SleepAllowed></root>";
    }

    private static ComputerDetails pc(String serverInfo, ComputerDetails.State state) {
        ComputerDetails details = new ComputerDetails();
        details.uuid = "pc";
        details.name = "PC";
        details.state = state;
        details.caps = serverInfo == null ? HostCapabilities.UNKNOWN : TestHosts.caps(serverInfo);
        return details;
    }

    @Test
    public void anOnlineHostThatOffersSleepShowsIt() {
        assertEquals(SleepEntry.ENABLED, HostMenu.sleepEntry(pc(power(1, 1), ComputerDetails.State.ONLINE)));
        assertEquals(SleepEntry.ENABLED, HostMenu.sleepEntry(pc(HEAD.replace("VP", "RL")
                + "<RustHostVersion>3.0.0</RustHostVersion><PowerActionVersion>1</PowerActionVersion>"
                + "<SleepSupported>1</SleepSupported><SleepAllowed>1</SleepAllowed></root>", ComputerDetails.State.ONLINE)));
    }

    @Test
    public void anOfflineOrUnknownStatePcHasNothingToPutToSleep() {
        assertEquals(SleepEntry.HIDDEN, HostMenu.sleepEntry(pc(power(1, 1), ComputerDetails.State.OFFLINE)));
        assertEquals(SleepEntry.HIDDEN, HostMenu.sleepEntry(pc(power(1, 1), ComputerDetails.State.UNKNOWN)));
    }

    @Test
    public void aHostThatWithholdsSleepFromThisDeviceShowsItDisabledWithTheReason() {
        ComputerDetails details = pc(power(1, 0), ComputerDetails.State.ONLINE);
        assertEquals(SleepEntry.DISABLED, HostMenu.sleepEntry(details));
        assertEquals(R.string.host_feature_reason_sleep_permission,
                details.caps.availability(HostCapabilities.Feature.SLEEP).reasonText());
    }

    @Test
    public void aHostThatCannotSleepHidesIt() {
        assertEquals(SleepEntry.HIDDEN, HostMenu.sleepEntry(pc(power(0, 1), ComputerDetails.State.ONLINE)));
        assertEquals(SleepEntry.HIDDEN, HostMenu.sleepEntry(pc(power(0, 0), ComputerDetails.State.ONLINE)));
        // Unpaired: the host answers power actions only to paired clients.
        assertEquals(SleepEntry.HIDDEN, HostMenu.sleepEntry(pc(power(1, 1).replace("<PairStatus>1", "<PairStatus>0"),
                ComputerDetails.State.ONLINE)));
    }

    @Test
    public void stockAndOlderHostsWithoutPowerActionsNeverShowIt() {
        assertEquals(SleepEntry.HIDDEN, HostMenu.sleepEntry(pc(HEAD + "</root>", ComputerDetails.State.ONLINE)));
        assertEquals(SleepEntry.HIDDEN, HostMenu.sleepEntry(pc(TestHosts.serverInfo("sunshine-serverinfo.xml"),
                ComputerDetails.State.ONLINE)));
        // Rubylight before Sleep PC: an update would add it, but the menu offers no dead entry.
        assertEquals(SleepEntry.HIDDEN, HostMenu.sleepEntry(pc(TestHosts.serverInfo("rubylight-serverinfo.xml"),
                ComputerDetails.State.ONLINE)));
        assertEquals(SleepEntry.HIDDEN, HostMenu.sleepEntry(pc(null, ComputerDetails.State.ONLINE)));
    }

    private static MaterialButton sleepRow(ViewGroup group, String label) {
        for (int i = 0; i < group.getChildCount(); i++) {
            View child = group.getChildAt(i);
            if (child instanceof MaterialButton && ((MaterialButton) child).getText().toString().startsWith(label)) {
                return (MaterialButton) child;
            }
            if (child instanceof ViewGroup) {
                MaterialButton found = sleepRow((ViewGroup) child, label);
                if (found != null) {
                    return found;
                }
            }
        }
        return null;
    }

    @Test
    public void aWithheldSleepRowIsDimmedYetReachableAndAsksTheHostAgain() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            String label = screen.getString(R.string.host_menu_sleep);
            List<Integer> chosen = new ArrayList<>();

            ActionSheet withheld = HostMenu.build(screen, pc(power(1, 0), ComputerDetails.State.ONLINE),
                    PcGridAdapter.RowState.READY, null, chosen::add);
            withheld.show();
            MaterialButton row = sleepRow((ViewGroup) withheld.findViewById(android.R.id.content), label);
            assertEquals(label + "\n" + screen.getString(R.string.host_feature_reason_sleep_permission),
                    row.getText().toString());
            assertTrue("D-pad and TalkBack reach it", row.isEnabled() && row.isFocusable() && row.isClickable());
            assertEquals(HostMenu.DISABLED_ALPHA, row.getAlpha(), 0f);
            row.performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(List.of(HostMenu.SLEEP), chosen);

            ActionSheet allowed = HostMenu.build(screen, pc(power(1, 1), ComputerDetails.State.ONLINE),
                    PcGridAdapter.RowState.READY, null, chosen::add);
            allowed.show();
            assertEquals(1f, sleepRow((ViewGroup) allowed.findViewById(android.R.id.content), label).getAlpha(), 0f);
            allowed.dismiss();
        }
    }

    @Test
    public void anAwayPcWithOnlyTheMacsItsWakeListedOffersWake() throws Exception {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            new WakeTargets(screen).store("pc", NativeApiFixtures.wake("02:e0:4c:68:00:01"));
            List<Integer> chosen = new ArrayList<>();
            ActionSheet menu = HostMenu.build(screen, pc(power(1, 1), ComputerDetails.State.OFFLINE),
                    PcGridAdapter.RowState.OFFLINE, null, chosen::add);
            menu.show();
            MaterialButton wake = sleepRow((ViewGroup) menu.findViewById(android.R.id.content),
                    screen.getString(R.string.pcview_menu_send_wol));
            wake.performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(List.of(HostMenu.WAKE), chosen);
        }
    }
}
