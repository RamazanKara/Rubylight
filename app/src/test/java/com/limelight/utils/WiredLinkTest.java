package com.limelight.utils;

import com.limelight.utils.WiredLink.Route;

import org.junit.Test;

import java.util.Arrays;
import java.util.Collections;

import static org.junit.Assert.assertEquals;

public class WiredLinkTest {
    private static Route ethernet(String name, int prefix, boolean active) {
        return new Route(name, true, false, active, prefix);
    }

    private static Route wifi(int prefix, boolean active) {
        return new Route("wlan0", false, false, active, prefix);
    }

    @Test public void speedFileHoldsMegabitsPerSecond() {
        assertEquals(1000, WiredLink.parseMbps("1000\n"));
        assertEquals(2500, WiredLink.parseMbps(" 2500 "));
        assertEquals(10000, WiredLink.parseMbps("10000"));
    }

    @Test public void unknownOrBrokenSpeedsAreZero() {
        assertEquals(0, WiredLink.parseMbps("-1"));
        assertEquals(0, WiredLink.parseMbps("0"));
        assertEquals(0, WiredLink.parseMbps(""));
        assertEquals(0, WiredLink.parseMbps(null));
        assertEquals(0, WiredLink.parseMbps("unknown"));
        assertEquals(0, WiredLink.parseMbps("99999999999"));
        assertEquals(0, WiredLink.parseMbps("2000000"));
    }

    @Test public void ethernetRouteReportsItsInterfaceSpeed() {
        assertEquals(2500, WiredLink.mbpsToward(Collections.singletonList(ethernet("eth0", 24, true)),
                name -> name.equals("eth0") ? 2500 : -7));
    }

    @Test public void wifiCellularAndNoRouteGiveNoHint() {
        WiredLink.SpeedSource fast = name -> 1000;
        assertEquals(0, WiredLink.mbpsToward(Collections.singletonList(wifi(24, true)), fast));
        assertEquals(0, WiredLink.mbpsToward(Collections.<Route>emptyList(), fast));
    }

    @Test public void theMostSpecificRouteDecides() {
        WiredLink.SpeedSource fast = name -> 1000;
        // Ethernet and Wi-Fi both on the host's subnet: the active network wins the tie.
        assertEquals(1000, WiredLink.mbpsToward(Arrays.asList(wifi(24, false), ethernet("eth0", 24, true)), fast));
        assertEquals(0, WiredLink.mbpsToward(Arrays.asList(ethernet("eth0", 24, false), wifi(24, true)), fast));
        // A subnet route on Wi-Fi beats Ethernet's default route, and the other way round.
        assertEquals(0, WiredLink.mbpsToward(Arrays.asList(ethernet("eth0", 0, true), wifi(24, false)), fast));
        assertEquals(1000, WiredLink.mbpsToward(Arrays.asList(wifi(0, true), ethernet("eth0", 24, false)), fast));
    }

    @Test public void vpnOrOddInterfaceNamesGiveNoHint() {
        WiredLink.SpeedSource fast = name -> 1000;
        assertEquals(0, WiredLink.mbpsToward(Collections.singletonList(new Route("eth0", true, true, true, 0)), fast));
        assertEquals(0, WiredLink.mbpsToward(Collections.singletonList(ethernet("../eth0", 24, true)), fast));
        assertEquals(0, WiredLink.mbpsToward(Collections.singletonList(ethernet(null, 24, true)), fast));
        assertEquals(0, WiredLink.mbpsToward(Collections.singletonList(ethernet("eth0", 24, true)), name -> 0));
    }
}
