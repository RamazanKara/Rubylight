package com.limelight.nvstream.wol;

import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.RejectedExecutionException;

/** The computer manager's polls keep the wake targets, so the PC list and shortcuts wake every adapter. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class WakeTargetsPollTest {
    private static final String CAPABILITIES = ok("{\"api_version\":1,\"features\":[\"wake\"],"
            + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true,\"host_control\":false}}");

    private final NativeApiFixtures.FakeTransport transport = new NativeApiFixtures.FakeTransport();
    private WakeTargets targets;
    private WakeTargetsPoll poll;
    private long now;
    private boolean paired = true;
    /** The requests handed to the executor; they run at once unless {@link #queue} holds them. */
    private final List<Runnable> queued = new ArrayList<>();
    private boolean queue;
    /** The executor refuses the requests, as one shut down does. */
    private boolean rejecting;
    private int handOffs;
    private int opens;
    /** Runs while the host answers /wake, so a test can act during the request. */
    private Runnable duringWake;

    @Before
    public void setUp() {
        targets = new WakeTargets(RuntimeEnvironment.getApplication());
        poll = new WakeTargetsPoll(targets, computer -> {
            opens++;
            return paired ? new NativeApiClient((host, op, argument) -> {
                if (op.equals("wake") && duringWake != null) {
                    duringWake.run();
                }
                return transport.request(host, op, argument);
            }, HostEndpoint.of(computer, "unique", NativeApiFixtures.crypto())) : null;
        }, task -> {
            handOffs++;
            if (rejecting) {
                throw new RejectedExecutionException("shut down");
            }
            if (queue) {
                queued.add(task);
            } else {
                task.run();
            }
        }, () -> now);
        transport.reply("pc", "capabilities", CAPABILITIES);
        transport.reply("pc", "wake", ok(WakeTargetsTest.WAKE));
    }

    private void runQueued() {
        List<Runnable> tasks = new ArrayList<>(queued);
        queued.clear();
        tasks.forEach(Runnable::run);
    }

    private int asks() {
        return Collections.frequency(transport.calls, "pc|wake");
    }

    @Test
    public void aPcThatAnswersIsAskedOnceAndAgainAfterTheInterval() {
        ComputerDetails pc = NativeApiFixtures.nativeHost("pc", "PC", true);
        poll.polled(pc);
        assertEquals(Arrays.asList("02:7c:16:b4:13:4f", "02:e0:4c:68:00:01"), targets.load("pc").macs);
        assertEquals(1, asks());

        now += WakeTargetsPoll.INTERVAL_MS - 1;
        poll.polled(pc);
        assertEquals("polls in between ask nothing", 1, asks());
        now += 1;
        poll.polled(pc);
        assertEquals(2, asks());
    }

    @Test
    public void aPcThatWasAwayIsAskedOnceItAnswersAgain() {
        poll.polled(NativeApiFixtures.nativeHost("pc", "PC", true));
        poll.polled(NativeApiFixtures.nativeHost("pc", "PC", false));
        assertEquals(1, asks());
        assertEquals("an away PC keeps its targets", 2, targets.load("pc").macs.size());
        poll.polled(NativeApiFixtures.nativeHost("pc", "PC", true));
        assertEquals(2, asks());
    }

    @Test
    public void aPcNotPairedYetIsAskedOnTheFirstPollAfterItIs() {
        paired = false;
        ComputerDetails pc = NativeApiFixtures.nativeHost("pc", "PC", true);
        poll.polled(pc);
        assertTrue(transport.calls.isEmpty());
        paired = true;
        poll.polled(pc);
        assertEquals(1, asks());
    }

    @Test
    public void aPcWithoutTheNativeApiIsNotAsked() {
        poll.polled(NativeApiFixtures.classicHost("pc", "PC"));
        assertTrue(transport.calls.isEmpty());
        assertSame(WakeTargets.Targets.NONE, targets.load("pc"));
    }

    @Test
    public void anAskOnItsWayKeepsASecondCallerFromSendingAnother() {
        queue = true;
        ComputerDetails pc = NativeApiFixtures.nativeHost("pc", "PC", true);
        poll.polled(pc);
        poll.polled(pc);
        assertEquals("the poll hands the request off and returns", 0, asks());
        assertEquals(1, queued.size());
        runQueued();
        assertEquals(1, asks());
    }

    @Test
    public void aPcThatCouldNotBeAskedIsAskedByTheNextPoll() {
        queue = true;
        paired = false;
        ComputerDetails pc = NativeApiFixtures.nativeHost("pc", "PC", true);
        poll.polled(pc);
        runQueued();
        paired = true;
        poll.polled(pc);
        assertEquals("no client gives the reservation back", 1, queued.size());
        runQueued();
        assertEquals(1, asks());
    }

    @Test
    public void aPcRemovedWhileItsAskWaitsInTheQueueIsAskedNothing() {
        queue = true;
        ComputerDetails pc = NativeApiFixtures.nativeHost("pc", "PC", true);
        poll.polled(pc);
        poll.forget("pc");
        runQueued();
        assertEquals("no client is opened", 0, opens);
        assertTrue(transport.calls.isEmpty());

        poll.polled(pc);
        assertEquals("a PC added again is asked at once", 1, queued.size());
    }

    @Test
    public void aRemovedPcKeepsNothingOfAnAskStillOnItsWay() {
        duringWake = () -> {
            poll.forget("pc");
            targets.forget("pc");
        };
        poll.polled(NativeApiFixtures.nativeHost("pc", "PC", true));
        assertEquals(1, asks());
        assertSame(WakeTargets.Targets.NONE, targets.load("pc"));
    }

    @Test
    public void anAskOnItsWayWhenTheServiceGoesAwayStoresNothing() {
        duringWake = poll::shutdown;
        poll.polled(NativeApiFixtures.nativeHost("pc", "PC", true));
        assertEquals(1, asks());
        assertSame(WakeTargets.Targets.NONE, targets.load("pc"));

        queue = true;
        duringWake = null;
        now += WakeTargetsPoll.INTERVAL_MS;
        poll.polled(NativeApiFixtures.nativeHost("pc", "PC", true));
        poll.shutdown();
        runQueued();
        assertEquals("a queued ask is dropped too", 1, asks());
    }

    @Test
    public void anAskTheExecutorRefusesGivesItsReservationBack() {
        rejecting = true;
        ComputerDetails pc = NativeApiFixtures.nativeHost("pc", "PC", true);
        poll.polled(pc);
        poll.polled(pc);
        assertEquals("the next poll, at the same time, tries again", 2, handOffs);
        assertEquals(0, opens);
    }
}
