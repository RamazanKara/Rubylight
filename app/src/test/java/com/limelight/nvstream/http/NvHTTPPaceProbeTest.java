package com.limelight.nvstream.http;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class NvHTTPPaceProbeTest {
    private static final String VIBESHINE = "<root status_code=\"200\"><PairStatus>1</PairStatus>" +
            "<ServerCodecModeSupport>8388608</ServerCodecModeSupport>" +
            "<PyroWaveHostLinkMbps>2500</PyroWaveHostLinkMbps><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes>" +
            "<PyroWaveWireBudgetVersion>1</PyroWaveWireBudgetVersion><PyroWaveUdpProbeVersion>1</PyroWaveUdpProbeVersion>" +
            "<PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion><PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion>" +
            "<PyroWaveCriticalFecPercentage>25</PyroWaveCriticalFecPercentage><PyroWaveMinParityShards>3</PyroWaveMinParityShards></root>";

    @Test
    public void burstProbeHostIsReadWithItsFecBudget() throws Exception {
        ComputerDetails details = new ComputerDetails();
        NvHTTP.readClientCapabilities(details, VIBESHINE);
        assertTrue(details.pyroWaveBurstProbe);
        assertEquals(25, details.pyroWaveCriticalFecPercentage);
        assertEquals(3, details.pyroWaveMinParityShards);
        ComputerDetails copy = new ComputerDetails(details);
        assertTrue(copy.pyroWaveBurstProbe);
        assertEquals(25, copy.pyroWaveCriticalFecPercentage);
        assertEquals(3, copy.pyroWaveMinParityShards);
    }

    @Test
    public void hostWithoutBurstProbesOrHandshakeIsNotCalibrated() throws Exception {
        ComputerDetails details = new ComputerDetails();
        NvHTTP.readClientCapabilities(details, VIBESHINE.replace(
                "<PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion>", ""));
        assertFalse(details.pyroWaveBurstProbe);
        NvHTTP.readClientCapabilities(details, VIBESHINE.replace(
                "<PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion>", ""));
        assertFalse(details.pyroWaveBurstProbe);
        details.pairState = PairingManager.PairState.PAIRED;
        assertFalse(details.supportsPyroWavePaceCalibration());
    }

    @Test
    public void onlyAPlainUnprivilegedPortIsAccepted() {
        assertEquals(50123, NvHTTP.parseUdpPort("50123"));
        assertEquals(1024, NvHTTP.parseUdpPort("1024"));
        assertEquals(0, NvHTTP.parseUdpPort(null));
        assertEquals(0, NvHTTP.parseUdpPort(""));
        assertEquals(0, NvHTTP.parseUdpPort("80"));
        assertEquals(0, NvHTTP.parseUdpPort("1023"));
        assertEquals(0, NvHTTP.parseUdpPort("70000"));
        assertEquals(0, NvHTTP.parseUdpPort(" 5000"));
        assertEquals(0, NvHTTP.parseUdpPort("+5000"));
    }
}
