package com.limelight.binding.input.driver;

import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.*;

public class DualSenseReportTest {
    @Test
    public void waveformModeSurvivesOtherFeedbackAndRestoresOrdinaryRumble() {
        for (boolean enhanced : new boolean[] {false, true}) {
            DualSenseReport.Output output = new DualSenseReport.Output(enhanced);
            output.rumble((short)0xffff, (short)0xffff);
            byte[] report = output.waveform(true);
            assertEquals(0, report[1] & 3);
            assertEquals(0, report[39] & 4);
            assertEquals(0, report[3] | report[4]);
            assertNull(output.rumble((short)0xffff, (short)0xffff));
            report = output.led((byte)1, (byte)2, (byte)3);
            assertEquals(0, report[1] & 3);
            assertEquals(0, report[39] & 4);
            byte[] effect = new byte[10];
            effect[0] = 12;
            report = output.adaptiveTriggers((byte)0x0c, (byte)1, (byte)2, effect, effect);
            assertEquals(0x0c, report[1] & 0xff);
            assertEquals(12, report[12]);
            assertEquals(12, report[23]);
            report = output.waveform(false);
            assertEquals(0, report[3] | report[4]);
            report = output.rumble((short)0xffff, (short)0xffff);
            assertEquals(enhanced ? 255 : 127, report[3] & 0xff);
            assertEquals(enhanced ? 2 : 3, report[1] & 3);
            output.waveform(true);
            report = output.reset();
            assertEquals(enhanced ? 2 : 3, report[1] & 3);
            assertEquals(0, report[3] | report[4]);
        }
    }

    // Physical USB captures, Total Phase sniffer, DS5 -> PS5; unmodified 64-byte reports:
    // https://github.com/matlo/GIMX/issues/672#issuecomment-1019605142
    private static final byte[][] CAPTURED_USB = {
            hex("01 7f 83 83 7d 00 00 66 08 00 00 00 af d5 c6 68 " +
                "fa ff fd ff 05 00 32 00 40 1f 8d 05 f7 f5 e4 00 " +
                "01 80 00 00 00 80 00 00 00 00 09 09 00 00 00 00 " +
                "00 96 f8 e4 00 2a 08 00 42 31 48 e4 b4 54 34 31"),
            hex("01 7f 83 83 7e 00 00 76 08 00 00 00 bf d5 c6 68 " +
                "fc ff ff ff 03 00 37 00 42 1f 82 05 ee b0 e5 00 " +
                "01 80 00 00 00 80 00 00 00 00 09 09 00 00 00 00 " +
                "00 16 b4 e5 00 2a 08 00 45 f3 a0 df bb 82 1d 49")
    };

    @Test
    public void capturedUsbReportsDecodeDespiteNonzeroReservedBytes() {
        for (int i = 0; i < CAPTURED_USB.length; i++) {
            DualSenseReport.Input input = parse(CAPTURED_USB[i], false);
            assertEquals(0, input.buttons);
            assertEquals(-1 / 128.0f, input.leftStickX, 0);
            assertEquals(3 / 127.0f, input.leftStickY, 0);
            assertEquals(3 / 127.0f, input.rightStickX, 0);
            assertEquals((i == 0 ? -3 : -2) / 128.0f, input.rightStickY, 0);
            assertEquals(0, input.leftTrigger, 0);
            assertEquals(0, input.rightTrigger, 0);
            assertArrayEquals(new boolean[]{false, false}, input.touching);
            assertEquals(MoonBridge.LI_BATTERY_STATE_FULL, input.batteryState);
            assertEquals(100, input.batteryPercentage);
            new DualSenseReport.Calibration().apply(input);
            assertArrayEquals(i == 0 ? new float[]{-0.375f, -0.1875f, 0.3125f} :
                    new float[]{-0.25f, -0.0625f, 0.1875f}, input.gyro, 0);
            float unit = 9.80665f / 8192;
            assertArrayEquals(i == 0 ? new float[]{50 * unit, 8000 * unit, 1421 * unit} :
                    new float[]{55 * unit, 8002 * unit, 1410 * unit}, input.accel, 0.000001f);
        }
    }

    // Synthetic edge cases complement the physical captures above.
    private static final byte[] NEUTRAL = hex(
            "01 80 80 80 80 00 00 00 08 00 00 00 00 00 00 00 " +
            "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 " +
            "00 80 00 00 00 80 00 00 00 00 00 00 00 00 00 00 " +
            "00 00 00 00 00 2a 08 00 00 00 00 00 00 00 00 00");
    private static final byte[] ACTIVE = hex(
            "01 00 ff 40 c0 80 ff 5a f1 f3 07 f0 12 34 56 78 " +
            "40 06 80 f3 00 40 00 20 00 e0 00 10 12 34 56 78 " +
            "00 01 c0 73 21 7f 80 e7 42 00 00 00 00 00 00 00 " +
            "00 00 00 00 00 17 08 00 00 00 00 00 00 00 00 00");

    static byte[] hex(String text) {
        String[] bytes = text.split(" ");
        byte[] result = new byte[bytes.length];
        for (int i = 0; i < bytes.length; i++) {
            result[i] = (byte)Integer.parseInt(bytes[i], 16);
        }
        return result;
    }

    private static DualSenseReport.Input parse(byte[] report, boolean edge) {
        DualSenseReport.Input input = new DualSenseReport.Input();
        assertTrue(input.parse(report, report.length, edge));
        return input;
    }

    @Test
    public void neutralUsbReportHasNoPressedControls() {
        DualSenseReport.Input input = parse(NEUTRAL, false);
        assertEquals(0, input.buttons);
        assertEquals(0, input.leftStickX, 0);
        assertEquals(0, input.leftStickY, 0);
        assertEquals(0, input.rightStickX, 0);
        assertEquals(0, input.rightStickY, 0);
        assertEquals(0, input.leftTrigger, 0);
        assertEquals(0, input.rightTrigger, 0);
        assertArrayEquals(new boolean[]{false, false}, input.touching);
        assertEquals(MoonBridge.LI_BATTERY_STATE_FULL, input.batteryState);
        assertEquals(100, input.batteryPercentage);
    }

    @Test
    public void goldenReportDecodesButtonsAxesTwoFingersMotionAndBattery() {
        DualSenseReport.Input input = parse(ACTIVE, false);
        assertEquals(0x30F7F9, input.buttons);
        assertEquals(-1, input.leftStickX, 0);
        assertEquals(1, input.leftStickY, 0);
        assertEquals(-0.5f, input.rightStickX, 0);
        assertEquals(64 / 127.0f, input.rightStickY, 0);
        assertEquals(128 / 255.0f, input.leftTrigger, 0);
        assertEquals(1, input.rightTrigger, 0);
        assertArrayEquals(new boolean[]{true, true}, input.touching);
        assertArrayEquals(new int[]{1, 127}, input.touchId);
        assertArrayEquals(new float[]{0.5f, 1}, input.touchX, 0.00001f);
        assertArrayEquals(new float[]{0.5f, 1}, input.touchY, 0.00001f);
        new DualSenseReport.Calibration().apply(input);
        assertArrayEquals(new float[]{100, -200, 1024}, input.gyro, 0.00001f);
        assertArrayEquals(new float[]{9.80665f, -9.80665f, 4.903325f}, input.accel, 0.00001f);
        assertEquals(MoonBridge.LI_BATTERY_STATE_CHARGING, input.batteryState);
        assertEquals(75, input.batteryPercentage);
    }

    @Test
    public void edgePaddlesAreIndependentAndFnButtonsAreIgnored() {
        byte[] report = NEUTRAL.clone();
        report[10] = 0x30;
        assertEquals(0, parse(report, true).buttons);
        report[10] = 0x70;
        assertEquals(ControllerPacket.PADDLE1_FLAG, parse(report, true).buttons);
        report[10] = (byte)0xB0;
        assertEquals(ControllerPacket.PADDLE2_FLAG, parse(report, true).buttons);
        report[10] = (byte)0xF0;
        assertEquals(ControllerPacket.PADDLE1_FLAG | ControllerPacket.PADDLE2_FLAG, parse(report, true).buttons);
        assertEquals(0, parse(report, false).buttons);
        assertEquals(0, DualSenseReport.EDGE_BUTTONS & (ControllerPacket.PADDLE3_FLAG | ControllerPacket.PADDLE4_FLAG));
    }

    @Test
    public void digitalTriggerStopsSupplyFullTravelOnlyWhenAnalogIsZero() {
        byte[] report = NEUTRAL.clone();
        report[9] = 0x0C;
        DualSenseReport.Input input = parse(report, true);
        assertEquals(1, input.leftTrigger, 0);
        assertEquals(1, input.rightTrigger, 0);
        assertEquals(0, input.buttons);
        report[5] = 64;
        report[6] = (byte)128;
        input = parse(report, true);
        assertEquals(64 / 255.0f, input.leftTrigger, 0);
        assertEquals(128 / 255.0f, input.rightTrigger, 0);
    }

    @Test
    public void allHatDirectionsAndButtonBitsAreIndependent() {
        int[] hats = {1, 9, 8, 10, 2, 6, 4, 5, 0, 0, 0, 0, 0, 0, 0, 0};
        byte[] report = NEUTRAL.clone();
        for (int i = 0; i < hats.length; i++) {
            report[8] = (byte)i;
            assertEquals(hats[i], parse(report, false).buttons);
        }
        int[] face = {ControllerPacket.X_FLAG, ControllerPacket.A_FLAG, ControllerPacket.B_FLAG, ControllerPacket.Y_FLAG};
        for (int i = 0; i < face.length; i++) {
            report[8] = (byte)(8 | (0x10 << i));
            assertEquals(face[i], parse(report, false).buttons);
        }
        report[8] = 8;
        int[] shoulders = {0x100, 0x200, 0, 0, 0x20, 0x10, 0x40, 0x80};
        for (int i = 0; i < shoulders.length; i++) {
            report[9] = (byte)(1 << i);
            assertEquals(shoulders[i], parse(report, false).buttons);
        }
        report[9] = 0;
        int[] special = {0x400, 0x100000, 0x200000, 0, 0, 0, 0, 0};
        for (int i = 0; i < special.length; i++) {
            report[10] = (byte)(1 << i);
            assertEquals(special[i], parse(report, false).buttons);
        }
    }

    @Test
    public void truncatedAndNonUsbReportsLeavePreviousStateIntact() {
        DualSenseReport.Input input = parse(ACTIVE, false);
        assertFalse(input.parse(NEUTRAL, 63, false));
        assertFalse(input.parse(new byte[0], 0, false));
        assertFalse(input.parse(NEUTRAL, 65, false));
        assertFalse(input.parse(new byte[63], 64, false));
        byte[] bluetooth = NEUTRAL.clone();
        bluetooth[0] = 0x31;
        assertFalse(input.parse(bluetooth, 64, false));
        assertEquals(0x30F7F9, input.buttons);
        assertEquals(75, input.batteryPercentage);
    }

    @Test
    public void batteryStatesClampCapacityAndReportUnknownErrors() {
        byte[] report = NEUTRAL.clone();
        int[] raw = {0x00, 0x09, 0x0F, 0x10, 0x1A, 0x20, 0xA0, 0xB0, 0xF0};
        int[] state = {2, 2, 2, 3, 3, 5, 0, 0, 0};
        int[] percent = {5, 95, 100, 5, 100, 100, 255, 255, 255};
        for (int i = 0; i < raw.length; i++) {
            report[53] = (byte)raw[i];
            DualSenseReport.Input input = parse(report, false);
            assertEquals(state[i], input.batteryState);
            assertEquals(percent[i], input.batteryPercentage & 0xFF);
        }
    }

    @Test
    public void touchContactsMoveReleaseAndSurviveSlotChanges() {
        TouchListener listener = new TouchListener();
        DualSenseReport.Input active = parse(ACTIVE, false);
        active.reportTouchChanges(parse(NEUTRAL, false), listener, 42);
        assertEquals(Arrays.asList(0x101, 0x17F), listener.events);
        listener.events.clear();
        byte[] report = ACTIVE.clone();
        System.arraycopy(ACTIVE, 37, report, 33, 4);
        System.arraycopy(ACTIVE, 33, report, 37, 4);
        DualSenseReport.Input swapped = parse(report, false);
        swapped.reportTouchChanges(active, listener, 42);
        assertTrue(listener.events.isEmpty());
        report[38]++;
        DualSenseReport.Input moved = parse(report, false);
        moved.reportTouchChanges(swapped, listener, 42);
        assertEquals(Arrays.asList(0x301), listener.events);
        listener.events.clear();
        parse(NEUTRAL, false).reportTouchChanges(moved, listener, 42);
        assertEquals(Arrays.asList(0x27F, 0x201), listener.events);
    }

    @Test
    public void reusedTouchSlotReleasesOldIdBeforeNewDown() {
        byte[] report = NEUTRAL.clone();
        report[33] = 127;
        DualSenseReport.Input previous = parse(report, false);
        report[33] = 0;
        TouchListener listener = new TouchListener();
        parse(report, false).reportTouchChanges(previous, listener, 42);
        assertEquals(Arrays.asList(0x27F, 0x100), listener.events);
    }

    @Test
    public void factoryCalibrationUsesBiasAndSensorUnits() {
        byte[] calibration = hex("05 10 00 e0 ff 40 00 " +
                "40 1f c0 e0 40 1f c0 e0 40 1f c0 e0 f4 01 f4 01 " +
                "4a 1f ca e0 2c 1f ac e0 5e 1f de e0 00 00 00 00 00 00");
        DualSenseReport.Calibration factory = new DualSenseReport.Calibration();
        factory.parse(calibration, calibration.length);
        DualSenseReport.Input input = parse(ACTIVE, false);
        factory.apply(input);
        assertArrayEquals(new float[]{99, -198, 1020}, input.gyro, 0.0001f);
        assertArrayEquals(new float[]{(8192 - 10) * 9.80665f / 8000,
                (-8192 + 20) * 9.80665f / 8000, (4096 - 30) * 9.80665f / 8000}, input.accel, 0.0001f);
    }

    @Test
    public void invalidCalibrationRetainsNominalConversion() {
        DualSenseReport.Calibration calibration = new DualSenseReport.Calibration();
        byte[] invalid = new byte[41];
        invalid[0] = 5;
        calibration.parse(invalid, 34);
        calibration.parse(invalid, 41);
        DualSenseReport.Input input = parse(ACTIVE, false);
        calibration.apply(input);
        assertArrayEquals(new float[]{100, -200, 1024}, input.gyro, 0.0001f);
    }

    @Test
    public void rumbleReportsMatchLegacyAndEnhancedUsbLayouts() {
        assertArrayEquals(hex("02 03 00 40 7f 00 00 00 00 00 00 00 00 00 00 00 " +
                "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 " +
                "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00"),
                new DualSenseReport.Output(false).rumble((short)0xFFFF, (short)0x8000));
        assertArrayEquals(hex("02 02 00 80 ff 00 00 00 00 00 00 00 00 00 00 00 " +
                "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 " +
                "00 00 00 00 00 00 00 04 00 00 00 00 00 00 00 00"),
                new DualSenseReport.Output(true).rumble((short)0xFFFF, (short)0x8000));
    }

    @Test
    public void ledReportHasUnsignedRgbAndPreservesRumble() {
        DualSenseReport.Output output = new DualSenseReport.Output(false);
        output.rumble((short)0xFFFF, (short)0x8000);
        assertArrayEquals(hex("02 03 04 40 7f 00 00 00 00 00 00 00 00 00 00 00 " +
                "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 " +
                "00 00 00 00 00 00 00 00 00 00 00 00 00 80 fe 01"),
                output.led((byte)0x80, (byte)0xFE, (byte)1));
    }

    @Test
    public void playerLedsUseAssignedPlayerNumberAndCanBeCleared() {
        DualSenseReport.Output output = new DualSenseReport.Output(true);
        int[] lights = {0x24, 0x2A, 0x35, 0x3B, 0x3F, 0x31, 0x2E};
        for (int i = 0; i < 16; i++) {
            byte[] report = output.player(i);
            assertEquals(48, report.length);
            assertEquals(0x10, report[2]);
            assertEquals(lights[i % lights.length], report[44]);
        }
        assertEquals(0, output.player(-1)[44]);
    }

    @Test
    public void partialTriggerUpdatesDoNotReapplyTheOtherTriggerOrOtherHostFlags() {
        DualSenseReport.Output output = new DualSenseReport.Output(false);
        byte[] left = hex("00 01 02 03 04 05 06 07 08 09");
        byte[] right = hex("ff fe fd fc fb fa f9 f8 f7 f6");
        output.adaptiveTriggers((byte)0x0C, (byte)0x21, (byte)0x26, left, right);
        byte[] report = output.adaptiveTriggers((byte)0xFB, (byte)0x05, (byte)0, new byte[10], null);
        assertEquals(0x0B, report[1]);
        assertEquals(0x26, report[11]);
        assertEquals(0x05, report[22]);
        assertArrayEquals(right, Arrays.copyOfRange(report, 12, 22));
        assertEquals(3, output.led((byte)1, (byte)2, (byte)3)[1]);
        report = output.adaptiveTriggers((byte)0x04, (byte)0, (byte)0x01, null, right);
        assertEquals(0x07, report[1]);
        assertEquals(0x05, report[22]);
    }

    @Test
    public void malformedEffectsAndUnselectedTriggersAreIgnoredWithoutChangingState() {
        DualSenseReport.Output output = new DualSenseReport.Output(false);
        assertNull(output.adaptiveTriggers((byte)0, (byte)1, (byte)1, null, null));
        assertNull(output.adaptiveTriggers((byte)0x0C, (byte)1, (byte)2, new byte[10], new byte[9]));
        assertNull(output.adaptiveTriggers((byte)0x08, (byte)1, (byte)2, new byte[11], null));
        assertNull(output.adaptiveTriggers((byte)0x08, (byte)1, (byte)2, null, null));
        assertEquals(0, output.rumble((short)0, (short)0)[22]);
    }

    @Test
    public void resetClearsRumbleLightsAndBothAdaptiveTriggers() {
        DualSenseReport.Output output = new DualSenseReport.Output(true);
        output.rumble((short)-1, (short)-1);
        output.led((byte)-1, (byte)-1, (byte)-1);
        output.player(3);
        output.adaptiveTriggers((byte)0x0C, (byte)0x21, (byte)0x26, new byte[10], new byte[10]);
        assertArrayEquals(hex("02 0e 14 00 00 00 00 00 00 00 00 05 00 00 00 00 " +
                "00 00 00 00 00 00 05 00 00 00 00 00 00 00 00 00 " +
                "00 00 00 00 00 00 00 04 00 00 00 00 00 00 00 00"), output.reset());
    }

    private static class TouchListener implements UsbDriverListener {
        final List<Integer> events = new ArrayList<>();

        @Override
        public void reportControllerTouch(int id, byte type, int pointerId, float x, float y, float pressure) {
            assertEquals(42, id);
            assertEquals(type == MoonBridge.LI_TOUCH_EVENT_UP ? 0 : 1, pressure, 0);
            events.add((type << 8) | pointerId);
        }

        @Override
        public void reportControllerState(int id, int buttons, float lx, float ly, float rx, float ry, float lt, float rt) {}
        @Override
        public void reportControllerMotion(int id, byte type, float x, float y, float z) {}
        @Override
        public void reportControllerBattery(int id, byte state, byte percent) {}
        @Override
        public void deviceRemoved(AbstractController controller) {}
        @Override
        public void deviceAdded(AbstractController controller) {}
    }
}
