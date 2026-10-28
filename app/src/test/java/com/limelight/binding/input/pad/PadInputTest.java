package com.limelight.binding.input.pad;

import com.limelight.nvstream.input.ControllerPacket;

import org.junit.Test;

import static org.junit.Assert.*;

public class PadInputTest {
    private static final float SIZE = 400;

    private static int faceAt(float fx, float fy) {
        return PadInput.face(fx * SIZE, fy * SIZE, SIZE);
    }

    @Test public void eachFaceButtonSitsWhereAnXboxPadHasIt() {
        assertEquals(ControllerPacket.A_FLAG, faceAt(.5f, .78f));
        assertEquals(ControllerPacket.B_FLAG, faceAt(.78f, .5f));
        assertEquals(ControllerPacket.X_FLAG, faceAt(.22f, .5f));
        assertEquals(ControllerPacket.Y_FLAG, faceAt(.5f, .22f));
    }

    @Test public void aThumbBetweenTwoNeighboursPressesBoth() {
        assertEquals(ControllerPacket.A_FLAG | ControllerPacket.B_FLAG, faceAt(.64f, .64f));
        assertEquals(ControllerPacket.X_FLAG | ControllerPacket.Y_FLAG, faceAt(.36f, .36f));
        // Nearer one button than the chord margin allows: only that one.
        assertEquals(ControllerPacket.A_FLAG, faceAt(.55f, .74f));
    }

    @Test public void theMiddleAndTheCornersPressNothing() {
        assertEquals(0, faceAt(.5f, .5f));
        assertEquals(0, faceAt(.02f, .02f));
        assertEquals(0, PadInput.face(10, 10, 0));
    }

    @Test public void theDpadSplitsIntoEightDirectionsAroundADeadCenter() {
        assertEquals(0, PadInput.dpad(SIZE / 2, SIZE / 2, SIZE));
        assertEquals(ControllerPacket.UP_FLAG, PadInput.dpad(SIZE / 2, 10, SIZE));
        assertEquals(ControllerPacket.RIGHT_FLAG, PadInput.dpad(SIZE - 10, SIZE / 2, SIZE));
        assertEquals(ControllerPacket.DOWN_FLAG | ControllerPacket.LEFT_FLAG, PadInput.dpad(40, SIZE - 40, SIZE));
        assertEquals(0, PadInput.dpad(1, 1, 0));
    }

    @Test public void aStickTapFollowedQuicklyByATouchHoldsTheStickClick() {
        assertTrue(PadInput.stickTap(120, .1f));
        assertFalse("A flick is a push, not a tap", PadInput.stickTap(120, .9f));
        assertFalse("A long rest is not a tap", PadInput.stickTap(600, 0));
        assertTrue(PadInput.stickClick(1000, 1000 + PadInput.STICK_CLICK_MS));
        assertFalse(PadInput.stickClick(1000, 1001 + PadInput.STICK_CLICK_MS));
        assertFalse("Never tapped", PadInput.stickClick(0, 50));
    }

    @Test public void simpleControlsHoldTheirOwnButton() {
        assertEquals(ControllerPacket.LB_FLAG, PadInput.buttonFlag(PadLayout.Control.LB));
        assertEquals(ControllerPacket.RB_FLAG, PadInput.buttonFlag(PadLayout.Control.RB));
        assertEquals(ControllerPacket.LS_CLK_FLAG, PadInput.buttonFlag(PadLayout.Control.L3));
        assertEquals(ControllerPacket.RS_CLK_FLAG, PadInput.buttonFlag(PadLayout.Control.RIGHT_STICK));
        assertEquals(ControllerPacket.BACK_FLAG, PadInput.buttonFlag(PadLayout.Control.SELECT));
        assertEquals(ControllerPacket.PLAY_FLAG, PadInput.buttonFlag(PadLayout.Control.START));
        assertEquals(ControllerPacket.SPECIAL_BUTTON_FLAG, PadInput.buttonFlag(PadLayout.Control.HOME));
        assertEquals(0, PadInput.buttonFlag(PadLayout.Control.LT));
        assertTrue(PadInput.isTrigger(PadLayout.Control.RT));
        assertFalse(PadInput.isTrigger(PadLayout.Control.RB));
    }
}
