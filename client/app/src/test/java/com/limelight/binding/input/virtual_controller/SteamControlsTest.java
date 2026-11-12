package com.limelight.binding.input.virtual_controller;

import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

/** The Steam Controller controls' wire values and how their trackpads share touchpad 0. */
public class SteamControlsTest {
    /** The PC's split (rust/core/src/steam_pad.rs touch_pad): x below 0.5 is the left pad. */
    private static int hostPad(float x) {
        return x >= 0.5f ? SteamControls.RIGHT_PAD : SteamControls.LEFT_PAD;
    }

    /** The PC's stretch (steam_pad.rs touch_x): a half back to the pad's full width. */
    private static float hostX(float x, int pad) {
        return Math.max(0f, Math.min(1f, (x - pad / 2f) * 2f));
    }

    @Test
    public void eachTrackpadLandsOnItsOwnPadAcrossItsFullWidth() {
        for (int pad : new int[]{SteamControls.LEFT_PAD, SteamControls.RIGHT_PAD}) {
            for (float across : new float[]{0f, 0.25f, 0.5f, 0.75f, 1f}) {
                float x = SteamControls.touchX(pad, across);
                assertEquals("pad of " + across, pad, hostPad(x));
                assertEquals("x of " + across, across, hostX(x, pad), 0.001f);
            }
        }
        // A finger dragged off the surface stays on its pad.
        assertEquals(SteamControls.LEFT_PAD, hostPad(SteamControls.touchX(SteamControls.LEFT_PAD, 1.4f)));
        assertEquals(SteamControls.RIGHT_PAD, hostPad(SteamControls.touchX(SteamControls.RIGHT_PAD, -0.3f)));
        assertEquals(0f, SteamControls.touchY(-0.2f), 0f);
        assertEquals(1f, SteamControls.touchY(1.5f), 0f);
        assertEquals(0f, SteamControls.pressure(Float.NaN), 0f);
        assertEquals(1f, SteamControls.pressure(3f), 0f);
    }

    @Test
    public void theTwoPadsUseTheirOwnFingers() {
        assertNotEquals(SteamControls.pointerId(SteamControls.LEFT_PAD), SteamControls.pointerId(SteamControls.RIGHT_PAD));
        // Apart from a physical touchpad's first fingers on the same pad.
        assertTrue(SteamControls.pointerId(SteamControls.LEFT_PAD) > 16);
    }

    @Test
    public void flagsAndCapabilitiesMatchTheSteamControllerProtocol() {
        // client/protocol/core/src/steam_controller.rs
        assertEquals(0x0040_0000, SteamControls.LEFT_GRIP_TOUCH_FLAG);
        assertEquals(0x0080_0000, SteamControls.RIGHT_GRIP_TOUCH_FLAG);
        assertEquals(0x0100_0000, SteamControls.LEFT_STICK_TOUCH_FLAG);
        assertEquals(0x0200_0000, SteamControls.RIGHT_STICK_TOUCH_FLAG);
        assertEquals(0x200, SteamControls.LI_CCAP_GRIP_SENSE);
        assertEquals(0x400, SteamControls.LI_CCAP_STICK_TOUCH);
        // R4, L4, R5, L5 are paddles 1 to 4.
        assertEquals(0x10000, SteamControls.R4_FLAG);
        assertEquals(0x20000, SteamControls.L4_FLAG);
        assertEquals(0x40000, SteamControls.R5_FLAG);
        assertEquals(0x80000, SteamControls.L5_FLAG);
        assertEquals(ControllerPacket.PADDLE1_FLAG | ControllerPacket.PADDLE2_FLAG |
                ControllerPacket.PADDLE3_FLAG | ControllerPacket.PADDLE4_FLAG, SteamControls.BUTTONS);

        assertEquals(MoonBridge.LI_CCAP_TOUCHPAD | 0x200 | 0x400, SteamControls.CAPABILITIES);
        // Not one touchpad per pad: touchpad 0's halves are the pads.
        assertEquals(0, SteamControls.CAPABILITIES & MoonBridge.LI_CCAP_DUAL_TOUCHPAD);
        // Not SC26 buttons: those make every pad and trigger click explicit, which the controls cannot send.
        assertEquals(0, SteamControls.CAPABILITIES & 0x1000);
    }
}
