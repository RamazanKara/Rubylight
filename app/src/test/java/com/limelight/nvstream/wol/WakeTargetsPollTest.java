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

import java.util.Arrays;
import java.util.Collections;

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

    @Before
    public void setUp() {
        targets = new WakeTargets(RuntimeEnvironment.getApplication());
        poll = new WakeTargetsPoll(targets, computer -> paired ? new NativeApiClient(transport,
                HostEndpoint.of(computer, "unique", NativeApiFixtures.crypto())) : null, () -> now);
        transport.reply("pc", "capabilities", CAPABILITIES);
        transport.reply("pc", "wake", ok(WakeTargetsTest.WAKE));
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
}
