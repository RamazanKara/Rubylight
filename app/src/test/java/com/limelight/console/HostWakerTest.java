package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotSame;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.os.Handler;
import android.os.Looper;

import com.limelight.PcViewLifecycleTest;
import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.io.IOException;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

/** Waking a PC and waiting for it, with a fake network, a fake clock and the main looper. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class HostWakerTest {
    private long now;
    private ComputerDetails host;
    private final List<ComputerDetails> sent = new ArrayList<>();
    private IOException sendError;
    private final List<String> events = new ArrayList<>();
    private HostWaker waker;

    @Before
    public void setUp() throws Exception {
        host = nativeHost("tower", "Tower", false);
        host.macAddress = "aa:bb:cc:dd:ee:ff";
        waker = new HostWaker(new Handler(Looper.getMainLooper()), Runnable::run, target -> {
            sent.add(target);
            if (sendError != null) {
                throw sendError;
            }
        }, () -> host, () -> now, new HostWaker.Listener() {
            @Override public void onOnline() {
                events.add("online");
            }

            @Override public void onTimedOut() {
                events.add("timeout");
            }

            @Override public void onSendFailed(IOException error) {
                events.add("failed");
            }
        });
    }

    private void pass(long ms) {
        now += ms;
        shadowOf(Looper.getMainLooper()).idleFor(ms, TimeUnit.MILLISECONDS);
    }

    @Test
    public void onlyAnAwayPcWithAMacAddressCanBeWoken() {
        Application app = RuntimeEnvironment.getApplication();
        assertTrue(HostWaker.canWake(app, host));
        host.macAddress = null;
        assertFalse("No MAC, no wake", HostWaker.canWake(app, host));
        assertFalse(HostWaker.canWake(app, null));
        ComputerDetails up = nativeHost("up", "Up", true);
        up.macAddress = "aa:bb:cc:dd:ee:ff";
        assertFalse("An online PC is not woken", HostWaker.canWake(app, up));
    }

    @Test
    public void sendsAtOnceAndResendsWhileThePcStaysAway() {
        assertTrue(waker.start());
        assertEquals("The first packet goes out at once", 1, sent.size());
        assertNotSame("The packet is built from a copy, off the feed's object", host, sent.get(0));
        assertEquals("aa:bb:cc:dd:ee:ff", sent.get(0).macAddress);
        pass(7_000);
        assertEquals(1, sent.size());
        pass(1_000);
        assertEquals(2, sent.size());
        assertTrue(waker.isWaiting());
        assertTrue(events.isEmpty());
    }

    @Test
    public void carriesOnWhenThePcAnswers() {
        waker.start();
        pass(5_000);
        host.state = ComputerDetails.State.ONLINE;
        pass(1_000);
        assertEquals(java.util.Collections.singletonList("online"), events);
        assertFalse(waker.isWaiting());
        pass(30_000);
        assertEquals("Nothing more happens after the answer", 1, events.size());
    }

    @Test
    public void givesUpLoudlyWhenThePcNeverAnswers() {
        waker.start();
        pass(WakeLaunch.TIMEOUT_MS + 1_000);
        assertEquals(java.util.Collections.singletonList("timeout"), events);
        assertFalse(waker.isWaiting());
    }

    @Test
    public void aSendThatFailsEndsTheWaitAndSaysSo() {
        sendError = new IOException("no route");
        waker.start();
        pass(1_000);
        assertEquals(java.util.Collections.singletonList("failed"), events);
        assertFalse(waker.isWaiting());
        assertEquals("No resends after a failure", 1, sent.size());
    }

    @Test
    public void cancelStopsTheWaitWithoutAnyCallback() {
        waker.start();
        waker.cancel();
        host.state = ComputerDetails.State.ONLINE;
        pass(WakeLaunch.TIMEOUT_MS + 1_000);
        assertTrue(events.isEmpty());
        assertFalse(waker.isWaiting());
    }

    @Test
    public void aSecondStartWhileWaitingIsRefused() {
        assertTrue(waker.start());
        assertFalse(waker.start());
        assertEquals(1, sent.size());
    }

    @Test
    public void startingAgainAfterACancelWorks() {
        waker.start();
        waker.cancel();
        assertTrue(waker.start());
        assertEquals(2, sent.size());
        host.state = ComputerDetails.State.ONLINE;
        pass(1_000);
        assertEquals(java.util.Collections.singletonList("online"), events);
    }
}
