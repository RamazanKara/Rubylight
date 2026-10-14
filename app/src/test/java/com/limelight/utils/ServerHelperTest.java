package com.limelight.utils;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;

import static org.junit.Assert.*;

public class ServerHelperTest {
    private ComputerDetails computer(String address, int httpPort, int httpsPort) {
        ComputerDetails computer = new ComputerDetails();
        computer.state = ComputerDetails.State.ONLINE;
        computer.caps = TestHosts.rubylight();
        computer.activeAddress = new ComputerDetails.AddressTuple(address, httpPort);
        computer.httpsPort = httpsPort;
        return computer;
    }

    @Test
    public void hostConsoleUsesAdvertisedHttpsPortAndCurrentAddress() {
        ComputerDetails computer = computer("192.168.1.10", 50000, 49995);
        computer.localAddress = new ComputerDetails.AddressTuple("other-host", 47989);
        assertEquals("https://192.168.1.10:50001/", ServerHelper.getHostSettingsUrl(computer).toString());
        computer.activeAddress = new ComputerDetails.AddressTuple("remote.example", 60000);
        assertEquals("https://remote.example:50001/", ServerHelper.getHostSettingsUrl(computer).toString());
    }

    @Test
    public void hostConsoleFormatsIpv6AndFallsBackOnlyWhenHttpsPortUnknown() {
        assertEquals("https://[2001:db8::1]:47990/",
                ServerHelper.getHostSettingsUrl(computer("[2001:db8::1]", 47989, 47984)).toString());
        assertEquals("https://host.example:50001/",
                ServerHelper.getHostSettingsUrl(computer("host.example", 50000, 0)).toString());
        assertNull(ServerHelper.getHostSettingsUrl(computer("host.example", 47989, 65530)));
        assertNull(ServerHelper.getHostSettingsUrl(computer("host.example", 47989, -1)));
        assertNull(ServerHelper.getHostSettingsUrl(computer("host.example", 65535, 0)));
        assertNull(ServerHelper.getHostSettingsUrl(computer("https://invalid.example", 47989, 47984)));
    }

    @Test
    public void hostWebUiIsOfferedForEveryOnlineHostButGameStream() {
        ComputerDetails computer = computer("host.example", 47989, 47984);
        computer.state = ComputerDetails.State.OFFLINE;
        assertNull(ServerHelper.getHostSettingsUrl(computer));
        computer.state = ComputerDetails.State.UNKNOWN;
        assertNull(ServerHelper.getHostSettingsUrl(computer));
        computer.state = ComputerDetails.State.ONLINE;
        for (String host : new String[] {"sunshine", "apollo", "apollo-http", "vibepollo", "rubylight"}) {
            computer.caps = TestHosts.fixture(host + "-serverinfo.xml");
            assertEquals(host, "https://host.example:47990/", ServerHelper.getHostSettingsUrl(computer).toString());
        }
        computer.caps = HostCapabilities.UNKNOWN;
        assertNotNull(ServerHelper.getHostSettingsUrl(computer));
        computer.caps = TestHosts.fixture("gamestream-serverinfo.xml");
        assertNull(ServerHelper.getHostSettingsUrl(computer));
        computer.caps = TestHosts.rubylight();
        computer.activeAddress = null;
        assertNull(ServerHelper.getHostSettingsUrl(computer));
    }
}
