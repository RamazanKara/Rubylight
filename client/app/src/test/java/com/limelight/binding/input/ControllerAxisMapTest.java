package com.limelight.binding.input;

import org.junit.Test;
import static org.junit.Assert.*;
import static com.limelight.nvstream.input.ControllerPacket.*;

public class ControllerAxisMapTest {
    @Test public void defaultPreservesAllButtonsAndUnsignedTriggers() {
        ControllerAxisMap.Report r = new ControllerAxisMap().apply(UP_FLAG | A_FLAG | PADDLE1_FLAG,
                (byte)255, (byte)128, (short)-20000, (short)30000, (short)123, (short)-456);
        assertEquals(UP_FLAG | A_FLAG | PADDLE1_FLAG, r.buttons);
        assertEquals(255, Byte.toUnsignedInt(r.lt)); assertEquals(128, Byte.toUnsignedInt(r.rt));
        assertEquals(-20000, r.lx); assertEquals(30000, r.ly); assertEquals(123, r.rx); assertEquals(-456, r.ry);
    }
    @Test public void swapsAndInvertsFromOriginalReportWithoutChaining() {
        ControllerAxisMap map = new ControllerAxisMap();
        map.setAxis(0, 2, true); map.setAxis(2, 0, false);
        map.setAxis(4, 5, false); map.setAxis(5, 4, false);
        ControllerAxisMap.Report r = map.apply(0, (byte)40, (byte)220, (short)123, (short)0, Short.MIN_VALUE, (short)0);
        assertEquals(32767, r.lx); assertEquals(123, r.rx);
        assertEquals(220, Byte.toUnsignedInt(r.lt)); assertEquals(40, Byte.toUnsignedInt(r.rt));
        r = map.apply(0, (byte)0, (byte)0, (short)0, (short)0, (short)0, (short)0);
        assertEquals(0, r.lx); assertEquals(0, r.rx); assertEquals(0, r.lt); assertEquals(0, r.rt);
    }
    @Test public void dpadSharedTargetStaysHeldWhenEitherPhysicalSourceReleases() {
        ControllerAxisMap map = new ControllerAxisMap(); map.setDpad(0, A_FLAG); map.setDpad(1, A_FLAG);
        for (int buttons : new int[] {UP_FLAG | DOWN_FLAG | A_FLAG, DOWN_FLAG | A_FLAG, A_FLAG, DOWN_FLAG}) {
            assertEquals(A_FLAG, map.apply(buttons, (byte)0, (byte)0, (short)0, (short)0, (short)0, (short)0).buttons);
        }
        assertEquals(0, map.apply(0, (byte)0, (byte)0, (short)0, (short)0, (short)0, (short)0).buttons);
    }
    @Test public void disabledAxesDpadAndProfileValidation() {
        ControllerAxisMap map = new ControllerAxisMap(); map.setAxis(0, -1, false); map.setDpad(2, 0);
        ControllerAxisMap restored = ControllerAxisMap.deserialize(map.serialize());
        assertEquals(map.serialize(), restored.serialize());
        assertEquals(0, restored.apply(LEFT_FLAG, (byte)0, (byte)0, (short)1234, (short)0, (short)0, (short)0).lx);
        assertEquals(0, restored.apply(LEFT_FLAG, (byte)0, (byte)0, (short)0, (short)0, (short)0, (short)0).buttons);
        assertThrows(IllegalArgumentException.class, () -> map.setAxis(0, 5, false));
        assertThrows(IllegalArgumentException.class, () -> map.setAxis(5, 4, true));
        assertTrue(ControllerAxisMap.deserialize("1,2147483647,2,3,4,5,6,1,2,4,8").isEmpty());
        assertTrue(ControllerAxisMap.deserialize("bad").isEmpty());
    }
}
