package com.limelight.computers;

import static org.junit.Assert.assertEquals;

import android.app.Application;

import com.limelight.R;
import com.limelight.computers.HostMenu.SleepEntry;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

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
}
