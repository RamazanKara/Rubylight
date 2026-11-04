package com.limelight.binding.input.pad;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

import static com.limelight.binding.input.pad.WasdKeys.VK_A;
import static com.limelight.binding.input.pad.WasdKeys.VK_D;
import static com.limelight.binding.input.pad.WasdKeys.VK_S;
import static com.limelight.binding.input.pad.WasdKeys.VK_W;
import static org.junit.Assert.*;

/** The modern controller's WASD stick: sectors, hysteresis, feedback and letting go (nightjar#61). */
public class WasdKeysTest {
    private final List<String> sent = new ArrayList<>();
    private final WasdKeys wasd = new WasdKeys((vk, down) -> sent.add((down ? "+" : "-") + (char) vk));

    private static float x(double degrees, float push) {
        return (float) (Math.cos(Math.toRadians(degrees)) * push);
    }

    private static float y(double degrees, float push) {
        return (float) (Math.sin(Math.toRadians(degrees)) * push);
    }

    private WasdKeys.Feedback at(double degrees) {
        return wasd.update(x(degrees, 1), y(degrees, 1));
    }

    @Test public void eachOfTheEightDirectionsHoldsItsKeys() {
        String[] expected = {"D", "WD", "W", "WA", "A", "SA", "S", "SD"};
        for (int sector = 0; sector < 8; sector++) {
            wasd.release();
            at(sector * 45);
            StringBuilder held = new StringBuilder();
            for (int vk : new int[] {VK_W, VK_S, VK_A, VK_D}) if (wasd.holds(vk)) held.append((char) vk);
            String want = expected[sector];
            assertEquals(sector * 45 + " degrees", want.length(), held.length());
            for (char key : want.toCharArray()) assertTrue(sector * 45 + " degrees holds " + key, wasd.holds(key));
            assertEquals(sector, wasd.sector());
        }
    }

    @Test public void aThumbOnASectorBorderDoesNotFlicker() {
        at(90); // W
        sent.clear();
        // Wobbling around the W / W+A border at 112.5 degrees, within the hysteresis, changes nothing.
        for (double degrees : new double[] {110, 114, 118, 112.5, 120, 115}) at(degrees);
        assertTrue("No key changed: " + sent, sent.isEmpty());
        assertEquals(2, wasd.sector());
        // Clearly past the border, W+A takes over and keeps the next wobble back across the border.
        assertEquals(WasdKeys.Feedback.DIAGONAL, at(112.5 + WasdKeys.HYSTERESIS + 1));
        assertEquals(List.of("+A"), sent);
        sent.clear();
        at(108);
        assertTrue(sent.isEmpty());
        assertEquals(3, wasd.sector());
        // Back past W+A's own border by the hysteresis: only W again.
        assertEquals(WasdKeys.Feedback.DIRECTION, at(112.5 - WasdKeys.HYSTERESIS - 1));
        assertEquals(List.of("-A"), sent);
    }

    @Test public void theBorderAtTheBackOfTheCircleWrapsAround() {
        at(-10); // D
        sent.clear();
        at(-28);
        at(350);
        assertTrue(sent.isEmpty());
        assertEquals(WasdKeys.Feedback.DIAGONAL, at(-40)); // S+D
        assertEquals(List.of("+S"), sent);
    }

    @Test public void theDeadzoneHoldsNothingAndStaysQuiet() {
        // The modern stick deadzone already gives 0 inside it.
        assertEquals(WasdKeys.Feedback.NONE, wasd.update(0, 0));
        // Just past it, a sliver of travel does not yet press a key.
        assertEquals(WasdKeys.Feedback.NONE, wasd.update(x(90, WasdKeys.ENGAGE * .8f), y(90, WasdKeys.ENGAGE * .8f)));
        assertTrue(sent.isEmpty());
        assertEquals(WasdKeys.NONE, wasd.sector());
        assertEquals(WasdKeys.Feedback.DIRECTION, wasd.update(x(90, WasdKeys.ENGAGE * 1.5f), y(90, WasdKeys.ENGAGE * 1.5f)));
        // Easing back toward the center keeps W until the stick is nearly home, then lets go silently.
        assertEquals(WasdKeys.Feedback.NONE, wasd.update(x(90, WasdKeys.ENGAGE * .8f), y(90, WasdKeys.ENGAGE * .8f)));
        assertTrue(wasd.holds(VK_W));
        assertEquals(WasdKeys.Feedback.NONE, wasd.update(0, 0));
        assertEquals(List.of("+W", "-W"), sent);
        // Turning inside the deadzone never answers.
        for (int degrees = 0; degrees < 360; degrees += 15) {
            assertEquals(WasdKeys.Feedback.NONE, wasd.update(x(degrees, .01f), y(degrees, .01f)));
        }
        assertEquals(WasdKeys.Feedback.NONE, wasd.update(Float.NaN, Float.NaN));
    }

    @Test public void feedbackComesOnlyWhenTheSectorChanges() {
        assertEquals(WasdKeys.Feedback.DIRECTION, at(90));
        // Staying in W, however far or wherever inside the sector, is silent.
        assertEquals(WasdKeys.Feedback.NONE, at(80));
        assertEquals(WasdKeys.Feedback.NONE, wasd.update(x(95, .5f), y(95, .5f)));
        assertEquals(WasdKeys.Feedback.DIAGONAL, at(135));
        assertEquals(WasdKeys.Feedback.NONE, at(140));
        assertEquals(WasdKeys.Feedback.DIRECTION, at(180));
        assertEquals(WasdKeys.Feedback.DIAGONAL, at(225));
        assertEquals(WasdKeys.Feedback.DIRECTION, at(270));
        // A jump straight from S to D is a direction too, with S up before D goes down.
        sent.clear();
        assertEquals(WasdKeys.Feedback.DIRECTION, at(0));
        assertEquals(List.of("-S", "+D"), sent);
        // Leaving every sector is silent.
        assertEquals(WasdKeys.Feedback.NONE, wasd.update(0, 0));
    }

    @Test public void aDiagonalHoldsBothKeysAndReleaseLetsGoOfEveryOne() {
        at(45);
        assertTrue(wasd.holds(VK_W));
        assertTrue(wasd.holds(VK_D));
        assertFalse(wasd.holds(VK_A));
        assertFalse(wasd.holds(VK_S));
        sent.clear();
        wasd.release();
        assertEquals(2, sent.size());
        assertTrue(sent.containsAll(List.of("-W", "-D")));
        assertEquals(WasdKeys.NONE, wasd.sector());
        // A second release sends nothing more.
        wasd.release();
        assertEquals(2, sent.size());
    }

    @Test public void heldKeysDrawAsDirections() {
        at(135);
        assertEquals(com.limelight.binding.input.LayoutStick.UP | com.limelight.binding.input.LayoutStick.LEFT,
                wasd.directions());
        wasd.release();
        assertEquals(0, wasd.directions());
    }
}
