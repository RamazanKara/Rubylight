package com.limelight.binding.video;

import static org.junit.Assert.*;

import org.junit.Test;

/** The probe's answer and its log line, from made-up native results (window_dataspace_test.cpp tests the calls). */
public class WindowDataspaceTest {
    private static final int PQ = 163971072;

    @Test
    public void thePublicCallTakingHlgIsNamed() {
        WindowDataspace.Probe probe = new WindowDataspace.Probe(WindowDataspace.CALL_PUBLIC,
                new int[] {0, WindowDataspace.NOT_CALLED, PQ, 0, WindowDataspace.NOT_CALLED});
        assertEquals(WindowDataspace.CALL_PUBLIC, probe.call);
        assertEquals("ANativeWindow_setBuffersDataSpace took BT2020_HLG; dataspace 0x9c60000 put back", probe.describe());
    }

    @Test
    public void performTakingWhatThePublicCallRefusedIsNamed() {
        WindowDataspace.Probe probe = new WindowDataspace.Probe(WindowDataspace.CALL_PERFORM,
                new int[] {-22, 0, PQ, 0, WindowDataspace.NOT_CALLED});
        assertEquals(WindowDataspace.CALL_PERFORM, probe.call);
        assertEquals("ANativeWindow_setBuffersDataSpace refused BT2020_HLG (-22); " +
                "ANativeWindow::perform(NATIVE_WINDOW_SET_BUFFERS_DATASPACE) took it; dataspace 0x9c60000 put back",
                probe.describe());
    }

    @Test
    public void bothRefusalsAreNamed() {
        WindowDataspace.Probe probe = new WindowDataspace.Probe(WindowDataspace.CALL_NONE,
                new int[] {-22, -22, PQ, WindowDataspace.NOT_CALLED, WindowDataspace.NOT_CALLED});
        assertFalse(probe.accepted());
        assertEquals("ANativeWindow_setBuffersDataSpace refused BT2020_HLG (-22); " +
                "ANativeWindow::perform(NATIVE_WINDOW_SET_BUFFERS_DATASPACE) refused it (-22)", probe.describe());

        probe = new WindowDataspace.Probe(WindowDataspace.CALL_NONE,
                new int[] {WindowDataspace.MISSING, WindowDataspace.NOT_MIRRORED, -1, 1, 1});
        assertEquals("ANativeWindow_setBuffersDataSpace refused BT2020_HLG (missing); " +
                "ANativeWindow::perform was not called: the window is not the platform's ANativeWindow", probe.describe());
    }

    @Test
    public void aFailedPutBackIsSaid() {
        WindowDataspace.Probe probe = new WindowDataspace.Probe(WindowDataspace.CALL_PERFORM,
                new int[] {-22, 0, -1, -22, -19});
        assertEquals(WindowDataspace.CALL_PERFORM, probe.call);
        assertTrue(probe.describe(), probe.describe().endsWith(
                "; putting UNKNOWN (the window did not say its dataspace) back failed (-22, perform -19)"));
    }

    @Test
    public void anUnreachableWindowIsRefusedAndSaysWhy() {
        WindowDataspace.Probe probe = new WindowDataspace.Probe(WindowDataspace.CALL_NONE, new int[] {
                WindowDataspace.NOT_CALLED, WindowDataspace.NOT_CALLED, -1, WindowDataspace.NOT_CALLED,
                WindowDataspace.NOT_CALLED});
        assertFalse(probe.accepted());
        assertEquals("the window was not asked (the Surface gave no ANativeWindow)", probe.describe());
        assertFalse(WindowDataspace.probeHlg(null).accepted());
    }
}
