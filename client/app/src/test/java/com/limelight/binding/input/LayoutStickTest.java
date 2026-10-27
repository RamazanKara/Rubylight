package com.limelight.binding.input;

import com.limelight.nvstream.input.ControllerPacket;
import org.junit.Test;
import static com.limelight.binding.input.LayoutStick.DOWN;
import static com.limelight.binding.input.LayoutStick.LEFT;
import static com.limelight.binding.input.LayoutStick.RIGHT;
import static com.limelight.binding.input.LayoutStick.UP;
import static org.junit.Assert.*;

public class LayoutStickTest {
    @Test public void aFloatingStickCentersWhereTheThumbLandsAndSendsNothingThere() {
        LayoutStick stick = new LayoutStick(true, 0);
        stick.down(30, 170, 400, 200, 50);
        assertEquals(30, stick.centerX(), 0);
        assertEquals(170, stick.centerY(), 0);
        assertEquals(0, stick.x, 0);
        assertEquals(0, stick.y, 0);
        // Half a radius right and up from the landing point, wherever in the zone that is.
        stick.move(55, 145);
        assertEquals(.5f, stick.x, 1e-6);
        assertEquals(.5f, stick.y, 1e-6);
    }

    @Test public void aFixedStickUsesTheControlCentreAndMovesAtOnce() {
        LayoutStick stick = new LayoutStick(false, 0);
        stick.down(150, 100, 200, 200, 100);
        assertEquals(100, stick.centerX(), 0);
        assertEquals(.5f, stick.x, 1e-6);
        assertEquals(0, stick.y, 1e-6);
    }

    @Test public void travelPastTheRimClampsToFullDeflection() {
        LayoutStick stick = new LayoutStick(true, 0);
        stick.down(100, 100, 400, 400, 50);
        stick.move(100, 400);
        assertEquals(0, stick.x, 1e-6);
        assertEquals(-1, stick.y, 1e-6);
    }

    @Test public void theDeadzoneSendsNothingThenRescalesToReachEveryValue() {
        LayoutStick stick = new LayoutStick(true, .2f);
        stick.down(0, 0, 100, 100, 100);
        stick.move(19, 0);
        assertEquals(0, stick.x, 0);
        stick.move(20, 0);
        assertEquals(0, stick.x, 0);
        stick.move(60, 0);
        assertEquals(.5f, stick.x, 1e-6);
        stick.move(100, 0);
        assertEquals(1, stick.x, 1e-6);
        // Up is positive: the finger is above the center.
        stick.move(0, -21);
        assertEquals(0, stick.x, 1e-6);
        assertTrue(stick.y > 0 && stick.y < .05f);
    }

    @Test public void releaseRecentersAndLaterMovesAreIgnored() {
        LayoutStick stick = new LayoutStick(false, 0);
        stick.down(200, 100, 200, 200, 100);
        stick.release();
        assertFalse(stick.active());
        assertEquals(0, stick.x, 0);
        stick.move(0, 0);
        assertEquals(0, stick.x, 0);
        assertEquals(0, stick.y, 0);
    }

    @Test public void wasdDirectionsFollowTheDiagonalThreshold() {
        assertEquals(0, LayoutStick.directions(0, 0, 45));
        assertEquals(UP, LayoutStick.directions(0, 1, 45));
        assertEquals(DOWN, LayoutStick.directions(0, -1, 45));
        assertEquals(LEFT, LayoutStick.directions(-1, 0, 45));
        assertEquals(RIGHT, LayoutStick.directions(1, 0, 45));
        // 45 degrees of diagonal: eight equal sectors, so up-right holds W and D.
        assertEquals(UP | RIGHT, LayoutStick.directions(.7f, .7f, 45));
        assertEquals(DOWN | LEFT, LayoutStick.directions(-.7f, -.7f, 45));
        // 15 degrees off the axis is a straight push at 45 but a diagonal at 90.
        float x = (float) Math.cos(Math.toRadians(15)), y = (float) Math.sin(Math.toRadians(15));
        assertEquals(RIGHT, LayoutStick.directions(x, y, 45));
        assertEquals(UP | RIGHT, LayoutStick.directions(x, y, 90));
        // No diagonal range never holds two keys, even 40 degrees off the axis.
        float x40 = (float) Math.cos(Math.toRadians(40)), y40 = (float) Math.sin(Math.toRadians(40));
        assertEquals(RIGHT, LayoutStick.directions(x40, y40, 0));
        assertEquals(UP, LayoutStick.directions(y40, x40, 0));
        // Exactly on a boundary with no overlap still holds one key.
        assertEquals(1, Integer.bitCount(LayoutStick.directions(1, 1, 0)));
        // Left is continuous across the -180/180 seam.
        assertEquals(LEFT, LayoutStick.directions(-1, .01f, 45));
        assertEquals(LEFT, LayoutStick.directions(-1, -.01f, 45));
    }

    @Test public void dpadDirectionsBecomeGamepadFlags() {
        assertEquals(ControllerPacket.UP_FLAG | ControllerPacket.RIGHT_FLAG, LayoutStick.dpadFlags(UP | RIGHT));
        assertEquals(ControllerPacket.DOWN_FLAG | ControllerPacket.LEFT_FLAG, LayoutStick.dpadFlags(DOWN | LEFT));
        assertEquals(0, LayoutStick.dpadFlags(0));
    }

    @Test public void snappingRoundsToTheGridAndStaysOnTheSurface() {
        // 1000 px surface, 100 px control, 50 px grid.
        assertEquals(100, LayoutStick.snap(110, 100, 1000, 50), 0);
        assertEquals(150, LayoutStick.snap(126, 100, 1000, 50), 0);
        assertEquals(0, LayoutStick.snap(-40, 100, 1000, 50), 0);
        assertEquals(900, LayoutStick.snap(990, 100, 1000, 50), 0);
        // A control that cannot reach the far edge on the grid sits flush against it.
        assertEquals(880, LayoutStick.snap(870, 120, 1000, 50), 0);
        // Off: only kept on the surface.
        assertEquals(123.5f, LayoutStick.snap(123.5f, 100, 1000, 0), 0);
        assertEquals(900, LayoutStick.snap(950, 100, 1000, 0), 0);
    }
}
