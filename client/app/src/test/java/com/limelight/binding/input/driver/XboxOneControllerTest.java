package com.limelight.binding.input.driver;

import org.junit.Test;

import static org.junit.Assert.*;

public class XboxOneControllerTest {
    @Test
    public void rumbleUsesUnsignedMotorStrengthsAtTheSignBoundary() {
        assertArrayEquals(new byte[] {9, 0, 42, 9, 0, 15, 0, 63, 64, 127, -1, 0, -1},
                XboxOneController.createRumblePacket((byte) 42, (short) 0x8000, (short) 0xFFFF,
                        (short) 0, (short) 0x7FFF));
    }

    @Test
    public void allFourMotorsIncreaseMonotonicallyOverTheUnsignedRange() {
        for (int strength = 0; strength <= 0xFFFF; strength++) {
            byte[] packet = XboxOneController.createRumblePacket((byte) 0, (short) strength,
                    (short) strength, (short) strength, (short) strength);
            for (int motor = 6; motor <= 9; motor++) {
                assertEquals(strength >> 9, packet[motor] & 0xFF);
            }
        }
    }
}
