/*
  Report layouts and calibration adapted from SDL_hidapi_ps5.c (altered Java implementation).
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

final class DualSenseReport {
    static final int INPUT_SIZE = 64;
    static final int EFFECT_SIZE = 10;
    static final int RIGHT_TRIGGER = 0x04;
    static final int LEFT_TRIGGER = 0x08;
    static final int BUTTONS = ControllerPacket.A_FLAG | ControllerPacket.B_FLAG |
            ControllerPacket.X_FLAG | ControllerPacket.Y_FLAG | ControllerPacket.UP_FLAG |
            ControllerPacket.DOWN_FLAG | ControllerPacket.LEFT_FLAG | ControllerPacket.RIGHT_FLAG |
            ControllerPacket.LB_FLAG | ControllerPacket.RB_FLAG | ControllerPacket.LS_CLK_FLAG |
            ControllerPacket.RS_CLK_FLAG | ControllerPacket.PLAY_FLAG | ControllerPacket.BACK_FLAG |
            ControllerPacket.SPECIAL_BUTTON_FLAG | ControllerPacket.TOUCHPAD_FLAG | ControllerPacket.MISC_FLAG;
    static final int EDGE_BUTTONS = ControllerPacket.PADDLE1_FLAG | ControllerPacket.PADDLE2_FLAG;

    private static final int[] HATS = {
            ControllerPacket.UP_FLAG,
            ControllerPacket.UP_FLAG | ControllerPacket.RIGHT_FLAG,
            ControllerPacket.RIGHT_FLAG,
            ControllerPacket.DOWN_FLAG | ControllerPacket.RIGHT_FLAG,
            ControllerPacket.DOWN_FLAG,
            ControllerPacket.DOWN_FLAG | ControllerPacket.LEFT_FLAG,
            ControllerPacket.LEFT_FLAG,
            ControllerPacket.UP_FLAG | ControllerPacket.LEFT_FLAG
    };

    private static int signedShort(byte[] data, int offset) {
        return (short)((data[offset] & 0xFF) | ((data[offset + 1] & 0xFF) << 8));
    }

    static final class Input {
        int buttons;
        float leftStickX, leftStickY, rightStickX, rightStickY, leftTrigger, rightTrigger;
        final float[] gyro = new float[3];
        final float[] accel = new float[3];
        final boolean[] touching = new boolean[2];
        final int[] touchId = new int[2];
        final float[] touchX = new float[2];
        final float[] touchY = new float[2];
        byte batteryState, batteryPercentage;
        boolean headphones;

        boolean parse(byte[] data, int length, boolean edge) {
            if (length != INPUT_SIZE || length > data.length || data[0] != 0x01) {
                return false;
            }

            headphones = (data[54] & 1) != 0;
            leftStickX = stick(data[1]);
            leftStickY = stick(data[2]);
            rightStickX = stick(data[3]);
            rightStickY = stick(data[4]);
            leftTrigger = (data[5] & 0xFF) / 255.0f;
            rightTrigger = (data[6] & 0xFF) / 255.0f;
            // Edge trigger stops can report only the digital trigger bit.
            if (leftTrigger == 0 && (data[9] & 0x04) != 0) {
                leftTrigger = 1;
            }
            if (rightTrigger == 0 && (data[9] & 0x08) != 0) {
                rightTrigger = 1;
            }

            int hat = data[8] & 0x0F;
            buttons = hat < HATS.length ? HATS[hat] : 0;
            buttons |= (data[8] & 0x10) != 0 ? ControllerPacket.X_FLAG : 0;
            buttons |= (data[8] & 0x20) != 0 ? ControllerPacket.A_FLAG : 0;
            buttons |= (data[8] & 0x40) != 0 ? ControllerPacket.B_FLAG : 0;
            buttons |= (data[8] & 0x80) != 0 ? ControllerPacket.Y_FLAG : 0;
            buttons |= (data[9] & 0x01) != 0 ? ControllerPacket.LB_FLAG : 0;
            buttons |= (data[9] & 0x02) != 0 ? ControllerPacket.RB_FLAG : 0;
            buttons |= (data[9] & 0x10) != 0 ? ControllerPacket.BACK_FLAG : 0;
            buttons |= (data[9] & 0x20) != 0 ? ControllerPacket.PLAY_FLAG : 0;
            buttons |= (data[9] & 0x40) != 0 ? ControllerPacket.LS_CLK_FLAG : 0;
            buttons |= (data[9] & 0x80) != 0 ? ControllerPacket.RS_CLK_FLAG : 0;
            buttons |= (data[10] & 0x01) != 0 ? ControllerPacket.SPECIAL_BUTTON_FLAG : 0;
            buttons |= (data[10] & 0x02) != 0 ? ControllerPacket.TOUCHPAD_FLAG : 0;
            buttons |= (data[10] & 0x04) != 0 ? ControllerPacket.MISC_FLAG : 0;
            if (edge) {
                // The other two Edge bits (0x10/0x20) are Fn, not back paddles.
                buttons |= (data[10] & 0x40) != 0 ? ControllerPacket.PADDLE1_FLAG : 0;
                buttons |= (data[10] & 0x80) != 0 ? ControllerPacket.PADDLE2_FLAG : 0;
            }

            for (int i = 0; i < 3; i++) {
                gyro[i] = signedShort(data, 16 + i * 2);
                accel[i] = signedShort(data, 22 + i * 2);
            }
            for (int i = 0; i < 2; i++) {
                int offset = 33 + i * 4;
                touching[i] = (data[offset] & 0x80) == 0;
                touchId[i] = data[offset] & 0x7F;
                int x = (data[offset + 1] & 0xFF) | ((data[offset + 2] & 0x0F) << 8);
                int y = ((data[offset + 2] & 0xFF) >> 4) | ((data[offset + 3] & 0xFF) << 4);
                touchX[i] = Math.min(x / 1920.0f, 1);
                touchY[i] = Math.min(y / 1070.0f, 1);
            }

            int status = (data[53] & 0xFF) >> 4;
            batteryPercentage = (byte)Math.min((data[53] & 0x0F) * 10 + 5, 100);
            switch (status) {
                case 0:
                    batteryState = MoonBridge.LI_BATTERY_STATE_DISCHARGING;
                    break;
                case 1:
                    batteryState = MoonBridge.LI_BATTERY_STATE_CHARGING;
                    break;
                case 2:
                    batteryState = MoonBridge.LI_BATTERY_STATE_FULL;
                    batteryPercentage = 100;
                    break;
                default:
                    batteryState = MoonBridge.LI_BATTERY_STATE_UNKNOWN;
                    batteryPercentage = MoonBridge.LI_BATTERY_PERCENTAGE_UNKNOWN;
                    break;
            }
            return true;
        }

        private static float stick(byte value) {
            int centered = (value & 0xFF) - 128;
            return centered / (centered < 0 ? 128.0f : 127.0f);
        }

        void reportTouchChanges(Input previous, UsbDriverListener listener, int controllerId) {
            for (int i = 0; i < 2; i++) {
                if (previous.touching[i] && findTouch(previous.touchId[i]) == -1) {
                    listener.reportControllerTouch(controllerId, MoonBridge.LI_TOUCH_EVENT_UP,
                            previous.touchId[i], previous.touchX[i], previous.touchY[i], 0);
                }
            }
            for (int i = 0; i < 2; i++) {
                if (!touching[i]) {
                    continue;
                }
                int old = previous.findTouch(touchId[i]);
                if (old == -1 || touchX[i] != previous.touchX[old] || touchY[i] != previous.touchY[old]) {
                    listener.reportControllerTouch(controllerId,
                            old == -1 ? MoonBridge.LI_TOUCH_EVENT_DOWN : MoonBridge.LI_TOUCH_EVENT_MOVE,
                            touchId[i], touchX[i], touchY[i], 1);
                }
            }
        }

        private int findTouch(int id) {
            for (int i = 0; i < 2; i++) {
                if (touching[i] && touchId[i] == id) {
                    return i;
                }
            }
            return -1;
        }
    }

    static final class Calibration {
        private final float[] bias = new float[6];
        private final float[] scale = {1 / 16.0f, 1 / 16.0f, 1 / 16.0f,
                9.80665f / 8192, 9.80665f / 8192, 9.80665f / 8192};

        void parse(byte[] data, int length) {
            if (length < 35 || length > data.length || data[0] != 0x05) {
                return;
            }
            float[] newBias = new float[6];
            float[] newScale = new float[6];
            int speed = signedShort(data, 19) + signedShort(data, 21);
            for (int i = 0; i < 6; i++) {
                int plus = signedShort(data, i < 3 ? 7 + i * 4 : 23 + (i - 3) * 4);
                int minus = signedShort(data, i < 3 ? 9 + i * 4 : 25 + (i - 3) * 4);
                int range = plus - minus;
                if (range <= 0) {
                    return;
                }
                newBias[i] = i < 3 ? signedShort(data, 1 + i * 2) : plus - range / 2;
                newScale[i] = i < 3 ? speed / (float)range : 2 * 9.80665f / range;
                // Match SDL's rejection of invalid factory calibration, retaining nominal scales.
                if (Math.abs(newBias[i]) > 1024 || Math.abs(1 - newScale[i] / scale[i]) > 0.5f) {
                    return;
                }
            }
            System.arraycopy(newBias, 0, bias, 0, 6);
            System.arraycopy(newScale, 0, scale, 0, 6);
        }

        void apply(Input input) {
            for (int i = 0; i < 3; i++) {
                input.gyro[i] = (input.gyro[i] - bias[i]) * scale[i];
                input.accel[i] = (input.accel[i] - bias[i + 3]) * scale[i + 3];
            }
        }
    }

    static final class Output {
        private static final int[] PLAYER_LIGHTS = {0x04, 0x0A, 0x15, 0x1B, 0x1F, 0x11, 0x0E};
        private final byte[] state = new byte[48];
        private final boolean enhancedRumble;
        private boolean waveform, audio, microphoneMuted;

        Output(boolean enhancedRumble) {
            this.enhancedRumble = enhancedRumble;
            state[0] = 0x02;
        }

        private byte[] build(int triggerFlags, int ledFlags) {
            byte[] report = state.clone();
            report[1] = (byte)(triggerFlags | (waveform ? 0 : enhancedRumble ? 0x02 : 0x03));
            report[2] = (byte)ledFlags;
            report[39] = (byte)(!waveform && enhancedRumble ? 0x04 : 0);
            if (audio) {
                report[1] |= (byte)0xf0; report[2] |= 3;
                if (microphoneMuted) { report[9] = 1; report[10] |= 0x10; }
            }
            if (waveform) report[3] = report[4] = 0;
            return report;
        }

        byte[] audio(boolean headphones, boolean muted) {
            if (!audio) { state[5] = 80; state[6] = 61; state[7] = 64; state[10] = 0; }
            audio = true; microphoneMuted = muted;
            // L/R to the headset, or the right speaker channel to the built-in speaker.
            state[8] = (byte)((state[8] & 0xcf) | (headphones ? 0 : 0x30));
            return build(0, 0);
        }
        byte[] audioControl(byte[] packet) {
            if (!DualSenseAudioPacket.validControl(packet) || packet[1] != 2) return null;
            for (int i = 0; i < 4; i++) if ((packet[4] & (0x10 << i)) != 0) state[5 + i] = packet[6 + i];
            if ((packet[5] & 1) != 0) state[9] = packet[10];
            if ((packet[5] & 2) != 0) state[10] = packet[11];
            return build(0, 0);
        }
        byte[] waveform(boolean enabled) {
            waveform = enabled;
            // Do not replay an emulated effect remembered before waveform playback.
            state[3] = state[4] = 0;
            return build(0, 0);
        }

        byte[] rumble(short lowFrequency, short highFrequency) {
            if (waveform) return null;
            int shift = enhancedRumble ? 8 : 9;
            state[3] = (byte)((highFrequency & 0xFFFF) >> shift);
            state[4] = (byte)((lowFrequency & 0xFFFF) >> shift);
            return build(0, 0);
        }

        byte[] led(byte r, byte g, byte b) {
            state[45] = r;
            state[46] = g;
            state[47] = b;
            return build(0, 0x04);
        }

        byte[] player(int playerNumber) {
            state[44] = playerNumber < 0 ? 0 : (byte)(0x20 | PLAYER_LIGHTS[playerNumber % PLAYER_LIGHTS.length]);
            return build(0, 0x10);
        }

        byte[] adaptiveTriggers(byte flags, byte typeLeft, byte typeRight, byte[] left, byte[] right) {
            int triggerFlags = flags & (LEFT_TRIGGER | RIGHT_TRIGGER);
            if (triggerFlags == 0 ||
                    ((triggerFlags & LEFT_TRIGGER) != 0 && (left == null || left.length != EFFECT_SIZE)) ||
                    ((triggerFlags & RIGHT_TRIGGER) != 0 && (right == null || right.length != EFFECT_SIZE))) {
                return null;
            }
            // Common-c supplies opaque device effects, including types unknown to this client.
            if ((triggerFlags & RIGHT_TRIGGER) != 0) {
                state[11] = typeRight;
                System.arraycopy(right, 0, state, 12, EFFECT_SIZE);
            }
            if ((triggerFlags & LEFT_TRIGGER) != 0) {
                state[22] = typeLeft;
                System.arraycopy(left, 0, state, 23, EFFECT_SIZE);
            }
            return build(triggerFlags, 0);
        }

        byte[] reset() {
            waveform = audio = microphoneMuted = false;
            Arrays.fill(state, (byte)0);
            state[0] = 0x02;
            state[11] = state[22] = 0x05;
            return build(LEFT_TRIGGER | RIGHT_TRIGGER, 0x14);
        }
    }
}
