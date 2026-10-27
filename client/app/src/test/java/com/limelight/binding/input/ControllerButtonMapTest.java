package com.limelight.binding.input;

import org.junit.Test;

import static org.junit.Assert.*;
import static com.limelight.nvstream.input.ControllerPacket.*;

public class ControllerButtonMapTest {
    @Test
    public void usbReportsUseTheSameModelMappingsWithoutChaining() {
        ControllerButtonMap map = new ControllerButtonMap();
        map.put(96, 97);
        map.put(97, 99);
        assertEquals(B_FLAG, map.mapButtonFlags(A_FLAG, false));
        assertEquals(B_FLAG | X_FLAG, map.mapButtonFlags(A_FLAG | B_FLAG, false));
        assertEquals(0, map.mapButtonFlags(0, false));
    }

    @Test
    public void usbMergedTargetsRemainPressedUntilAllSourcesRelease() {
        ControllerButtonMap map = new ControllerButtonMap();
        map.put(97, 96);
        assertEquals(A_FLAG, map.mapButtonFlags(A_FLAG | B_FLAG, false));
        assertEquals(A_FLAG, map.mapButtonFlags(B_FLAG, false));
        assertEquals(A_FLAG, map.mapButtonFlags(A_FLAG, false));
        assertEquals(0, map.mapButtonFlags(0, false));
    }

    @Test
    public void usbBindingsOverrideFaceSwapAndPreservePaddlesAndTouchpad() {
        ControllerButtonMap map = new ControllerButtonMap();
        map.put(96, 99);
        map.put(97, ControllerButtonMap.DISABLED);
        int extended = PADDLE1_FLAG | TOUCHPAD_FLAG | MISC_FLAG | UP_FLAG;
        assertEquals(X_FLAG | extended, map.mapButtonFlags(A_FLAG | B_FLAG | extended, true));
        assertEquals(Y_FLAG, map.mapButtonFlags(X_FLAG, true));
        assertEquals(A_FLAG | B_FLAG | X_FLAG | Y_FLAG,
                new ControllerButtonMap().mapButtonFlags(A_FLAG | B_FLAG | X_FLAG | Y_FLAG, true));
    }

    private static final int A = 96, B = 97, X = 99, Y = 100, L2 = 104, START = 108, BUTTON_1 = 188;
    private static final int DPAD_UP = 19;

    @Test
    public void mappingsSurviveStorageAndUnmappedButtonsPassThrough() {
        ControllerButtonMap map = new ControllerButtonMap();
        map.put(Y, A);
        map.put(X, ControllerButtonMap.DISABLED);
        map.put(BUTTON_1, START);

        ControllerButtonMap restored = ControllerButtonMap.deserialize(map.serialize());
        assertEquals("99:0,100:96,188:108", restored.serialize());
        assertEquals(A, restored.map(Y));
        assertEquals(ControllerButtonMap.DISABLED, restored.map(X));
        assertEquals(START, restored.map(BUTTON_1));
        assertEquals(ControllerButtonMap.UNMAPPED, restored.map(B));
    }

    @Test
    public void identityMappingAndRemovalRestoreDefault() {
        ControllerButtonMap map = new ControllerButtonMap();
        map.put(Y, A);
        map.put(Y, Y);
        assertTrue(map.isEmpty());
        map.put(B, A);
        map.remove(B);
        assertTrue(map.isEmpty());
        assertEquals("", map.serialize());
    }

    @Test
    public void dpadAndTriggerKeysCannotBeRemapped() {
        ControllerButtonMap map = new ControllerButtonMap();
        assertFalse(ControllerButtonMap.isRemappableSource(DPAD_UP));
        assertFalse(ControllerButtonMap.isRemappableSource(L2));
        assertThrows(IllegalArgumentException.class, () -> map.put(L2, A));
        assertThrows(IllegalArgumentException.class, () -> map.put(A, L2));
        assertThrows(IllegalArgumentException.class, () -> map.put(A, DPAD_UP));
    }

    @Test
    public void malformedStoredValuesAreIgnored() {
        ControllerButtonMap map = ControllerButtonMap.deserialize("bad,100:96,104:96,97:19,:,98:x,1:2:3");
        assertEquals("100:96", map.serialize());
        assertTrue(ControllerButtonMap.deserialize(null).isEmpty());
        assertTrue(ControllerButtonMap.deserialize("").isEmpty());
    }

    @Test
    public void deviceKeyGroupsModelsAndFallsBackToName() {
        assertEquals("usb:045e:0b13", ControllerButtonMap.deviceKey(0x045e, 0x0b13, "Xbox Wireless Controller"));
        assertEquals("usb:054c:0ce6", ControllerButtonMap.deviceKey(0x054c, 0x0ce6, null));
        assertEquals("name:Generic Pad", ControllerButtonMap.deviceKey(0, 0, " Generic Pad "));
        assertEquals("name:", ControllerButtonMap.deviceKey(0, 0, null));
    }
}
