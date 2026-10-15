package com.limelight.binding.input;

import com.limelight.nvstream.input.ControllerPacket;
import org.junit.Test;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import static org.junit.Assert.*;

public class LayoutGamepadTest {
    private final List<String> reports = new ArrayList<>();
    private final LayoutGamepad pad = new LayoutGamepad(new HeldInputState.Sink() {
        @Override public void key(int key, boolean down) { fail("Gamepad only"); }
        @Override public void mouse(int button, boolean down) { fail("Gamepad only"); }
        @Override public void gamepad(int buttons, short lx, short ly, short rx, short ry, byte lt, byte rt) {
            reports.add(Integer.toHexString(buttons) + " " + lx + "," + ly + " " + rx + "," + ry + " " + (lt & 0xFF) + "," + (rt & 0xFF));
        }
    });
    private static final String NEUTRAL = "0 0,0 0,0 0,0";

    @Test public void releaseOnPauseRecentersSticksZeroesTriggersAndLiftsButtonsInOneReport() {
        pad.setButtons(0, ControllerPacket.A_FLAG);
        pad.setStick(1, TouchLayout.LEFT, .5f, -1);
        pad.setStick(2, TouchLayout.RIGHT, 1, 0);
        pad.setTrigger(3, TouchLayout.RIGHT, 1);
        pad.setButtons(4, ControllerPacket.UP_FLAG | ControllerPacket.LEFT_FLAG);
        assertEquals("1005 16383,-32766 32766,0 0,255", reports.get(reports.size() - 1));
        reports.clear();
        pad.releaseAll();
        assertEquals(Collections.singletonList(NEUTRAL), reports);
        assertFalse(pad.holds(0) || pad.holds(1) || pad.holds(3));
        // Nothing is held any more, so a second release (pause after focus loss) sends nothing.
        pad.releaseAll();
        assertEquals(Collections.singletonList(NEUTRAL), reports);
    }

    @Test public void releasingBeforeAnyInputSendsNothing() {
        pad.releaseAll();
        pad.resend();
        assertTrue(reports.isEmpty());
    }

    @Test public void duplicateButtonsReleaseOnlyTheirOwnPress() {
        pad.setButtons(0, ControllerPacket.A_FLAG);
        pad.setButtons(5, ControllerPacket.A_FLAG);
        pad.setButtons(0, 0);
        assertEquals("1000 0,0 0,0 0,0", reports.get(reports.size() - 1));
        pad.setButtons(5, 0);
        assertEquals(NEUTRAL, reports.get(reports.size() - 1));
        // Unchanged state sends no duplicate report.
        assertEquals(2, reports.size());
    }

    @Test public void twoSticksOnOneSideReportTheOnePushedFurther() {
        pad.setStick(0, TouchLayout.LEFT, .2f, 0);
        pad.setStick(1, TouchLayout.LEFT, 0, .9f);
        assertEquals("0 0,29489 0,0 0,0", reports.get(reports.size() - 1));
        pad.setStick(1, TouchLayout.LEFT, 0, 0);
        assertEquals("0 6553,0 0,0 0,0", reports.get(reports.size() - 1));
        pad.setTrigger(2, TouchLayout.LEFT, .5f);
        assertEquals("0 6553,0 0,0 128,0", reports.get(reports.size() - 1));
    }

    @Test public void resendRepeatsTheLastReport() {
        pad.setButtons(0, ControllerPacket.B_FLAG);
        pad.resend();
        assertEquals(Arrays.asList("2000 0,0 0,0 0,0", "2000 0,0 0,0 0,0"), reports);
    }
}
