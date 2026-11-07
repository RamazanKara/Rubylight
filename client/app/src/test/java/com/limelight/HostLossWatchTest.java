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
        final List<Boolean> closedPassed = new ArrayList<>();
        final List<Integer> pingsPassed = new ArrayList<>();
        final List<Boolean> onLinkPassed = new ArrayList<>();
        final List<String> addresses = new ArrayList<>();
        int roundCode = 2; // SilentLinkUp
        /** Each round's code in turn, then roundCode. */
        final List<Integer> roundCodes = new ArrayList<>();
        String gateway = "192.168.4.1";
        /** The PC's answer to the first ping, then to every round's. */
        int firstPing = 1;
        int roundPing = 0;
        int pinged;
        int rounds;
        boolean goodbye = true;
        long clock;
        Runnable onRound = () -> { };

        @Override public int round(String address, int port, String gateway) {
            rounds++;
            addresses.add(address);
            onRound.run();
            return roundCodes.isEmpty() ? roundCode : roundCodes.remove(0);
        }
        @Override public int afterGoodbye(boolean pingedAtStart, boolean onLink, int round, int pings, boolean closedBefore, long waitedMs) {
            assertTrue(pingedAtStart);
            onLinkPassed.add(onLink);
            pingsPassed.add(pings);
            closedPassed.add(closedBefore);
            return verdicts.isEmpty() ? HostLossWatch.GOODBYE_UNKNOWN : verdicts.remove(0);
        }
        @Override public synchronized int ping(String address, int timeoutMs) {
            addresses.add(address);
            return pinged++ == 0 ? firstPing : roundPing;
        }
        @Override public boolean saidGoodbye() { return goodbye; }
        @Override public String gatewayToward(InetAddress host) { return gateway; }
        @Override public long now() { return clock; }
        @Override public void sleep(long ms) { clock += ms + 750; }
    }

    private static HostLossWatch started(Script script) throws Exception {
        HostLossWatch watch = new HostLossWatch(script, InetAddress.getByName("192.168.4.69"), 47984);
        ReflectionHelpers.callInstanceMethod(watch, "firstPing");
        return watch;
    }

    @Test public void aPcShuttingDownEndsAfterOneRound() throws Exception {
        Script script = new Script();
        script.verdicts.add(HostLossWatch.GOODBYE_GONE);
        assertEquals(HostLossWatch.GOODBYE_GONE, started(script).afterGoodbye());
        assertEquals(1, script.rounds);
        assertEquals("the round pinged the PC and passed its answer", Arrays.asList(0), script.pingsPassed);
        assertEquals(Arrays.asList(false), script.closedPassed);
    }

    @Test public void aRestartIsWaitedForAndComesBack() throws Exception {
        Script script = new Script();
        script.roundPing = 1;
        script.verdicts.addAll(Arrays.asList(HostLossWatch.GOODBYE_WAIT, HostLossWatch.GOODBYE_WAIT, HostLossWatch.GOODBYE_BACK));
        assertEquals(HostLossWatch.GOODBYE_BACK, started(script).afterGoodbye());
        assertEquals(3, script.rounds);
        assertEquals("only rounds after a closed one say so", Arrays.asList(false, true, true), script.closedPassed);
        assertEquals(Arrays.asList(1, 1, 1), script.pingsPassed);
    }

    @Test public void aHostStillListeningRightAfterItsGoodbyeIsNoClosedServer() throws Exception {
        Script script = new Script();
        script.roundPing = 1;
        // Answered while the old host exits, refused once it is gone, answered by the new one.
        script.roundCodes.addAll(Arrays.asList(HostLossWatch.ROUND_ANSWERED, HostLossWatch.ROUND_ANSWERED, 1, HostLossWatch.ROUND_ANSWERED));
        script.verdicts.addAll(Arrays.asList(HostLossWatch.GOODBYE_WAIT, HostLossWatch.GOODBYE_WAIT, HostLossWatch.GOODBYE_WAIT, HostLossWatch.GOODBYE_BACK));
        assertEquals(HostLossWatch.GOODBYE_BACK, started(script).afterGoodbye());
        assertEquals(Arrays.asList(false, false, false, true), script.closedPassed);
    }

    @Test public void aSilenceIsNeverChecked() throws Exception {
        Script script = new Script();
        script.goodbye = false;
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, started(script).afterGoodbye());
        assertEquals(0, script.rounds);
    }

    @Test public void aPcThatNeverAnsweredAPingIsTheOldBehaviour() throws Exception {
        Script script = new Script();
        script.firstPing = 0;
        script.verdicts.add(HostLossWatch.GOODBYE_GONE);
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, started(script).afterGoodbye());
        assertEquals(0, script.rounds);
    }

    @Test public void aPcOffThisLinkIsNeverPinged() throws Exception {
        // A relay, a VPN or a routed PC: no gateway to compare with, so the old behaviour decides.
        Script script = new Script();
        script.gateway = null;
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, started(script).afterGoodbye());
        assertEquals(0, script.pinged);
        assertEquals(0, script.rounds);
    }

    @Test public void aLinkThatGoesQuietMidWayPassesThatOn() throws Exception {
        Script script = new Script();
        script.verdicts.add(HostLossWatch.GOODBYE_WAIT);
        script.onRound = () -> script.gateway = null;
        started(script).afterGoodbye();
        assertEquals(Arrays.asList(true, false), script.onLinkPassed);
        assertEquals(HostLossWatch.NOT_PINGED, (int) script.pingsPassed.get(1));
    }

    @Test public void aStopEndsTheWaitWithTheOldBehaviour() throws Exception {
        Script script = new Script();
        script.roundPing = 1;
        for (int i = 0; i < 100; i++) script.verdicts.add(HostLossWatch.GOODBYE_WAIT);
        HostLossWatch watch = started(script);
        script.onRound = () -> { if (script.rounds == 2) watch.stop(); };
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, watch.afterGoodbye());
        assertEquals("the verdict of a round that ran into the stop is dropped", 1, script.closedPassed.size());

        Script before = new Script();
        HostLossWatch stopped = started(before);
        stopped.stop();
        assertEquals(HostLossWatch.GOODBYE_UNKNOWN, stopped.afterGoodbye());
        assertEquals(0, before.rounds);
    }

    @Test public void theStreamsAddressIsUsedAsIsAndNeverLookedUp() throws Exception {
        Script script = new Script();
        script.verdicts.add(HostLossWatch.GOODBYE_GONE);
        started(script).afterGoodbye();
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
