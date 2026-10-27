package com.limelight.binding.input.driver;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.junit.runners.Parameterized;

import java.util.ArrayList;
import java.util.Collection;

import static org.junit.Assert.*;

@RunWith(Parameterized.class)
public class DualSenseTriggerEffectTest {
    @Parameterized.Parameters(name = "effectType={0}")
    public static Collection<Object[]> effectTypes() {
        Collection<Object[]> types = new ArrayList<>();
        // The host forwards raw types: off, feedback, weapon, vibration, and firmware-specific effects.
        for (int i = 0; i <= 255; i++) {
            types.add(new Object[]{i});
        }
        return types;
    }

    private final int type;

    public DualSenseTriggerEffectTest(int type) {
        this.type = type;
    }

    @Test
    public void forwardsEveryHostEffectTypeAndAllTenBytesToTheCorrectTrigger() {
        byte[] left = DualSenseReportTest.hex("80 01 fe 03 fc 05 fa 07 f8 09");
        byte[] right = DualSenseReportTest.hex("ff 7e fd 7c fb 7a f9 78 f7 76");
        DualSenseReport.Output output = new DualSenseReport.Output(false);
        byte[] actual = output.adaptiveTriggers((byte)0x0C, (byte)type, (byte)(255 - type), left, right);
        byte[] expected = DualSenseReportTest.hex(
                "02 0f 00 00 00 00 00 00 00 00 00 00 ff 7e fd 7c " +
                "fb 7a f9 78 f7 76 00 80 01 fe 03 fc 05 fa 07 f8 " +
                "09 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00");
        expected[11] = (byte)(255 - type);
        expected[22] = (byte)type;
        assertArrayEquals(expected, actual);
        left[0] = right[0] = 0;
        assertArrayEquals(expected, actual);
        byte[] rumble = output.rumble((short)0, (short)0);
        assertEquals(3, rumble[1]);
        assertEquals((byte)0xFF, rumble[12]);
        assertEquals((byte)0x80, rumble[23]);
    }
}
