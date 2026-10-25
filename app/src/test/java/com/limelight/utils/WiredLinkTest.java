package com.limelight.utils;

import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.utils.WiredLink.Route;

import org.junit.Test;

import java.util.Arrays;
import java.util.Collections;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

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

    private static Route vpnOver(boolean wifi, boolean cellular, int prefix, boolean active) {
        return new Route("tun0", false, true, active, prefix, wifi, cellular);
    }

    @Test public void wifiAndCellularRoutesAreWireless() {
        assertTrue(WiredLink.wirelessToward(Collections.singletonList(
                new Route("wlan0", false, false, true, 24, true, false))));
        assertTrue(WiredLink.wirelessToward(Collections.singletonList(
                new Route("rmnet0", false, false, true, 0, false, true))));
    }

    @Test public void wiredUnknownAndEmptyRoutesAreNotWireless() {
        assertFalse(WiredLink.wirelessToward(Collections.singletonList(ethernet("eth0", 24, true))));
        // A network that shows no Wi-Fi or cellular transport (the 5-argument constructor sets
        // neither) is unknown, not wireless.
        assertFalse(WiredLink.wirelessToward(Collections.singletonList(new Route("tun9", false, false, true, 24))));
        assertFalse(WiredLink.wirelessToward(Collections.<Route>emptyList()));
        // Ethernet among the transports wins over a Wi-Fi flag on the same network.
        assertFalse(WiredLink.wirelessToward(Collections.singletonList(
                new Route("eth0", true, false, true, 24, true, false))));
    }

    @Test public void vpnCountsByItsVisibleUnderlyingTransport() {
        assertTrue(WiredLink.wirelessToward(Collections.singletonList(vpnOver(true, false, 0, true))));
        assertTrue(WiredLink.wirelessToward(Collections.singletonList(vpnOver(false, true, 0, true))));
        assertFalse(WiredLink.wirelessToward(Collections.singletonList(vpnOver(false, false, 0, true))));
    }

    // Android 9 and older cannot ask for the route to a host, so the active network's transports
    // stand in for it: they can say wireless, never a link speed.
    @Test public void activeNetworkAloneDecidesBeforeAndroid10() {
        // ethernet, wifi, cellular, vpn
        assertTrue(WiredLink.wirelessToward(WiredLink.activeNetworkRoute(false, true, false, false)));
        assertTrue(WiredLink.wirelessToward(WiredLink.activeNetworkRoute(false, false, true, false)));
        assertTrue(WiredLink.wirelessToward(WiredLink.activeNetworkRoute(false, true, false, true)));
        assertFalse(WiredLink.wirelessToward(WiredLink.activeNetworkRoute(true, false, false, false)));
        assertFalse(WiredLink.wirelessToward(WiredLink.activeNetworkRoute(true, true, false, false)));
        // A VPN that shows no underlying transport, or no active network at all, is unknown.
        assertTrue(WiredLink.activeNetworkRoute(false, false, false, true).isEmpty());
        assertFalse(WiredLink.wirelessToward(WiredLink.activeNetworkRoute(false, false, false, false)));
        // Never a speed: even an Ethernet guess has no interface to read one from.
        assertEquals(0, WiredLink.mbpsToward(WiredLink.activeNetworkRoute(true, false, false, false), name -> 1000));
    }

    @Test public void theRouteTheTrafficTakesDecidesWhetherItIsWireless() {
        Route wlan = new Route("wlan0", false, false, true, 0, true, false);
        // Wi-Fi is active, but the host's subnet is on Ethernet: the stream is wired.
        assertFalse(WiredLink.wirelessToward(Arrays.asList(wlan, ethernet("eth0", 24, false))));
        // Ethernet is the default, but the host's subnet is on Wi-Fi.
        assertTrue(WiredLink.wirelessToward(Arrays.asList(ethernet("eth0", 0, true),
                new Route("wlan0", false, false, false, 24, true, false))));
        // Equal prefix: the active network wins the tie.
        assertTrue(WiredLink.wirelessToward(Arrays.asList(
                new Route("wlan0", false, false, true, 24, true, false), ethernet("eth0", 24, false))));
        assertFalse(WiredLink.wirelessToward(Arrays.asList(
                new Route("wlan0", false, false, false, 24, true, false), ethernet("eth0", 24, true))));
    }

    @Test public void onlyARubylightHostOnAWirelessRouteWithTheSettingOnIsTold() {
        for (HostCapabilities.Family family : HostCapabilities.Family.values()) {
            boolean rubylight = family == HostCapabilities.Family.RUBYLIGHT;
            assertEquals(family.name(), rubylight, WiredLink.tellsHostWireless(true, family, true));
            assertFalse(WiredLink.tellsHostWireless(false, family, true));
            assertFalse(WiredLink.tellsHostWireless(true, family, false));
        }
    }
}
