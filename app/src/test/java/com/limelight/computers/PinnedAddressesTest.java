package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

import android.app.Application;
import android.content.Context;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.ComputerDetails.AddressTuple;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.Arrays;

/** A pinned address is the only one polled, and so streamed to, until the pin is removed. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class PinnedAddressesTest {
    private static final AddressTuple LOCAL = new AddressTuple("192.168.1.20", 47989);
    private static final AddressTuple REMOTE = new AddressTuple("203.0.113.7", 47989);
    private static final AddressTuple IPV6 = new AddressTuple("[2001:db8::7]", 47989);

    @Test
    public void aPinnedAddressWinsUntilThePinIsRemoved() {
        Context context = RuntimeEnvironment.getApplication();
        ComputerDetails pc = new ComputerDetails();
        pc.uuid = "uuid-1";
        pc.localAddress = LOCAL;
        pc.manualAddress = LOCAL;
        pc.remoteAddress = REMOTE;
        pc.ipv6Address = IPV6;
        assertEquals("automatic: local, then remote, then IPv6, without repeats",
                Arrays.asList(LOCAL, REMOTE, IPV6), pc.pollAddresses());

        PinnedAddresses.set(context, pc.uuid, REMOTE);
        pc.pinnedAddress = PinnedAddresses.get(context, pc.uuid);
        assertEquals(REMOTE, pc.pinnedAddress);
        assertEquals("the pinned address is the only one polled", Arrays.asList(REMOTE), pc.pollAddresses());
        assertEquals("a copy keeps the pin", Arrays.asList(REMOTE), new ComputerDetails(pc).pollAddresses());
        // The review's regression: an mDNS rediscovery or another answer must not move a pinned PC.
        pc.activeAddress = REMOTE;
        ComputerDetails rediscovered = new ComputerDetails();
        rediscovered.uuid = pc.uuid;
        rediscovered.localAddress = LOCAL;
        rediscovered.activeAddress = LOCAL;
        pc.update(rediscovered);
        assertEquals("the pin keeps the address streams use", REMOTE, pc.activeAddress);
        assertEquals(REMOTE, new ComputerDetails(pc).activeAddress);
        PinnedAddresses.set(context, "uuid-2", IPV6);
        assertEquals("IPv6 survives the round trip", IPV6, PinnedAddresses.get(context, "uuid-2"));

        PinnedAddresses.set(context, pc.uuid, null);
        pc.pinnedAddress = PinnedAddresses.get(context, pc.uuid);
        assertNull(pc.pinnedAddress);
        assertEquals("back to the automatic pick", Arrays.asList(LOCAL, REMOTE, IPV6), pc.pollAddresses());
    }
}
