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

    private static final int C = 98, Z = 101, BUTTON_16 = 203, MEDIA_RECORD = 130;

    @Test
    public void extraButtonsMapToTheExtendedButtonsTheHostPassesThrough() {
        ControllerButtonMap map = new ControllerButtonMap();
        map.put(C, ControllerButtonMap.TARGET_TOUCHPAD);
        map.put(Z, ControllerButtonMap.TARGET_MISC);
        map.put(BUTTON_1, ControllerButtonMap.TARGET_PADDLE1);
        map.put(BUTTON_16, ControllerButtonMap.TARGET_PADDLE4);
        map.put(MEDIA_RECORD, ControllerButtonMap.TARGET_PADDLE2);
        assertEquals(ControllerButtonMap.TARGET_TOUCHPAD, map.map(C));
        assertEquals(ControllerButtonMap.TARGET_MISC, map.map(Z));
        assertEquals(TOUCHPAD_FLAG | MISC_FLAG | PADDLE1_FLAG | PADDLE2_FLAG | PADDLE4_FLAG, map.targetButtonFlags());
        assertEquals(0, new ControllerButtonMap().targetButtonFlags());

        // Standard targets add no extended flags to the arrival.
        ControllerButtonMap standard = new ControllerButtonMap();
        standard.put(C, A);
        assertEquals(0, standard.targetButtonFlags());

        // Extended buttons are targets only: nothing presses them as Android keys.
        assertThrows(IllegalArgumentException.class, () -> map.put(ControllerButtonMap.TARGET_TOUCHPAD, A));
    }

    @Test
    public void usbReportsMapStandardButtonsToExtendedButtons() {
        ControllerButtonMap map = new ControllerButtonMap();
        map.put(A, ControllerButtonMap.TARGET_TOUCHPAD);
        map.put(B, ControllerButtonMap.TARGET_MISC);
        map.put(X, ControllerButtonMap.TARGET_PADDLE3);
        assertEquals(TOUCHPAD_FLAG | MISC_FLAG | PADDLE3_FLAG | Y_FLAG,
                map.mapButtonFlags(A_FLAG | B_FLAG | X_FLAG | Y_FLAG, false));
        // A real touchpad click still passes through beside a mapped one.
        assertEquals(TOUCHPAD_FLAG, map.mapButtonFlags(TOUCHPAD_FLAG, false));
        assertEquals(TOUCHPAD_FLAG, map.mapButtonFlags(A_FLAG | TOUCHPAD_FLAG, false));
        assertEquals(0, map.mapButtonFlags(0, false));
    }

    @Test
    public void storedMappingsFromEarlierVersionsLoadUnchanged() {
        // The value format and key codes of earlier versions: C to A, Z disabled, 1 to Start, 16 to Guide.
        String earlier = "98:96,101:0,188:108,203:110";
        ControllerButtonMap map = ControllerButtonMap.deserialize(earlier);
        assertEquals(earlier, map.serialize());
        assertEquals(A, map.map(C));
        assertEquals(ControllerButtonMap.DISABLED, map.map(Z));
        assertEquals(START, map.map(BUTTON_1));
        assertEquals(A_FLAG, map.mapButtonFlags(A_FLAG, false));

        // Extended targets are stored beside them, and a code this version does not know is skipped.
        map.put(X, ControllerButtonMap.TARGET_TOUCHPAD);
        ControllerButtonMap restored = ControllerButtonMap.deserialize(map.serialize() + ",100:65543");
        assertEquals("98:96,99:65537,101:0,188:108,203:110", restored.serialize());
        assertEquals(ControllerButtonMap.TARGET_TOUCHPAD, restored.map(X));
        assertEquals(ControllerButtonMap.UNMAPPED, restored.map(Y));
    }

    @Test
    public void connectAsSurvivesStorageBesideTheMappings() {
        ControllerButtonMap map = new ControllerButtonMap();
        assertEquals(ControllerButtonMap.CONNECT_AS_AUTOMATIC, map.connectAs());
        assertFalse(map.isConnectAsPinned());
        map.put(C, A);
        map.setConnectAs(ControllerButtonMap.CONNECT_AS_XBOX);
        assertEquals("98:96,131072:1", map.serialize());

        ControllerButtonMap restored = ControllerButtonMap.deserialize(map.serialize());
        assertEquals(ControllerButtonMap.CONNECT_AS_XBOX, restored.connectAs());
        assertTrue(restored.isConnectAsPinned());
        assertEquals(A, restored.map(C));
        assertEquals(1, restored.entries().size());

        // A pinned type alone is worth storing; Automatic alone is not.
        ControllerButtonMap pinnedOnly = new ControllerButtonMap();
        pinnedOnly.setConnectAs(ControllerButtonMap.CONNECT_AS_NINTENDO);
        assertFalse(pinnedOnly.isEmpty());
        assertTrue(pinnedOnly.entries().isEmpty());
        assertEquals("131072:3", pinnedOnly.serialize());
        pinnedOnly.setConnectAs(ControllerButtonMap.CONNECT_AS_AUTOMATIC);
        assertTrue(pinnedOnly.isEmpty());
        assertEquals("", pinnedOnly.serialize());

        assertThrows(IllegalArgumentException.class, () -> map.setConnectAs(4));
        assertThrows(IllegalArgumentException.class, () -> map.setConnectAs(-1));
    }

    @Test
    public void connectAsIsSkippedByEarlierVersionsAndMissingFromTheirMaps() {
        // Maps stored before the choice existed connect as Automatic.
        ControllerButtonMap earlier = ControllerButtonMap.deserialize("98:96,99:65537");
        assertEquals(ControllerButtonMap.CONNECT_AS_AUTOMATIC, earlier.connectAs());
        assertEquals("98:96,99:65537", earlier.serialize());

        // An earlier version parses every entry with put(), which refuses the connect-as entry,
        // so it keeps the buttons and drops only the choice.
        ControllerButtonMap pinned = ControllerButtonMap.deserialize("98:96");
        pinned.setConnectAs(ControllerButtonMap.CONNECT_AS_PLAYSTATION);
        ControllerButtonMap olderLoad = new ControllerButtonMap();
        for (String entry : pinned.serialize().split(",")) {
            String[] parts = entry.split(":");
            try {
                olderLoad.put(Integer.parseInt(parts[0]), Integer.parseInt(parts[1]));
            } catch (IllegalArgumentException ignored) {
            }
        }
        assertEquals("98:96", olderLoad.serialize());

        // Unknown or malformed types load as Automatic without losing the buttons.
        ControllerButtonMap future = ControllerButtonMap.deserialize("98:96,131072:4,131072:x");
        assertEquals(ControllerButtonMap.CONNECT_AS_AUTOMATIC, future.connectAs());
        assertEquals(A, future.map(C));
    }

    @Test
    public void connectAsCodesAreTheProtocolControllerTypes() {
        assertEquals(com.limelight.nvstream.jni.MoonBridge.LI_CTYPE_UNKNOWN, ControllerButtonMap.CONNECT_AS_AUTOMATIC);
        assertEquals(com.limelight.nvstream.jni.MoonBridge.LI_CTYPE_XBOX, ControllerButtonMap.CONNECT_AS_XBOX);
        assertEquals(com.limelight.nvstream.jni.MoonBridge.LI_CTYPE_PS, ControllerButtonMap.CONNECT_AS_PLAYSTATION);
        assertEquals(com.limelight.nvstream.jni.MoonBridge.LI_CTYPE_NINTENDO, ControllerButtonMap.CONNECT_AS_NINTENDO);
    }

    @Test
    public void deviceKeyGroupsModelsAndFallsBackToName() {
        assertEquals("usb:045e:0b13", ControllerButtonMap.deviceKey(0x045e, 0x0b13, "Xbox Wireless Controller"));
        assertEquals("usb:054c:0ce6", ControllerButtonMap.deviceKey(0x054c, 0x0ce6, null));
        assertEquals("name:Generic Pad", ControllerButtonMap.deviceKey(0, 0, " Generic Pad "));
        assertEquals("name:", ControllerButtonMap.deviceKey(0, 0, null));
    }
}
