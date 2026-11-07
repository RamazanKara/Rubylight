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
import java.net.UnknownHostException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostLossWatchTest {
    /** A silence and a detector scripted per tick; the watch runs on the test's thread. */
    private static final class Script implements HostLossWatch.Probes {
        final List<Long> silences = new ArrayList<>();
        final List<Integer> answers = new ArrayList<>();
        final List<Long> stepped = new ArrayList<>();
        final List<Integer> pingsPassed = new ArrayList<>();
        final List<Integer> firstStates = new ArrayList<>();
        long bytes;
        long bytesPerTick;
        boolean networkLost;
        int ended;
        int gatewayLookups;
        int rounds;
        int roundCode = 2; // SilentLinkUp
        String gateway = "192.168.4.1";
        /** The PC's answer to the first ping, then to every round's. */
        int firstPing = 1;
        int roundPing = 0;
        int pinged;
        boolean goodbye;
        long silenceAfterVerdict = -1;
        private int tick;

        @Override public long silenceMs() {
            if (tick >= silences.size()) return silenceAfterVerdict >= 0 ? silenceAfterVerdict : silences.get(silences.size() - 1);
            return silences.get(tick);
        }
        @Override public long mediaBytes() { return bytes; }
        @Override public int round(String address, int port, String gateway) {
            rounds++;
            return roundCode;
        }
        @Override public int decide(long silenceMs, int round, int pings, int state) {
            stepped.add(silenceMs);
            pingsPassed.add(pings);
            tick = silences.size();
            return answers.isEmpty() ? state : answers.remove(0);
        }
        @Override public int firstState(boolean pings) {
            firstStates.add(pings ? 256 : 0);
            return pings ? 256 : 0;
        }
        @Override public synchronized int ping(String address, int timeoutMs) {
            return pinged++ == 0 ? firstPing : roundPing;
        }
        @Override public boolean saidGoodbye() { return goodbye; }
        @Override public String gatewayToward(InetAddress host) {
            gatewayLookups++;
            return gateway;
        }
        @Override public boolean networkLost() { return networkLost; }
        @Override public void endStream() { ended++; }
        @Override public InetAddress resolve(String address) throws UnknownHostException {
            return InetAddress.getByName(address);
        }
        @Override public void sleep(long ms) throws InterruptedException {
            tick = nextTick++;
            bytes += bytesPerTick;
            if (nextTick > silences.size() + 2) throw new InterruptedException();
        }
        private int nextTick;
    }

    private static Script silences(Long... ms) {
        Script script = new Script();
        script.silences.addAll(Arrays.asList(ms));
        return script;
    }

    private static HostLossWatch watch(Script script) {
        return new HostLossWatch(script, "192.168.4.69", 47984);
    }

    private static void runOnThisThread(HostLossWatch watch) {
        ReflectionHelpers.callInstanceMethod(watch, "run");
    }

    @Test public void aHostThatIsHeardIsNeverProbed() {
        Script script = silences(100L, 1999L, 0L);
        runOnThisThread(watch(script));
        assertTrue(script.stepped.isEmpty());
        assertEquals(0, script.rounds);
        assertEquals(0, script.ended);
    }

    @Test public void theVerdictEndsTheStreamOnce() {
        Script script = silences(2000L, 3000L, 4000L);
        script.answers.addAll(Arrays.asList(257, 258, -1));
        runOnThisThread(watch(script));
        assertEquals(1, script.ended);
        assertEquals("every round pinged the PC and passed its answer", Arrays.asList(0, 0, 0), script.pingsPassed);
        assertEquals(1 + 3, script.pinged);
    }

    @Test public void aPcThatAnsweredTheFirstPingIsAskedEveryRound() {
        Script script = silences(2000L, 3000L);
        runOnThisThread(watch(script));
        assertEquals(Arrays.asList(256), script.firstStates);
        assertEquals(1 + script.rounds, script.pinged);
    }

    @Test public void aPcThatStillAnswersPingsPassesThatOn() {
        // A crashed or restarting server: the decision (nightjar_core) keeps the recovery.
        Script script = silences(2000L, 3000L, 4000L);
        script.roundPing = 1;
        runOnThisThread(watch(script));
        assertFalse(script.pingsPassed.isEmpty());
        for (int pings : script.pingsPassed) assertEquals(1, pings);
        assertEquals(0, script.ended);
    }

    @Test public void aPcThatNeverAnsweredAPingIsNeverPingedAgain() {
        Script script = silences(2000L, 3000L, 4000L);
        script.firstPing = 0;
        runOnThisThread(watch(script));
        assertEquals(Arrays.asList(0), script.firstStates);
        assertEquals(1, script.pinged);
        assertFalse(script.pingsPassed.isEmpty());
        for (int pings : script.pingsPassed) assertEquals(HostLossWatch.NOT_PINGED, pings);
    }

    @Test public void aPcOffThisLinkIsNeverPinged() {
        // A relay, a VPN or a routed PC: no gateway to compare with, so the old timeout decides.
        Script script = silences(2000L, 3000L);
        script.gateway = null;
        runOnThisThread(watch(script));
        assertEquals(0, script.pinged);
        assertEquals(Arrays.asList(0), script.firstStates);
        for (int pings : script.pingsPassed) assertEquals(HostLossWatch.NOT_PINGED, pings);
    }

    @Test public void mediaStillArrivingIsAHost() {
        Script script = silences(5000L, 6000L, 7000L);
        script.bytesPerTick = 1000;
        script.answers.addAll(Arrays.asList(-1, -1, -1));
        runOnThisThread(watch(script));
        assertTrue(script.stepped.isEmpty());
        assertEquals(0, script.ended);
    }

    @Test public void aDeviceWithoutANetworkProvesNothing() {
        Script script = silences(5000L, 6000L, 7000L);
        script.networkLost = true;
        script.answers.addAll(Arrays.asList(-1, -1, -1));
        runOnThisThread(watch(script));
        assertTrue(script.stepped.isEmpty());
        assertEquals(0, script.ended);
    }

    @Test public void aHostHeardDuringTheLastRoundIsNotGone() {
        Script script = silences(5000L);
        script.answers.add(-1);
        script.silenceAfterVerdict = 0;
        runOnThisThread(watch(script));
        assertEquals(0, script.ended);
    }

    @Test public void aWatchThatStoppedDoesNothing() {
        Script script = silences(5000L, 6000L);
        script.answers.addAll(Arrays.asList(-1, -1));
        script.goodbye = true;
        HostLossWatch watch = watch(script);
        watch.stop();
        runOnThisThread(watch);
        assertEquals(0, script.ended);
        assertFalse(watch.goneAfterGoodbye());
    }

    @Test public void onlyAGoodbyeWithTheServerClosedIsAGonePc() {
        Script silent = silences(0L);
        assertFalse("a silence past the timeout is left to the recovery", watch(silent).goneAfterGoodbye());
        assertEquals(0, silent.rounds);

        Script shuttingDown = silences(0L);
        shuttingDown.goodbye = true;
        assertTrue(watch(shuttingDown).goneAfterGoodbye());
        assertEquals(1, shuttingDown.rounds);

        Script refused = silences(0L);
        refused.goodbye = true;
        refused.roundCode = 1;
        assertTrue("its server stopped", watch(refused).goneAfterGoodbye());

        Script stillServing = silences(0L);
        stillServing.goodbye = true;
        stillServing.roundCode = HostLossWatch.ROUND_ANSWERED;
        assertFalse("only this stream ended", watch(stillServing).goneAfterGoodbye());
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
