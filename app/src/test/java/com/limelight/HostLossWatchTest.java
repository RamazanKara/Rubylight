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
        long bytes;
        long bytesPerTick;
        boolean networkLost;
        int ended;
        int gatewayLookups;
        long silenceAfterVerdict = -1;
        private int tick;

        @Override public long silenceMs() {
            if (tick >= silences.size()) return silenceAfterVerdict >= 0 ? silenceAfterVerdict : silences.get(silences.size() - 1);
            return silences.get(tick);
        }
        @Override public long mediaBytes() { return bytes; }
        @Override public int step(long silenceMs, String address, int port, String gateway, int state) {
            stepped.add(silenceMs);
            tick = silences.size();
            return answers.isEmpty() ? state : answers.remove(0);
        }
        @Override public String gatewayToward(InetAddress host) {
            gatewayLookups++;
            return "192.168.4.1";
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
        assertEquals(0, script.gatewayLookups);
        assertEquals(0, script.ended);
    }

    @Test public void theVerdictEndsTheStreamOnce() {
        Script script = silences(2000L, 3000L, 4000L);
        script.answers.addAll(Arrays.asList(1, 2, -1));
        runOnThisThread(watch(script));
        assertEquals(1, script.ended);
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
        HostLossWatch watch = watch(script);
        watch.stop();
        runOnThisThread(watch);
        assertEquals(0, script.ended);
        assertFalse(watch.confirmGone());
    }

    @Test public void aConnectionThatEndedChecksThePcUntilItKnows() {
        Script gone = silences(0L);
        gone.answers.addAll(Arrays.asList(1, 2, -1));
        assertTrue(watch(gone).confirmGone());
        assertEquals(3, gone.stepped.size());
        assertTrue("every verdict needs its time", gone.stepped.get(0) >= 4000);

        Script answers = silences(0L);
        answers.answers.addAll(Arrays.asList(1, 0, -1));
        assertFalse(watch(answers).confirmGone());
        assertEquals(2, answers.stepped.size());

        Script neverDecides = silences(0L);
        neverDecides.answers.addAll(Arrays.asList(1, 1, 1, 1, 1, 1, -1));
        assertFalse(watch(neverDecides).confirmGone());
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
