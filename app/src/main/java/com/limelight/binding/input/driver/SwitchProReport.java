/*
  Report layouts, calibration and rumble encoding follow the public protocol notes in
  dekuNukem/Nintendo_Switch_Reverse_Engineering (USB-HID-Notes.md, bluetooth_hid_notes.md,
  bluetooth_hid_subcommands_notes.md, spi_flash_notes.md, imu_sensor_notes.md, rumble_data_table.md).
  The USB start sequence, stick extents, motion axes and rumble frequencies are adapted from
  SDL_hidapi_switch.c (altered Java implementation).
  Simple DirectMedia Layer
  Copyright (C) 1997-2026 Sam Lantinga <slouken@libsdl.org>

  This software is provided 'as-is', without any express or implied
  warranty. In no event will the authors be held liable for any damages
  arising from the use of this software.

  Permission is granted to anyone to use this software for any purpose,
  including commercial applications, and to alter it and redistribute it
  freely, subject to the following restrictions:

  1. The origin of this software must not be misrepresented; you must not
     claim that you wrote the original software. If you use this software
     in a product, an acknowledgment in the product documentation would be
     appreciated but is not required.
  2. Altered source versions must be plainly marked as such, and must not be
     misrepresented as being the original software.
  3. This notice may not be removed or altered from any source distribution.
*/
package com.limelight.binding.input.driver;

import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;

import java.util.Arrays;

// Switch Pro Controller over USB. Joy-Con (0x2006/0x2007) and the Charging Grip (0x200E) are not
// claimed: they report per half and need pairing logic this driver does not have.
final class SwitchProReport {
    static final int VENDOR_ID = 0x057E;
    static final int PRO_CONTROLLER = 0x2009;
    static final int REPORT_SIZE = 64;

    static final int FULL_STATE = 0x30;
    static final int SUBCOMMAND_REPLY = 0x21;
    static final int COMMAND_ACK = 0x81;

    static final int PROPRIETARY_HANDSHAKE = 0x02;
    static final int PROPRIETARY_HIGH_SPEED = 0x03;
    static final int PROPRIETARY_FORCE_USB = 0x04;

    static final int SUBCOMMAND_INPUT_MODE = 0x03;
    static final int SUBCOMMAND_SPI_READ = 0x10;
    static final int SUBCOMMAND_PLAYER_LIGHTS = 0x30;
    static final int SUBCOMMAND_IMU = 0x40;
    static final int SUBCOMMAND_VIBRATION = 0x48;

    static final int SPI_STICK_FACTORY = 0x603D, SPI_STICK_FACTORY_SIZE = 18;
    static final int SPI_STICK_USER = 0x8010, SPI_STICK_USER_SIZE = 22;
    static final int SPI_IMU_FACTORY = 0x6020, SPI_IMU_FACTORY_SIZE = 24;
    static final int SPI_IMU_USER = 0x8026, SPI_IMU_USER_SIZE = 20;

    static final int BUTTONS = ControllerPacket.A_FLAG | ControllerPacket.B_FLAG |
            ControllerPacket.X_FLAG | ControllerPacket.Y_FLAG | ControllerPacket.UP_FLAG |
            ControllerPacket.DOWN_FLAG | ControllerPacket.LEFT_FLAG | ControllerPacket.RIGHT_FLAG |
            ControllerPacket.LB_FLAG | ControllerPacket.RB_FLAG | ControllerPacket.LS_CLK_FLAG |
            ControllerPacket.RS_CLK_FLAG | ControllerPacket.PLAY_FLAG | ControllerPacket.BACK_FLAG |
            ControllerPacket.SPECIAL_BUTTON_FLAG | ControllerPacket.MISC_FLAG;

    private static final float GRAVITY = 9.80665f;

    private SwitchProReport() {}

    static boolean isProController(int vendorId, int productId) {
        return vendorId == VENDOR_ID && productId == PRO_CONTROLLER;
    }

    private static int signedShort(byte[] data, int offset) {
        return (short)((data[offset] & 0xFF) | ((data[offset + 1] & 0xFF) << 8));
    }

    static boolean isCommandAck(byte[] report, int length, int command) {
        return length >= 2 && (report[0] & 0xFF) == COMMAND_ACK && (report[1] & 0xFF) == command;
    }

    // Subcommand replies: byte 13 is the ACK (MSB set), byte 14 the subcommand, data from byte 15.
    static boolean isSubcommandReply(byte[] report, int length, int subcommand) {
        return length >= 15 && (report[0] & 0xFF) == SUBCOMMAND_REPLY &&
                (report[13] & 0x80) != 0 && (report[14] & 0xFF) == subcommand;
    }

    // An SPI read reply repeats the address (uint32 LE) and size, then carries the data from byte 20.
    static int spiReadData(byte[] report, int length, int address, int size) {
        if (!isSubcommandReply(report, length, SUBCOMMAND_SPI_READ) || length < 20 + size) {
            return -1;
        }
        int replyAddress = (report[15] & 0xFF) | ((report[16] & 0xFF) << 8) |
                ((report[17] & 0xFF) << 16) | ((report[18] & 0xFF) << 24);
        return replyAddress == address && (report[19] & 0xFF) == size ? 20 : -1;
    }

    static final class Input {
        int buttons;
        float leftStickX, leftStickY, rightStickX, rightStickY, leftTrigger, rightTrigger;
        int batteryByte = -1;
        boolean hasMotion;
        final float[] gyro = new float[3];
        final float[] accel = new float[3];

        // Standard full report 0x30: buttons at 3-5, sticks at 6-11, IMU samples from 13.
        boolean parse(byte[] data, int length, StickCalibration sticks, ImuCalibration imu) {
            if (length < 49 || length > data.length || (data[0] & 0xFF) != FULL_STATE) {
                return false;
            }
            batteryByte = data[2] & 0xF0;

            int right = data[3] & 0xFF, shared = data[4] & 0xFF, left = data[5] & 0xFF;
            // Positional face buttons, as Android's own Switch driver and SDL report them.
            int flags = 0;
            flags |= (right & 0x01) != 0 ? ControllerPacket.X_FLAG : 0;
            flags |= (right & 0x02) != 0 ? ControllerPacket.Y_FLAG : 0;
            flags |= (right & 0x04) != 0 ? ControllerPacket.A_FLAG : 0;
            flags |= (right & 0x08) != 0 ? ControllerPacket.B_FLAG : 0;
            flags |= (right & 0x40) != 0 ? ControllerPacket.RB_FLAG : 0;
            flags |= (shared & 0x01) != 0 ? ControllerPacket.BACK_FLAG : 0;
            flags |= (shared & 0x02) != 0 ? ControllerPacket.PLAY_FLAG : 0;
            flags |= (shared & 0x04) != 0 ? ControllerPacket.RS_CLK_FLAG : 0;
            flags |= (shared & 0x08) != 0 ? ControllerPacket.LS_CLK_FLAG : 0;
            flags |= (shared & 0x10) != 0 ? ControllerPacket.SPECIAL_BUTTON_FLAG : 0;
            flags |= (shared & 0x20) != 0 ? ControllerPacket.MISC_FLAG : 0;
            flags |= (left & 0x01) != 0 ? ControllerPacket.DOWN_FLAG : 0;
            flags |= (left & 0x02) != 0 ? ControllerPacket.UP_FLAG : 0;
            flags |= (left & 0x04) != 0 ? ControllerPacket.RIGHT_FLAG : 0;
            flags |= (left & 0x08) != 0 ? ControllerPacket.LEFT_FLAG : 0;
            flags |= (left & 0x40) != 0 ? ControllerPacket.LB_FLAG : 0;
            buttons = flags;
            rightTrigger = (right & 0x80) != 0 ? 1 : 0;
            leftTrigger = (left & 0x80) != 0 ? 1 : 0;

            // The pad reports up as larger values; the listener takes Android's down-positive Y.
            leftStickX = sticks.apply(StickCalibration.LEFT_X, stickX(data, 6));
            leftStickY = -sticks.apply(StickCalibration.LEFT_Y, stickY(data, 6));
            rightStickX = sticks.apply(StickCalibration.RIGHT_X, stickX(data, 9));
            rightStickY = -sticks.apply(StickCalibration.RIGHT_Y, stickY(data, 9));

            // Three samples 5 ms apart; the first one is the newest. All-zero accel means no IMU data.
            hasMotion = data[13] != 0 || data[14] != 0 || data[15] != 0 ||
                    data[16] != 0 || data[17] != 0 || data[18] != 0;
            if (hasMotion) {
                imu.apply(signedShort(data, 13), signedShort(data, 15), signedShort(data, 17),
                        signedShort(data, 19), signedShort(data, 21), signedShort(data, 23), accel, gyro);
            }
            return true;
        }

        static int stickX(byte[] data, int offset) {
            return (data[offset] & 0xFF) | ((data[offset + 1] & 0x0F) << 8);
        }

        static int stickY(byte[] data, int offset) {
            return ((data[offset + 1] & 0xFF) >> 4) | ((data[offset + 2] & 0xFF) << 4);
        }

        // High nibble of byte 2: level 0-8 in its upper three bits, charging in its lowest bit.
        byte batteryState() {
            if (batteryByte < 0) {
                return MoonBridge.LI_BATTERY_STATE_UNKNOWN;
            }
            int level = (batteryByte & 0xE0) >> 4;
            if ((batteryByte & 0x10) != 0) {
                return level == 8 ? MoonBridge.LI_BATTERY_STATE_FULL : MoonBridge.LI_BATTERY_STATE_CHARGING;
            }
            return MoonBridge.LI_BATTERY_STATE_DISCHARGING;
        }

        byte batteryPercentage() {
            if (batteryByte < 0) {
                return MoonBridge.LI_BATTERY_PERCENTAGE_UNKNOWN;
            }
            return (byte)Math.min(100, Math.round(((batteryByte & 0xE0) >> 4) / 8.0f * 100));
        }
    }

    static final class StickCalibration {
        static final int LEFT_X = 0, LEFT_Y = 1, RIGHT_X = 2, RIGHT_Y = 3;
        final int[] center = new int[4];
        // Distance from the centre to each end, growing if a stick reaches further.
        final int[] below = new int[4];
        final int[] above = new int[4];

        StickCalibration() {
            reset();
            finish();
        }

        void reset() {
            Arrays.fill(center, 0xFFF);
            Arrays.fill(below, 0xFFF);
            Arrays.fill(above, 0xFFF);
        }

        // User block from 0x8010: magic B2 A1, left stick, magic B2 A1, right stick.
        static boolean hasUserLeft(byte[] user) {
            return user != null && (user[0] & 0xFF) == 0xB2 && (user[1] & 0xFF) == 0xA1;
        }

        static boolean hasUserRight(byte[] user) {
            return user != null && (user[11] & 0xFF) == 0xB2 && (user[12] & 0xFF) == 0xA1;
        }

        // Each stick takes its user calibration if set, else the factory one (0x603D: left, right).
        boolean load(byte[] user, byte[] factory) {
            boolean left = hasUserLeft(user), right = hasUserRight(user);
            if ((!left || !right) && factory == null) {
                return false;
            }
            reset();
            if (left) parseLeft(user, 2); else parseLeft(factory, 0);
            if (right) parseRight(user, 13); else parseRight(factory, 9);
            finish();
            return true;
        }

        // Nine bytes pack six 12-bit values. Left: X/Y above centre, X/Y centre, X/Y below centre.
        void parseLeft(byte[] data, int offset) {
            above[LEFT_X] = packed(data, offset, 0);
            above[LEFT_Y] = packed(data, offset, 1);
            center[LEFT_X] = packed(data, offset, 2);
            center[LEFT_Y] = packed(data, offset, 3);
            below[LEFT_X] = packed(data, offset, 4);
            below[LEFT_Y] = packed(data, offset, 5);
        }

        // Right: X/Y centre, X/Y below centre, X/Y above centre.
        void parseRight(byte[] data, int offset) {
            center[RIGHT_X] = packed(data, offset, 0);
            center[RIGHT_Y] = packed(data, offset, 1);
            below[RIGHT_X] = packed(data, offset, 2);
            below[RIGHT_Y] = packed(data, offset, 3);
            above[RIGHT_X] = packed(data, offset, 4);
            above[RIGHT_Y] = packed(data, offset, 5);
        }

        static int packed(byte[] data, int offset, int index) {
            int b = offset + (index / 2) * 3;
            if ((index & 1) == 0) {
                return ((data[b + 1] & 0x0F) << 8) | (data[b] & 0xFF);
            }
            return ((data[b + 2] & 0xFF) << 4) | ((data[b + 1] & 0xFF) >> 4);
        }

        // Unwritten flash reads 0xFFF. Like SDL, use the 12-bit midpoint and 70 % of it, then take
        // 70 % of each range so a stick reaches full travel; ranges grow when a stick goes further.
        void finish() {
            for (int i = 0; i < 4; i++) {
                if (center[i] == 0xFFF) center[i] = 2048;
                if (below[i] == 0xFFF) below[i] = (int)(center[i] * 0.7f);
                if (above[i] == 0xFFF) above[i] = (int)(center[i] * 0.7f);
                below[i] = Math.max(1, (int)(below[i] * 0.7f));
                above[i] = Math.max(1, (int)(above[i] * 0.7f));
            }
        }

        float apply(int axis, int raw) {
            int value = raw - center[axis];
            if (value >= 0) {
                if (value > above[axis]) above[axis] = value;
                return value / (float)above[axis];
            }
            if (-value > below[axis]) below[axis] = -value;
            return value / (float)below[axis];
        }
    }

    static final class ImuCalibration {
        // Nominal ranges: ±8 G over 16 bits (4096 per G) and 936 / 13371 dps per count.
        static final float NOMINAL_ACCEL = GRAVITY / 4096;
        static final float NOMINAL_GYRO = 936.0f / 13371;
        final float[] accelScale = {NOMINAL_ACCEL, NOMINAL_ACCEL, NOMINAL_ACCEL};
        final float[] gyroScale = {NOMINAL_GYRO, NOMINAL_GYRO, NOMINAL_GYRO};
        final int[] gyroOffset = new int[3];

        // Factory block (24 bytes): accel origin XYZ, accel coefficient XYZ, gyro offset XYZ, gyro
        // coefficient XYZ, all Int16LE. A user block (20 bytes, magic B2 A1) replaces the accel
        // origin and gyro offset; the coefficients stay factory ones, as in SDL.
        boolean parse(byte[] factory, int factoryOffset, byte[] user, int userOffset) {
            int[] origin = new int[3], accelCoeff = new int[3], offset = new int[3], gyroCoeff = new int[3];
            for (int i = 0; i < 3; i++) {
                origin[i] = signedShort(factory, factoryOffset + i * 2);
                accelCoeff[i] = signedShort(factory, factoryOffset + 6 + i * 2);
                offset[i] = signedShort(factory, factoryOffset + 12 + i * 2);
                gyroCoeff[i] = signedShort(factory, factoryOffset + 18 + i * 2);
            }
            if (user != null && (user[userOffset] & 0xFF) == 0xB2 && (user[userOffset + 1] & 0xFF) == 0xA1) {
                for (int i = 0; i < 3; i++) {
                    origin[i] = signedShort(user, userOffset + 2 + i * 2);
                    offset[i] = signedShort(user, userOffset + 14 + i * 2);
                }
            }
            float[] newAccel = new float[3], newGyro = new float[3];
            for (int i = 0; i < 3; i++) {
                int accelRange = accelCoeff[i] - origin[i];
                int gyroRange = gyroCoeff[i] - offset[i];
                if (accelRange <= 0 || gyroRange <= 0) {
                    return false;
                }
                newAccel[i] = 4.0f / accelRange * GRAVITY;
                newGyro[i] = 936.0f / gyroRange;
                // Keep the nominal scales when the flash holds something far off them.
                if (Math.abs(1 - newAccel[i] / NOMINAL_ACCEL) > 0.5f || Math.abs(1 - newGyro[i] / NOMINAL_GYRO) > 0.5f) {
                    return false;
                }
            }
            System.arraycopy(newAccel, 0, accelScale, 0, 3);
            System.arraycopy(newGyro, 0, gyroScale, 0, 3);
            System.arraycopy(offset, 0, gyroOffset, 0, 3);
            return true;
        }

        // Accel in m/s² and gyro in °/s, reordered to the PlayStation axes the DualSense path sends.
        void apply(int ax, int ay, int az, int gx, int gy, int gz, float[] accel, float[] gyro) {
            accel[0] = -accelScale[1] * ay;
            accel[1] = accelScale[2] * az;
            accel[2] = -accelScale[0] * ax;
            gyro[0] = -gyroScale[1] * (gy - gyroOffset[1]);
            gyro[1] = gyroScale[2] * (gz - gyroOffset[2]);
            gyro[2] = -gyroScale[0] * (gx - gyroOffset[0]);
        }
    }

    static final class Output {
        // Documented amplitude steps 0x00-0xC8 (rumble_data_table.md, "Amplitude Table") as 16-bit
        // values: step i is the first whose amplitude reaches the requested one.
        private static final int[] AMPLITUDES = {
                0, 514, 775, 921, 1096, 1303, 1550, 1843, 2192, 2606, 3100, 3686, 4383, 5213, 6199, 7372,
                7698, 8039, 8395, 8767, 9155, 9560, 9984, 10426, 10887, 11369, 11873, 12398, 12947, 13520,
                14119, 14744, 15067, 15397, 15734, 16079, 16431, 16790, 17158, 17534, 17918, 18310, 18711,
                19121, 19540, 19967, 20405, 20851, 21308, 21775, 22251, 22739, 23236, 23745, 24265, 24797,
                25340, 25894, 26462, 27041, 27633, 28238, 28856, 29488, 30134, 30794, 31468, 32157, 32861,
                33581, 34316, 35068, 35836, 36620, 37422, 38242, 39079, 39935, 40809, 41703, 42616, 43549,
                44503, 45477, 46473, 47491, 48531, 49593, 50679, 51789, 52923, 54082, 55266, 56476, 57713,
                58977, 60268, 61588, 62936, 64315, 65535};
        // Both bands play at about 150 Hz: high band 0x0074 and low band 0x3D in that table.
        private static final int HIGH_FREQUENCY = 0x0074;
        private static final int LOW_FREQUENCY = 0x3D;
        private static final int[] PLAYER_LIGHTS = {0x1, 0x3, 0x7, 0xF, 0x9, 0x5, 0xD, 0x6};

        final byte[] report = new byte[REPORT_SIZE];
        private final byte[] rumble = new byte[8];
        private int packetNumber;
        private boolean rumbleActive;

        Output() {
            neutral();
        }

        static int amplitudeStep(int amplitude) {
            for (int i = 0; i < AMPLITUDES.length; i++) {
                if (amplitude <= AMPLITUDES[i]) {
                    return i;
                }
            }
            return AMPLITUDES.length - 1;
        }

        // Each motor gets the same 4 bytes: HF low byte, HF amplitude with the HF high bit,
        // LF with the LF amplitude's high bit, LF amplitude. Silence is 00 01 40 40.
        static void encode(byte[] target, int offset, int lowMotor, int highMotor) {
            if (lowMotor == 0 && highMotor == 0) {
                target[offset] = 0x00;
                target[offset + 1] = 0x01;
                target[offset + 2] = 0x40;
                target[offset + 3] = 0x40;
                return;
            }
            int highStep = amplitudeStep(highMotor);
            int lowStep = amplitudeStep(lowMotor);
            int highAmplitude = highStep * 2;
            int lowAmplitude = ((lowStep & 1) != 0 ? 0x8000 : 0) | (0x40 + lowStep / 2);
            target[offset] = (byte)(HIGH_FREQUENCY & 0xFF);
            target[offset + 1] = (byte)(highAmplitude | ((HIGH_FREQUENCY >> 8) & 0x01));
            target[offset + 2] = (byte)(LOW_FREQUENCY | ((lowAmplitude >> 8) & 0x80));
            target[offset + 3] = (byte)(lowAmplitude & 0xFF);
        }

        boolean rumbleActive() {
            return rumbleActive;
        }

        private void neutral() {
            encode(rumble, 0, 0, 0);
            encode(rumble, 4, 0, 0);
            rumbleActive = false;
        }

        private int nextPacket() {
            int number = packetNumber;
            packetNumber = (packetNumber + 1) & 0xF;
            return number;
        }

        byte[] proprietary(int command) {
            Arrays.fill(report, (byte)0);
            report[0] = (byte)0x80;
            report[1] = (byte)command;
            return report;
        }

        // Output 0x01: packet number, the current rumble, the subcommand and its data.
        byte[] subcommand(int subcommand, int length, int... data) {
            Arrays.fill(report, (byte)0);
            report[0] = 0x01;
            report[1] = (byte)nextPacket();
            System.arraycopy(rumble, 0, report, 2, 8);
            report[10] = (byte)subcommand;
            for (int i = 0; i < length; i++) {
                report[11 + i] = (byte)data[i];
            }
            return report;
        }

        byte[] spiRead(int address, int size) {
            return subcommand(SUBCOMMAND_SPI_READ, 5, address & 0xFF, (address >> 8) & 0xFF,
                    (address >> 16) & 0xFF, (address >>> 24) & 0xFF, size);
        }

        byte[] player(int playerNumber) {
            return subcommand(SUBCOMMAND_PLAYER_LIGHTS, 1,
                    playerNumber < 0 ? 0 : PLAYER_LIGHTS[playerNumber % PLAYER_LIGHTS.length]);
        }

        // The host's strong (low-frequency) motor drives the low band, the weak one the high band.
        byte[] rumble(short lowFrequency, short highFrequency) {
            int low = lowFrequency & 0xFFFF, high = highFrequency & 0xFFFF;
            encode(rumble, 0, low, high);
            encode(rumble, 4, low, high);
            rumbleActive = low != 0 || high != 0;
            return rumbleReport();
        }

        // Output 0x10 carries the current rumble alone. Like SDL, the driver repeats an active one.
        byte[] rumbleReport() {
            Arrays.fill(report, (byte)0);
            report[0] = 0x10;
            report[1] = (byte)nextPacket();
            System.arraycopy(rumble, 0, report, 2, 8);
            return report;
        }

        byte[] stopRumble() {
            neutral();
            return rumbleReport();
        }
    }
}
