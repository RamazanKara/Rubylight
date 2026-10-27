package com.limelight.binding.input;

import org.junit.Test;
import java.util.ArrayList;
import java.util.List;
import static org.junit.Assert.*;

public class InputBatcherTest {
    private final List<String> sent = new ArrayList<>();
    private final InputBatcher batcher = new InputBatcher(m -> sent.add(m.kind + ":" + m.x + ":" + m.y));

    @Test public void sumsRelativeMotionButOnlyKeepsLatestAbsolutePosition() {
        batcher.mouse(0, (short) 4, (short) -2, (short) 0, (short) 0);
        batcher.mouse(0, (short) -1, (short) 7, (short) 0, (short) 0);
        assertTrue(sent.isEmpty());
        batcher.flush();
        batcher.mouse(1, (short) 100, (short) 200, (short) 1920, (short) 1080);
        batcher.mouse(1, (short) 110, (short) 180, (short) 1920, (short) 1080);
        batcher.flush();
        batcher.flush();
        assertEquals(List.of("0:3:5", "1:110:180"), sent);
    }

    @Test public void overflowAndCoordinateChangesFlushWithoutLosingMotion() {
        batcher.mouse(0, (short) 30000, (short) -30000, (short) 0, (short) 0);
        batcher.mouse(0, (short) 10000, (short) -10000, (short) 0, (short) 0);
        batcher.mouse(2, (short) 5, (short) 6, (short) 800, (short) 600);
        batcher.mouse(2, (short) 7, (short) 8, (short) 1600, (short) 900);
        batcher.flush();
        assertEquals(List.of("0:30000:-30000", "0:10000:-10000", "2:5:6", "2:7:8"), sent);
    }

    private void pad(int number, int mask, int buttons, int trigger, String value) {
        batcher.controller((short) number, (short) mask, buttons, (byte) trigger, (byte) 0, () -> sent.add(value));
    }

    @Test public void coalescesPerControllerAndPreservesShortButtonAndTriggerPresses() {
        pad(0, 3, 0, 0, "connect0");
        pad(1, 3, 0, 0, "connect1");
        pad(0, 3, 0, 0, "old");
        pad(1, 3, 0, 0, "pad1");
        pad(0, 3, 0, 0, "latest");
        pad(0, 3, 1, 0, "press");
        pad(0, 3, 0, 0, "release");
        pad(0, 3, 0, 1, "trigger");
        pad(0, 3, 0, 127, "half");
        pad(0, 3, 0, 255, "full");
        pad(0, 3, 0, 0, "triggerRelease");
        batcher.flush();
        assertEquals(List.of("connect0", "connect1", "latest", "pad1", "press", "release",
                "trigger", "full", "triggerRelease"), sent);
    }

    @Test public void disconnectAndClearCannotReplayStaleInput() {
        pad(0, 1, 0, 0, "connect");
        pad(0, 1, 0, 0, "move");
        pad(0, 0, 0, 0, "detach");
        batcher.mouse(0, (short) 1, (short) 2, (short) 0, (short) 0);
        batcher.clear();
        batcher.flush();
        assertEquals(List.of("connect", "move", "detach"), sent);
    }

    @Test public void buttonBoundaryFlushesMotionBeforeTheClick() {
        batcher.mouse(0, (short) 2, (short) 3, (short) 0, (short) 0);
        batcher.boundary(() -> sent.add("click"));
        batcher.mouse(0, (short) 4, (short) 5, (short) 0, (short) 0);
        batcher.flush();
        assertEquals(List.of("0:2:3", "click", "0:4:5"), sent);
    }

    @Test public void pollingUsesFractionalPanelRateAndSafeFallback() {
        assertEquals(8_333_333, InputBatcher.intervalNs(120));
        assertEquals(8_341_675, InputBatcher.intervalNs(119.88f));
        assertEquals(16_666_667, InputBatcher.intervalNs(Float.NaN));
        assertEquals(16_666_667, InputBatcher.intervalNs(0));
        assertEquals(1_000_000, InputBatcher.intervalNs(1001));
    }
}
