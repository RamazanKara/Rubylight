package com.limelight.nvstream.http;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import java.io.IOException;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, manifest = Config.NONE)
public class NetworkPathInfoTest {
    @Test public void acceptsOnlyKnownCategoriesAndVersion() throws Exception {
        assertEquals(NetworkPathInfo.Kind.DIRECT, NetworkPathInfo.parse("{\"version\":1,\"kind\":\"direct\"}").kind);
        assertEquals(NetworkPathInfo.Kind.RELAY, NetworkPathInfo.parse("{\"version\":1,\"kind\":\"relay\"}").kind);
        assertEquals(NetworkPathInfo.Kind.PEER_RELAY, NetworkPathInfo.parse("{\"version\":1,\"kind\":\"peer_relay\"}").kind);
        assertEquals(NetworkPathInfo.Kind.UNKNOWN, NetworkPathInfo.parse("{\"version\":1,\"kind\":\"future\"}").kind);
        for (String input : new String[]{"{}", "[]", "<html/>", "{\"version\":2,\"kind\":\"direct\"}",
                "{\"version\":\"1\",\"kind\":\"direct\"}", new String(new char[513])}) {
            try { NetworkPathInfo.parse(input); fail(input); } catch (IOException expected) { }
        }
    }
    @Test public void copyPreservesCapability() {
        ComputerDetails computer = new ComputerDetails(); computer.caps = TestHosts.rubylight();
        assertTrue(new ComputerDetails(computer).caps.has(HostCapabilities.Feature.NETWORK_PATH));
    }
}
