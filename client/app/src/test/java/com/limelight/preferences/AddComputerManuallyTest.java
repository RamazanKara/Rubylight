package com.limelight.preferences;

import org.junit.Test;

import java.net.InetAddress;

import static org.junit.Assert.*;

public class AddComputerManuallyTest {
    @Test
    public void hostInputAcceptsNamesAndIpv6ButRejectsIgnoredUrlPartsAndInvalidPorts() {
        for (String value : new String[]{"gaming-pc", "pc.local", "192.168.1.10:47989", "[::1]:47989", "::1"}) {
            assertNotNull(value, AddComputerManually.parseRawUserInputToUri(value));
        }
        for (String value : new String[]{"", "bad host", "pc.local:0", "pc.local:65536", "pc.local/path",
                "user@pc.local", "pc.local?port=10", "pc.local#anything", "https://pc.local"}) {
            assertNull(value, AddComputerManually.parseRawUserInputToUri(value));
        }
    }

    @Test
    public void nonByteAlignedSubnetsCompareMostSignificantBitsFirst() throws Exception {
        byte[] local = InetAddress.getByName("172.16.8.10").getAddress();
        assertTrue(AddComputerManually.isInSubnet(InetAddress.getByName("172.31.2.20").getAddress(), local, 12));
        assertFalse(AddComputerManually.isInSubnet(InetAddress.getByName("172.32.8.10").getAddress(), local, 12));
        local = InetAddress.getByName("192.168.1.130").getAddress();
        assertTrue(AddComputerManually.isInSubnet(InetAddress.getByName("192.168.1.200").getAddress(), local, 25));
        assertFalse(AddComputerManually.isInSubnet(InetAddress.getByName("192.168.1.2").getAddress(), local, 25));
    }

    @Test
    public void byteAlignedAndHostRoutesStillMatch() throws Exception {
        byte[] local = InetAddress.getByName("192.168.1.10").getAddress();
        assertTrue(AddComputerManually.isInSubnet(InetAddress.getByName("192.168.1.200").getAddress(), local, 24));
        assertFalse(AddComputerManually.isInSubnet(InetAddress.getByName("192.168.2.10").getAddress(), local, 24));
        assertTrue(AddComputerManually.isInSubnet(local, local, 32));
        assertFalse(AddComputerManually.isInSubnet(InetAddress.getByName("192.168.1.11").getAddress(), local, 32));
    }
}
