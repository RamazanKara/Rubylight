package com.limelight.binding.input.driver;

import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;

import static org.junit.Assert.*;

// Byte layouts follow dekuNukem/Nintendo_Switch_Reverse_Engineering. Where a section has no
// complete sample, the report is built by following its documented layout, as noted per test.
public class SwitchProReportTest {
    private static final float G = 9.80665f;

    // spi_flash_notes.md, "Analog stick factory and user calibration": the left and right sample
    // values (X/Y above 0x4F7/0x424, centre 0x79F/0x8A0, below 0x510/0x479), packed as documented.
    private static final byte[] FACTORY_STICKS = bytes(
            "F7 44 42 9F 07 8A 10 95 47" + " 9F 07 8A 10 95 47 F7 44 42");

    // spi_flash_notes.md, "x8000 User Calibration" dump, 0x8010-0x8025: no left user calibration,
    // magic B2 A1 and the right stick's user calibration.
    private static final byte[] USER_STICKS = bytes(
            "FF FF FF FF FF FF FF FF FF FF FF B2 A1 C6 F8 71 68 84 42 A3 54 42");

    // spi_flash_notes.md, "6-Axis sensor factory and user calibration" sample, little-endian:
    // accel origin FFB0 FEB9 00E0, accel coefficient 4000, gyro origin 000E FFDF FFD0, gyro 343B.
    private static final byte[] FACTORY_IMU = bytes(
            "B0 FF B9 FE E0 00 00 40 00 40 00 40 0E 00 DF FF D0 FF 3B 34 3B 34 3B 34");

    // spi_flash_notes.md, "x8000 User Calibration" dump, 0x8026-0x8039.
    private static final byte[] USER_IMU = bytes(
            "B2 A1 76 00 A6 FE EA 02 00 40 00 40 00 40 0E 00 FC FF E0 FF");

    private static byte[] bytes(String hex) {
        String[] parts = hex.trim().split("\\s+");
        byte[] out = new byte[parts.length];
        for (int i = 0; i < parts.length; i++) out[i] = (byte) Integer.parseInt(parts[i], 16);
        return out;
    }

    private static void putStick(byte[] report, int offset, int x, int y) {
        report[offset] = (byte) (x & 0xFF);
        report[offset + 1] = (byte) (((x >> 8) & 0x0F) | ((y & 0x0F) << 4));
        report[offset + 2] = (byte) (y >> 4);
    }

    private static void putShort(byte[] report, int offset, int value) {
        report[offset] = (byte) value;
        report[offset + 1] = (byte) (value >> 8);
    }

    // bluetooth_hid_notes.md, "Standard input report format": built from the table, sticks centred.
    private static byte[] fullReport(int right, int shared, int left) {
        byte[] report = new byte[SwitchProReport.REPORT_SIZE];
        report[0] = 0x30;
        report[2] = (byte) 0x81;
        report[3] = (byte) right;
        report[4] = (byte) shared;
        report[5] = (byte) left;
        putStick(report, 6, 0x79F, 0x8A0);
        putStick(report, 9, 0x79F, 0x8A0);
        report[12] = 0x70;
        return report;
    }

    private static SwitchProReport.StickCalibration factorySticks() {
        SwitchProReport.StickCalibration sticks = new SwitchProReport.StickCalibration();
        assertTrue(sticks.load(null, FACTORY_STICKS));
        return sticks;
    }

    @Test
    public void stickCalibrationUnpacksBothDocumentedFieldOrders() {
        SwitchProReport.StickCalibration sticks = new SwitchProReport.StickCalibration();
        sticks.reset();
        sticks.parseLeft(FACTORY_STICKS, 0);
        sticks.parseRight(FACTORY_STICKS, 9);
        for (int x : new int[] {SwitchProReport.StickCalibration.LEFT_X, SwitchProReport.StickCalibration.RIGHT_X}) {
            assertEquals(0x79F, sticks.center[x]);
            assertEquals(0x4F7, sticks.above[x]);
            assertEquals(0x510, sticks.below[x]);
        }
        for (int y : new int[] {SwitchProReport.StickCalibration.LEFT_Y, SwitchProReport.StickCalibration.RIGHT_Y}) {
            assertEquals(0x8A0, sticks.center[y]);
            assertEquals(0x424, sticks.above[y]);
            assertEquals(0x479, sticks.below[y]);
        }
    }

    @Test
    public void userStickCalibrationWinsOnlyWhereItsMagicIsSet() {
        SwitchProReport.StickCalibration sticks = new SwitchProReport.StickCalibration();
        assertFalse(SwitchProReport.StickCalibration.hasUserLeft(USER_STICKS));
        assertTrue(SwitchProReport.StickCalibration.hasUserRight(USER_STICKS));
        assertTrue(sticks.load(USER_STICKS, FACTORY_STICKS));
        assertEquals(0x79F, sticks.center[SwitchProReport.StickCalibration.LEFT_X]);
        assertEquals(0x8C6, sticks.center[SwitchProReport.StickCalibration.RIGHT_X]);
        assertEquals(0x71F, sticks.center[SwitchProReport.StickCalibration.RIGHT_Y]);
        assertEquals((int) (0x468 * 0.7f), sticks.below[SwitchProReport.StickCalibration.RIGHT_X]);
        assertEquals((int) (0x4A3 * 0.7f), sticks.above[SwitchProReport.StickCalibration.RIGHT_X]);
        // Without the factory block the missing left stick cannot be calibrated.
        assertFalse(new SwitchProReport.StickCalibration().load(USER_STICKS, null));
    }

    @Test
    public void unwrittenFlashFallsBackToTheMidpoint() {
        SwitchProReport.StickCalibration sticks = new SwitchProReport.StickCalibration();
        byte[] blank = bytes("FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF");
        assertTrue(sticks.load(null, blank));
        assertEquals(2048, sticks.center[0]);
        assertEquals((int) ((int) (2048 * 0.7f) * 0.7f), sticks.above[0]);
        assertEquals(0, sticks.apply(0, 2048), 0);
    }

    @Test
    public void sticksReachFullTravelAndGrowBeyondTheirCalibration() {
        SwitchProReport.StickCalibration sticks = factorySticks();
        int x = SwitchProReport.StickCalibration.LEFT_X;
        int reach = (int) (0x4F7 * 0.7f);
        assertEquals(0, sticks.apply(x, 0x79F), 0);
        assertEquals(1, sticks.apply(x, 0x79F + reach), 1e-6);
        assertEquals(0.5f, sticks.apply(x, 0x79F + reach / 2), 0.01f);
        assertEquals(1, sticks.apply(x, 0x79F + reach + 100), 1e-6);
        assertEquals(1, sticks.apply(x, 0x79F + reach + 100), 1e-6);
        assertTrue(sticks.apply(x, 0x79F + reach) < 1);
        assertEquals(-1, sticks.apply(x, 0x79F - (int) (0x510 * 0.7f)), 1e-6);
    }

    @Test
    public void eachDocumentedButtonBitMapsToItsPosition() {
        // bluetooth_hid_notes.md, "Standard input report - buttons" (bytes 3, 4, 5).
        int[][] cases = {
                {0, 0x01, ControllerPacket.X_FLAG}, // Y, left face button
                {0, 0x02, ControllerPacket.Y_FLAG}, // X, top
                {0, 0x04, ControllerPacket.A_FLAG}, // B, bottom
                {0, 0x08, ControllerPacket.B_FLAG}, // A, right
                {0, 0x40, ControllerPacket.RB_FLAG},
                {1, 0x01, ControllerPacket.BACK_FLAG}, // Minus
                {1, 0x02, ControllerPacket.PLAY_FLAG}, // Plus
                {1, 0x04, ControllerPacket.RS_CLK_FLAG},
                {1, 0x08, ControllerPacket.LS_CLK_FLAG},
                {1, 0x10, ControllerPacket.SPECIAL_BUTTON_FLAG}, // Home
                {1, 0x20, ControllerPacket.MISC_FLAG}, // Capture
                {2, 0x01, ControllerPacket.DOWN_FLAG},
                {2, 0x02, ControllerPacket.UP_FLAG},
                {2, 0x04, ControllerPacket.RIGHT_FLAG},
                {2, 0x08, ControllerPacket.LEFT_FLAG},
                {2, 0x40, ControllerPacket.LB_FLAG},
        };
        SwitchProReport.StickCalibration sticks = factorySticks();
        SwitchProReport.ImuCalibration imu = new SwitchProReport.ImuCalibration();
        SwitchProReport.Input input = new SwitchProReport.Input();
        int all = 0;
        for (int[] c : cases) {
            int[] bytes = new int[3];
            bytes[c[0]] = c[1];
            assertTrue(input.parse(fullReport(bytes[0], bytes[1], bytes[2]), 64, sticks, imu));
            assertEquals("byte " + (3 + c[0]) + " bit " + c[1], c[2], input.buttons);
            assertEquals(0, input.leftTrigger, 0);
            assertEquals(0, input.rightTrigger, 0);
            all |= c[2];
        }
        assertEquals(SwitchProReport.BUTTONS, all);

        assertTrue(input.parse(fullReport(0x80, 0, 0x80), 64, sticks, imu));
        assertEquals(0, input.buttons);
        assertEquals(1, input.leftTrigger, 0);
        assertEquals(1, input.rightTrigger, 0);
        // SL/SR and the Charging Grip bit belong to Joy-Con and are ignored.
        assertTrue(input.parse(fullReport(0x30, 0x80, 0x30), 64, sticks, imu));
        assertEquals(0, input.buttons);
    }

    @Test
    public void sticksAreCalibratedAndReportedWithDownPositive() {
        SwitchProReport.StickCalibration sticks = factorySticks();
        SwitchProReport.ImuCalibration imu = new SwitchProReport.ImuCalibration();
        SwitchProReport.Input input = new SwitchProReport.Input();
        byte[] report = fullReport(0, 0, 0);
        assertTrue(input.parse(report, 64, sticks, imu));
        assertEquals(0, input.leftStickX, 0);
        assertEquals(0, input.leftStickY, 0);
        assertEquals(0, input.rightStickX, 0);
        assertEquals(0, input.rightStickY, 0);

        // Left stick pushed fully up, right stick fully left.
        putStick(report, 6, 0x79F, 0x8A0 + (int) (0x424 * 0.7f));
        putStick(report, 9, 0x79F - (int) (0x510 * 0.7f), 0x8A0);
        assertTrue(input.parse(report, 64, sticks, imu));
        assertEquals(-1, input.leftStickY, 1e-6);
        assertEquals(0, input.leftStickX, 0);
        assertEquals(-1, input.rightStickX, 1e-6);
    }

    @Test
    public void onlyFullReportsAreParsed() {
        SwitchProReport.Input input = new SwitchProReport.Input();
        SwitchProReport.StickCalibration sticks = factorySticks();
        SwitchProReport.ImuCalibration imu = new SwitchProReport.ImuCalibration();
        byte[] report = fullReport(0x04, 0, 0);
        assertFalse(input.parse(report, 48, sticks, imu));
        report[0] = 0x21;
        assertFalse(input.parse(report, 64, sticks, imu));
        report[0] = 0x3F;
        assertFalse(input.parse(report, 64, sticks, imu));
    }

    @Test
    public void batteryNibbleGivesLevelAndCharging() {
        // bluetooth_hid_notes.md: byte 2 high nibble, 8 = full ... 0 = empty, LSB = charging.
        SwitchProReport.Input input = new SwitchProReport.Input();
        assertEquals(MoonBridge.LI_BATTERY_STATE_UNKNOWN, input.batteryState());
        assertEquals(MoonBridge.LI_BATTERY_PERCENTAGE_UNKNOWN, input.batteryPercentage());
        int[][] cases = {
                {0x81, MoonBridge.LI_BATTERY_STATE_DISCHARGING, 100},
                {0x61, MoonBridge.LI_BATTERY_STATE_DISCHARGING, 75},
                {0x51, MoonBridge.LI_BATTERY_STATE_CHARGING, 50},
                {0x91, MoonBridge.LI_BATTERY_STATE_FULL, 100},
                {0x21, MoonBridge.LI_BATTERY_STATE_DISCHARGING, 25},
        };
        for (int[] c : cases) {
            byte[] report = fullReport(0, 0, 0);
            report[2] = (byte) c[0];
            assertTrue(input.parse(report, 64, factorySticks(), new SwitchProReport.ImuCalibration()));
            assertEquals(Integer.toHexString(c[0]), c[1], input.batteryState());
            assertEquals(Integer.toHexString(c[0]), c[2], input.batteryPercentage());
        }
    }

    @Test
    public void factoryImuCalibrationScalesToSiUnits() {
        SwitchProReport.ImuCalibration imu = new SwitchProReport.ImuCalibration();
        assertTrue(imu.parse(FACTORY_IMU, 0, null, 0));
        // imu_sensor_notes.md: acc_coeff = 4 / (cal_acc_coeff - origin) G per count,
        // gyro = (raw - offset) * 936 / (cal_gyro_coeff - offset) degrees per second.
        assertEquals(4.0f / (0x4000 + 0x50) * G, imu.accelScale[0], 1e-9);
        assertEquals(4.0f / (0x4000 + 0x147) * G, imu.accelScale[1], 1e-9);
        assertEquals(4.0f / (0x4000 - 0xE0) * G, imu.accelScale[2], 1e-9);
        assertEquals(936.0f / (0x343B - 0x0E), imu.gyroScale[0], 1e-9);
        assertEquals(936.0f / (0x343B + 0x21), imu.gyroScale[1], 1e-9);
        assertEquals(936.0f / (0x343B + 0x30), imu.gyroScale[2], 1e-9);
        assertArrayEquals(new int[] {0x0E, -0x21, -0x30}, imu.gyroOffset);
    }

    @Test
    public void userImuCalibrationReplacesOriginsButNotCoefficients() {
        SwitchProReport.ImuCalibration imu = new SwitchProReport.ImuCalibration();
        assertTrue(imu.parse(FACTORY_IMU, 0, USER_IMU, 0));
        assertEquals(4.0f / (0x4000 - 0x76) * G, imu.accelScale[0], 1e-9);
        assertEquals(4.0f / (0x4000 + 0x15A) * G, imu.accelScale[1], 1e-9);
        assertEquals(4.0f / (0x4000 - 0x2EA) * G, imu.accelScale[2], 1e-9);
        assertArrayEquals(new int[] {0x0E, -4, -0x20}, imu.gyroOffset);
        assertEquals(936.0f / (0x343B + 4), imu.gyroScale[1], 1e-9);

        byte[] noMagic = USER_IMU.clone();
        noMagic[0] = (byte) 0xFF;
        SwitchProReport.ImuCalibration factory = new SwitchProReport.ImuCalibration();
        assertTrue(factory.parse(FACTORY_IMU, 0, noMagic, 0));
        assertArrayEquals(new int[] {0x0E, -0x21, -0x30}, factory.gyroOffset);
    }

    @Test
    public void implausibleImuCalibrationKeepsNominalScales() {
        SwitchProReport.ImuCalibration imu = new SwitchProReport.ImuCalibration();
        byte[] blank = new byte[24];
        assertFalse(imu.parse(blank, 0, null, 0));
        assertEquals(G / 4096, imu.accelScale[0], 1e-9);
        assertEquals(936.0f / 13371, imu.gyroScale[0], 1e-9);
        assertArrayEquals(new int[3], imu.gyroOffset);
    }

    @Test
    public void motionUsesTheNewestSampleInPlayStationAxes() {
        // imu_sensor_notes.md, "Packet data information": accel XYZ at 13-18, gyro at 19-24, then
        // two older samples. Built from that layout with nominal scales.
        SwitchProReport.ImuCalibration imu = new SwitchProReport.ImuCalibration();
        SwitchProReport.Input input = new SwitchProReport.Input();
        byte[] report = fullReport(0, 0, 0);
        assertTrue(input.parse(report, 64, factorySticks(), imu));
        assertFalse("All-zero accel means the IMU is off", input.hasMotion);

        putShort(report, 13, 100);
        putShort(report, 15, 200);
        putShort(report, 17, 4096);
        putShort(report, 19, 10);
        putShort(report, 21, -20);
        putShort(report, 23, 30);
        putShort(report, 25, 9999);
        assertTrue(input.parse(report, 64, factorySticks(), imu));
        assertTrue(input.hasMotion);
        float a = G / 4096, g = 936.0f / 13371;
        assertEquals(-200 * a, input.accel[0], 1e-5);
        assertEquals(G, input.accel[1], 1e-5);
        assertEquals(-100 * a, input.accel[2], 1e-5);
        assertEquals(20 * g, input.gyro[0], 1e-5);
        assertEquals(30 * g, input.gyro[1], 1e-5);
        assertEquals(-10 * g, input.gyro[2], 1e-5);

        imu.parse(FACTORY_IMU, 0, null, 0);
        putShort(report, 19, 0x0E);
        putShort(report, 21, -0x21);
        putShort(report, 23, -0x30);
        assertTrue(input.parse(report, 64, factorySticks(), imu));
        assertArrayEquals("A still pad reads zero after its gyro offsets", new float[3], input.gyro, 0);
    }

    @Test
    public void repliesAreMatchedToTheirRequest() {
        // bluetooth_hid_subcommands_notes.md, "Subcommand 0x10: SPI flash read" response sample.
        byte[] reply = new byte[64];
        reply[0] = 0x21;
        reply[13] = (byte) 0x90;
        reply[14] = 0x10;
        reply[15] = (byte) 0x80;
        reply[16] = 0x60;
        reply[19] = 0x18;
        assertEquals(20, SwitchProReport.spiReadData(reply, 64, 0x6080, 0x18));
        assertEquals(-1, SwitchProReport.spiReadData(reply, 64, 0x6020, 0x18));
        assertEquals(-1, SwitchProReport.spiReadData(reply, 64, 0x6080, 0x10));
        assertEquals(-1, SwitchProReport.spiReadData(reply, 30, 0x6080, 0x18));
        reply[13] = 0x00; // NACK
        assertEquals(-1, SwitchProReport.spiReadData(reply, 64, 0x6080, 0x18));

        byte[] ack = new byte[64];
        ack[0] = (byte) 0x81;
        ack[1] = 0x02;
        assertTrue(SwitchProReport.isCommandAck(ack, 64, 0x02));
        assertFalse(SwitchProReport.isCommandAck(ack, 64, 0x03));

        byte[] subcommand = new byte[64];
        subcommand[0] = 0x21;
        subcommand[13] = (byte) 0x80;
        subcommand[14] = 0x48;
        assertTrue(SwitchProReport.isSubcommandReply(subcommand, 64, 0x48));
        assertFalse(SwitchProReport.isSubcommandReply(subcommand, 64, 0x40));
    }

    @Test
    public void outputReportsFollowTheDocumentedLayouts() {
        SwitchProReport.Output output = new SwitchProReport.Output();
        // USB-HID-Notes.md: proprietary commands start with 80.
        byte[] report = output.proprietary(0x02);
        assertEquals(64, report.length);
        assertEquals((byte) 0x80, report[0]);
        assertEquals(0x02, report[1]);

        // bluetooth_hid_subcommands_notes.md request sample: [01 .. 10 80 60 00 00 18].
        report = output.spiRead(0x6080, 0x18);
        assertEquals(0x01, report[0]);
        assertEquals(0, report[1]);
        // bluetooth_hid_notes.md: neutral rumble 00 01 40 40 for each side.
        assertArrayEquals(bytes("00 01 40 40 00 01 40 40"), java.util.Arrays.copyOfRange(report, 2, 10));
        assertArrayEquals(bytes("10 80 60 00 00 18"), java.util.Arrays.copyOfRange(report, 10, 16));

        report = output.player(0);
        assertEquals(1, report[1]);
        assertEquals(0x30, report[10]);
        assertEquals(0x1, report[11]);
        assertEquals(0x9, output.player(4)[11]);
        assertEquals(0x1, output.player(8)[11]);
        assertEquals(0x0, output.player(-1)[11]);

        report = output.rumble((short) 0xFFFF, (short) 0xFFFF);
        assertEquals(0x10, report[0]);
        assertEquals(5, report[1]);
        for (int i = 6; i < 16; i++) output.rumbleReport();
        assertEquals("Packet numbers loop in 0x0-0xF", 0, output.rumbleReport()[1]);
    }

    @Test
    public void amplitudeStepsMatchTheDocumentedTable() {
        // rumble_data_table.md, "Amplitude Table": HA byte 2*i, amplitude scaled to 16 bits.
        assertEquals(0, SwitchProReport.Output.amplitudeStep(0));
        assertEquals(1, SwitchProReport.Output.amplitudeStep(1));
        assertEquals(1, SwitchProReport.Output.amplitudeStep(Math.round(0.007843f * 65535)));
        assertEquals(2, SwitchProReport.Output.amplitudeStep(Math.round(0.007843f * 65535) + 1));
        assertEquals(0x40 / 2, SwitchProReport.Output.amplitudeStep(Math.round(0.229908f * 65535)));
        assertEquals(0xC0 / 2, SwitchProReport.Output.amplitudeStep(Math.round(0.919633f * 65535)));
        assertEquals(0xC8 / 2, SwitchProReport.Output.amplitudeStep(65535));
    }

    @Test
    public void rumbleEncodesBothBandsAndSilence() {
        byte[] motor = new byte[4];
        SwitchProReport.Output.encode(motor, 0, 0, 0);
        assertArrayEquals(bytes("00 01 40 40"), motor);
        // Full strength: HA C8 and LA 00 72 (table's last safe row), at 0x0074 / 0x3D (150 Hz).
        SwitchProReport.Output.encode(motor, 0, 0xFFFF, 0xFFFF);
        assertArrayEquals(bytes("74 C8 3D 72"), motor);
        // Step 1: HA 02 and LA 80 40; the LA high bit rides on the LF byte.
        SwitchProReport.Output.encode(motor, 0, 514, 514);
        assertArrayEquals(bytes("74 02 BD 40"), motor);
        // Only the strong (low-frequency) motor: high band silent, low band at full.
        SwitchProReport.Output.encode(motor, 0, 0xFFFF, 0);
        assertArrayEquals(bytes("74 00 3D 72"), motor);
        SwitchProReport.Output.encode(motor, 0, 0, 0xFFFF);
        assertArrayEquals(bytes("74 C8 3D 40"), motor);
    }

    @Test
    public void rumbleStateIsRepeatedUntilStopped() {
        SwitchProReport.Output output = new SwitchProReport.Output();
        assertFalse(output.rumbleActive());
        byte[] report = output.rumble((short) 0xFFFF, (short) 0);
        assertTrue(output.rumbleActive());
        assertArrayEquals(bytes("74 00 3D 72 74 00 3D 72"), java.util.Arrays.copyOfRange(report, 2, 10));
        report = output.rumbleReport();
        assertArrayEquals(bytes("74 00 3D 72 74 00 3D 72"), java.util.Arrays.copyOfRange(report, 2, 10));
        // Subcommands carry the current rumble too.
        report = output.subcommand(SwitchProReport.SUBCOMMAND_IMU, 1, 1);
        assertArrayEquals(bytes("74 00 3D 72 74 00 3D 72"), java.util.Arrays.copyOfRange(report, 2, 10));
        report = output.stopRumble();
        assertFalse(output.rumbleActive());
        assertArrayEquals(bytes("00 01 40 40 00 01 40 40"), java.util.Arrays.copyOfRange(report, 2, 10));
        output.rumble((short) 0, (short) 0);
        assertFalse(output.rumbleActive());
    }

    @Test
    public void onlyTheProControllerIsClaimed() {
        assertTrue(SwitchProReport.isProController(0x057E, 0x2009));
        // Joy-Con L/R and the Charging Grip stay on Android's input path.
        assertFalse(SwitchProReport.isProController(0x057E, 0x2006));
        assertFalse(SwitchProReport.isProController(0x057E, 0x2007));
        assertFalse(SwitchProReport.isProController(0x057E, 0x200E));
        assertFalse(SwitchProReport.isProController(0x054C, 0x2009));
    }
}
