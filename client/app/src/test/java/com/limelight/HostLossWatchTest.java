package com.limelight;

import android.app.Application;
import android.net.IpPrefix;
import android.net.RouteInfo;

import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.net.InetAddress;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostLossWatchTest {
    /** Rounds and verdicts scripted per round; the watch runs on the test's thread. */
    private static final class Script implements HostLossWatch.Probes {
        final List<Integer> verdicts = new ArrayList<>();
        final List<Integer> roundsPassed = new ArrayList<>();
        final List<Long> waitedPassed = new ArrayList<>();
        final List<String> addresses = new ArrayList<>();
        int roundCode = 2; // SilentLinkUp
        String gateway = "192.168.4.1";
        int rounds;
        boolean goodbye = true;
        long clock;
        Runnable onRound = () -> { };

        @Override public int round(String address, int port, String gateway) {
            rounds++;
            addresses.add(address);
            onRound.run();
            return roundCode;
        }
        @Override public int afterGoodbye(boolean onLink, int round, long waitedMs) {
            assertTrue("only a PC on this link gets a round", onLink);
            roundsPassed.add(round);
            waitedPassed.add(waitedMs);
            return verdicts.isEmpty() ? HostLossWatch.GOODBYE_UNKNOWN : verdicts.remove(0);
        }
        @Override public boolean saidGoodbye() { return goodbye; }
        @Override public String gatewayToward(InetAddress host) { return gateway; }
        @Override public long now() { return clock; }
        @Override public void sleep(long ms) { clock += ms + 750; }
    }

    private static HostLossWatch watch(Script script) throws Exception {
        return new HostLossWatch(script, InetAddress.getByName("192.168.4.69"), 47984);
    }

    @Test public void aClosedServerEndsAfterOneRound() throws Exception {
        Script script = new Script();
        script.verdicts.add(HostLossWatch.GOODBYE_GONE);
        assertEquals(HostLossWatch.GOODBYE_GONE, watch(script).afterGoodbye());
        assertEquals(1, script.rounds);
        assertEquals(Arrays.asList(2), script.roundsPassed);
        assertEquals(Arrays.asList(0L), script.waitedPassed);
    }

    @Test public void aHostStillExitingGetsMoreRoundsWithTheTimeWaited() throws Exception {
        Script script = new Script();
        script.roundCode = 0; // Answered
        script.verdicts.addAll(Arrays.asList(HostLossWatch.GOODBYE_WAIT, HostLossWatch.GOODBYE_WAIT, HostLossWatch.GOODBYE_UNKNOWN));
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, watch(script).afterGoodbye());
        assertEquals(3, script.rounds);
        assertEquals(Arrays.asList(0L, 1000L, 2000L), script.waitedPassed);
    }

    @Test public void aSilenceIsNeverChecked() throws Exception {
        Script script = new Script();
        script.goodbye = false;
        script.verdicts.add(HostLossWatch.GOODBYE_GONE);
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, watch(script).afterGoodbye());
        assertEquals(0, script.rounds);
    }

    @Test public void aPcOffThisLinkIsTheOldBehaviour() throws Exception {
        // A relay, a VPN or a routed PC: no gateway to compare with, so the old behaviour decides.
        Script script = new Script();
        script.gateway = null;
        script.verdicts.add(HostLossWatch.GOODBYE_GONE);
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, watch(script).afterGoodbye());
        assertEquals(0, script.rounds);
    }

    @Test public void aLinkThatGoesAwayMidWayIsTheOldBehaviour() throws Exception {
        Script script = new Script();
        script.verdicts.addAll(Arrays.asList(HostLossWatch.GOODBYE_WAIT, HostLossWatch.GOODBYE_GONE));
        script.onRound = () -> script.gateway = null;
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, watch(script).afterGoodbye());
        assertEquals(1, script.rounds);
    }

    @Test public void aStopEndsTheWaitWithTheOldBehaviour() throws Exception {
        Script script = new Script();
        for (int i = 0; i < 100; i++) script.verdicts.add(HostLossWatch.GOODBYE_WAIT);
        HostLossWatch watch = watch(script);
        script.onRound = () -> { if (script.rounds == 2) watch.stop(); };
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, watch.afterGoodbye());
        assertEquals("the verdict of a round that ran into the stop is dropped", 1, script.roundsPassed.size());

        Script before = new Script();
        HostLossWatch stopped = watch(before);
        stopped.stop();
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, stopped.afterGoodbye());
        assertEquals(0, before.rounds);
    }

    @Test public void theStreamsAddressIsUsedAsIsAndNeverLookedUp() throws Exception {
        Script script = new Script();
        script.verdicts.add(HostLossWatch.GOODBYE_GONE);
        watch(script).afterGoodbye();
        for (String address : script.addresses) assertEquals("192.168.4.69", address);
        assertEquals(InetAddress.getByName("192.168.4.69"), HostLossWatch.parse("192.168.4.69"));
        assertEquals(InetAddress.getByName("fe80::1"), HostLossWatch.parse("fe80::1"));
        assertNull("a name would need DNS", HostLossWatch.parse("my-pc.local"));
        assertNull(HostLossWatch.parse("deadbeef"));
        assertNull(HostLossWatch.parse(""));
        assertNull(HostLossWatch.parse(null));
    }

    private static RouteInfo route(String prefix, int length, String gateway) throws Exception {
        IpPrefix ipPrefix = ReflectionHelpers.callConstructor(IpPrefix.class,
                ClassParameter.from(InetAddress.class, InetAddress.getByName(prefix)), ClassParameter.from(int.class, length));
        return ReflectionHelpers.callConstructor(RouteInfo.class, ClassParameter.from(IpPrefix.class, ipPrefix),
                ClassParameter.from(InetAddress.class, gateway != null ? InetAddress.getByName(gateway) : InetAddress.getByName("0.0.0.0")),
                ClassParameter.from(String.class, "wlan0"));
    }

    @Test public void theGatewayCountsOnlyForAPcOnThisLink() throws Exception {
        List<RouteInfo> wifi = Arrays.asList(route("192.168.4.0", 24, null), route("0.0.0.0", 0, "192.168.4.1"));
        assertEquals("192.168.4.1", HostLossWatch.gatewayToward(wifi, InetAddress.getByName("192.168.4.69")));
        assertNull("behind a router", HostLossWatch.gatewayToward(wifi, InetAddress.getByName("10.0.0.5")));
        assertNull("a link without a gateway proves nothing",
                HostLossWatch.gatewayToward(Arrays.asList(route("192.168.4.0", 24, null)), InetAddress.getByName("192.168.4.69")));
        assertNull("only a route through a router", HostLossWatch.gatewayToward(
                Arrays.asList(route("192.168.4.0", 24, "192.168.1.1"), route("0.0.0.0", 0, "192.168.1.1")), InetAddress.getByName("192.168.4.69")));
    }

    @Test public void aGonePcEndsWithoutRecoveryOrAnotherLook() {
        int gone = MoonBridge.CLIENT_ERROR_HOST_GONE;
        assertFalse(StreamRecoveryPolicy.recoversTermination(gone, true));
        assertFalse(StreamRecoveryPolicy.transportTermination(gone));
        assertFalse(StreamExitPolicy.hostEndedStream(gone));
        assertFalse("back to the list with a message, nothing to act on",
                StreamExitPolicy.terminationNeedsAction(gone, false, false, false));
    }
}
