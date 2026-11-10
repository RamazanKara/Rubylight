package com.limelight.console;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;

import org.junit.Test;

import java.util.Arrays;
import java.util.Collections;
import java.util.List;

/**
 * A poll reports every PC whether or not it changed. The console's screens reload only when one brings news, and
 * must never miss any: that every field of a PC counts is ComputerDetailsFieldsTest's to say.
 */
public class HostsDrawnTest {
    private long now = 1_000_000_000_000L;
    private final HostsDrawn drawn = new HostsDrawn(() -> now);

    /** A PC as one poll reports it; every call is a new copy, as the feed's are. */
    private static ComputerDetails pc(String uuid) {
        ComputerDetails details = new ComputerDetails();
        details.uuid = uuid;
        details.name = "PC " + uuid;
        details.state = ComputerDetails.State.ONLINE;
        details.pairState = PairingManager.PairState.PAIRED;
        details.activeAddress = new ComputerDetails.AddressTuple("10.0.0.2", 47989);
        return details;
    }

    private static List<ComputerDetails> two() {
        return Arrays.asList(pc("tower"), pc("den"));
    }

    @Test
    public void beforeAnythingIsDrawnEveryPollIsNews() {
        assertTrue(drawn.changed(two(), false));
        assertTrue(drawn.changed(Collections.emptyList(), false));
    }

    @Test
    public void aPollThatReportsTheSamePcsIsNoNews() {
        drawn.set(two(), false);
        assertFalse(drawn.changed(two(), false));
        assertFalse(drawn.changed(two(), false));
    }

    @Test
    public void whatAScreenShowsOfAPcIsNews() {
        drawn.set(two(), false);
        List<ComputerDetails> hosts = two();
        hosts.get(1).state = ComputerDetails.State.OFFLINE;
        assertTrue("it went away", drawn.changed(hosts, false));

        hosts = two();
        hosts.get(0).runningGameId = 7;
        assertTrue("it started a game", drawn.changed(hosts, false));

        hosts = two();
        hosts.get(0).runningGameUuid = "a-game";
        assertTrue("a game hidden behind id 0", drawn.changed(hosts, false));

        hosts = two();
        hosts.get(0).alias = "Den PC";
        assertTrue("it was renamed here", drawn.changed(hosts, false));

        hosts = two();
        hosts.get(0).pairState = PairingManager.PairState.NOT_PAIRED;
        assertTrue("it was unpaired", drawn.changed(hosts, false));

        hosts = two();
        hosts.get(0).activeAddress = new ComputerDetails.AddressTuple("203.0.113.5", 47989);
        assertTrue("it answers at another address", drawn.changed(hosts, false));

        hosts = two();
        hosts.get(0).permission = ComputerDetails.PERMISSION_VIEW;
        assertTrue("it may no longer be launched on", drawn.changed(hosts, false));

        hosts = two();
        hosts.get(0).rawAppList = "<root/>";
        assertTrue("its app list was read", drawn.changed(hosts, false));
    }

    @Test
    public void aPcThatComesGoesOrMovesIsNews() {
        drawn.set(two(), false);
        assertTrue(drawn.changed(Collections.singletonList(pc("tower")), false));
        assertTrue(drawn.changed(Arrays.asList(pc("tower"), pc("den"), pc("attic")), false));
        assertTrue(drawn.changed(Arrays.asList(pc("den"), pc("tower")), false));
        assertTrue(drawn.changed(Collections.emptyList(), false));
    }

    @Test
    public void theEndOfTheWaitForSavedPcsIsNews() {
        drawn.set(Collections.emptyList(), true);
        assertFalse(drawn.changed(Collections.emptyList(), true));
        assertTrue("Home may now say there are no PCs", drawn.changed(Collections.emptyList(), false));
    }

    @Test
    public void aPcChangedInPlaceAfterTheDrawIsNews() {
        List<ComputerDetails> hosts = two();
        drawn.set(hosts, false);
        hosts.get(0).state = ComputerDetails.State.OFFLINE;
        assertTrue(drawn.changed(hosts, false));
    }

    @Test
    public void theClockMovingOnRedrawsWhatSaysHowLongAgo() {
        now -= now % HostsDrawn.CLOCK_STEP_MS;
        drawn.set(two(), false);
        now += HostsDrawn.CLOCK_STEP_MS - 1;
        assertFalse(drawn.changed(two(), false));
        now += 1;
        assertTrue(drawn.changed(two(), false));
        drawn.set(two(), false);
        assertFalse(drawn.changed(two(), false));
    }

    @Test
    public void afterForgetTheNextPollIsNewsOnce() {
        drawn.set(two(), false);
        drawn.forget();
        assertTrue(drawn.changed(two(), false));
        drawn.set(two(), false);
        assertFalse(drawn.changed(two(), false));
    }
}
