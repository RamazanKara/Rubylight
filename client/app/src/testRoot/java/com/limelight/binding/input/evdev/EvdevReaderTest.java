package com.limelight.binding.input.evdev;

import org.junit.Test;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;

import static org.junit.Assert.*;

public class EvdevReaderTest {
    @Test
    public void truncatedFramesFailWithoutReturningPartialEvents() {
        ByteBuffer packet = ByteBuffer.allocate(4 + 23).order(ByteOrder.nativeOrder());
        packet.putInt(24);
        assertThrows(IOException.class,
                () -> EvdevReader.read(new ByteArrayInputStream(packet.array())));
    }

    @Test
    public void rejectsLengthsOtherThanNativeEventLayoutsBeforeReadingThePayload() throws Exception {
        for (int length : new int[] {-1, 0, 15, 17, 23, 25, 1024}) {
            ByteBuffer packet = ByteBuffer.allocate(4 + 25).order(ByteOrder.nativeOrder());
            packet.putInt(length);
            ByteArrayInputStream input = new ByteArrayInputStream(packet.array());
            assertNull("Accepted event size " + length, EvdevReader.read(input));
            assertEquals(25, input.available());
        }
    }

    @Test
    public void bothNativeEventLayoutsKeepSignedValuesAndDiscardTimestamps() throws Exception {
        for (int length : new int[] {16, 24}) {
            ByteBuffer packet = ByteBuffer.allocate(4 + length).order(ByteOrder.nativeOrder());
            packet.putInt(length);
            packet.position(4 + length - 8);
            packet.putShort(EvdevEvent.EV_REL).putShort(EvdevEvent.REL_X).putInt(-123);
            EvdevEvent event = EvdevReader.read(new ByteArrayInputStream(packet.array()));
            assertNotNull(event);
            assertEquals(EvdevEvent.EV_REL, event.type);
            assertEquals(EvdevEvent.REL_X, event.code);
            assertEquals(-123, event.value);
        }
    }
}
