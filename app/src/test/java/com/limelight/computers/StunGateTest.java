package com.limelight.computers;

import org.junit.Test;

import java.net.InetAddress;
import java.net.UnknownHostException;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** A PC on the local network gets a STUN remote address, whether it was added by address or by name. */
public class StunGateTest {
    private static InetAddress resolveGamePc(String host) throws UnknownHostException {
        if (host.equals("gamepc.lan")) {
            return InetAddress.getByAddress(host, new byte[] {(byte) 192, (byte) 168, 1, 10});
        }
        if (host.equals("gone.lan")) {
            throw new UnknownHostException(host);
        }
        return InetAddress.getByName(host);
    }

    @Test
    public void aNameOnTheLocalNetworkIsResolvedAndClassified() {
        assertTrue(ComputerManagerService.isSiteLocalHost("gamepc.lan", StunGateTest::resolveGamePc));
        assertTrue(ComputerManagerService.isSiteLocalHost("10.0.0.5", StunGateTest::resolveGamePc));
        assertFalse(ComputerManagerService.isSiteLocalHost("8.8.8.8", StunGateTest::resolveGamePc));
        assertFalse(ComputerManagerService.isSiteLocalHost("gone.lan", StunGateTest::resolveGamePc));
    }
}
