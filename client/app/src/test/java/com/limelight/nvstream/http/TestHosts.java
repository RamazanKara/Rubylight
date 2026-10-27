package com.limelight.nvstream.http;

import java.io.InputStream;
import java.nio.charset.StandardCharsets;

/** Host capabilities for tests, read from serverinfo the way NvHTTP reads them. */
public final class TestHosts {
    private TestHosts() {
    }

    /** A fixture from src/test/resources/protocol. */
    public static String serverInfo(String fixture) {
        try (InputStream input = TestHosts.class.getResourceAsStream("/protocol/" + fixture)) {
            if (input == null) {
                throw new IllegalArgumentException("No fixture " + fixture);
            }
            return new String(input.readAllBytes(), StandardCharsets.UTF_8);
        } catch (java.io.IOException e) {
            throw new IllegalStateException(e);
        }
    }

    public static HostCapabilities caps(String serverInfoXml) {
        try {
            return HostCapabilities.fromServerInfo(serverInfoXml);
        } catch (Exception e) {
            throw new IllegalArgumentException(e);
        }
    }

    public static HostCapabilities fixture(String fixture) {
        return caps(serverInfo(fixture));
    }

    /** Paired Rubylight 2.2.21 with every permission and feature. */
    public static HostCapabilities rubylight() {
        return fixture("rubylight-serverinfo.xml");
    }

    /** Capabilities, permissions, server commands and encoder bitrate of a fixture, as a stream sees them. */
    public static ComputerDetails details(String fixture) {
        ComputerDetails details = new ComputerDetails();
        try {
            NvHTTP.readClientCapabilities(details, serverInfo(fixture));
        } catch (Exception e) {
            throw new IllegalArgumentException(e);
        }
        details.httpsPort = 47984;
        return details;
    }
}
