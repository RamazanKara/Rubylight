package com.limelight.binding.input.pad;

import com.limelight.binding.input.HeldInputState;
import com.limelight.nvstream.input.MouseButtonPacket;

import org.junit.Test;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/**
 * Pure-JVM tests for {@link PadTouchZoneHandler}, {@link PadHeldInputs}, and 2-slot
 * {@link PadTouchZoneHandler.AimForwarder} zero-jump pointer handoff.
 */
public class PadTouchZoneHandlerTest {

    private static final class RecordingSink implements HeldInputState.Sink,
            PadTouchZoneHandler.PointerOut, PadTouchZoneHandler.StickSink {
        final Map<Integer, Boolean> keys = new HashMap<>();
        final Map<Integer, Boolean> mice = new HashMap<>();
        final List<int[]> moves = new ArrayList<>();
        final List<Integer> taps = new ArrayList<>();
        float lastStickX, lastStickY;
        boolean lastStickRight;
        int keyTransitions;

        @Override
        public void key(int vk, boolean down) {
            keys.put(vk, down);
            keyTransitions++;
        }

        @Override
        public void mouse(int mouseButton, boolean down) {
            mice.put(mouseButton, down);
        }

        @Override
        public void move(int dx, int dy) {
            moves.add(new int[] {dx, dy});
        }

        @Override
        public void tap(int mouseButton) {
            taps.add(mouseButton);
        }

        @Override
        public void onStick(boolean right, float x, float y) {
            lastStickRight = right;
            lastStickX = x;
            lastStickY = y;
        }
    }

    @Test public void moveWasdFloatingOriginAndForwardAutoSprintHysteresis() {
        RecordingSink sink = new RecordingSink();
        PadHeldInputs held = new PadHeldInputs(sink);
        PadTouchZoneHandler.AimForwarder aim = new PadTouchZoneHandler.AimForwarder();
        PadTouchZone zone = new PadTouchZone("m1", PadTouchZone.Kind.MOVE_WASD, 0f, 0f, 0.5f, 1f);
        zone.autoSprint = true;
        zone.setSprintThreshold(0.85f);
        PadTouchZoneHandler h = new PadTouchZoneHandler(zone, held, aim, 1);

        float travel = PadTouchZoneHandler.defaultTravelRadius(800f, 720f, 2f);
        // Origin spawns right at initial touch (300, 400); no key emitted until deadzone exceeded.
        h.down(0, 300f, 400f, 300f, 400f, 800f, 720f, travel, 1_000L);
        assertFalse(Boolean.TRUE.equals(sink.keys.get(0x57))); // W

        // Full push forward (upwards on screen: dy negative) engages W + VK_LSHIFT.
        h.move(0, 300f, 250f, 300f, 250f, 1_020_000_000L, 2f);
        assertTrue(Boolean.TRUE.equals(sink.keys.get(0x57)));
        assertTrue(Boolean.TRUE.equals(sink.keys.get(PadTouchZoneHandler.VK_LSHIFT)));
        assertTrue(h.isSprinting());

        // Slight pullback inside hysteresis band keeps sprint engaged (threshold 0.85 - 0.08 = 0.77).
        h.move(0, 300f, 400f - travel * 0.80f, 300f, 400f - travel * 0.80f, 1_040_000_000L, 2f);
        assertTrue(h.isSprinting());

        // Drag straight sideways beyond release arc stops auto-sprint while still strafing.
        h.move(0, 300f + travel * 0.90f, 400f, 300f + travel * 0.90f, 400f, 1_060_000_000L, 2f);
        assertFalse(h.isSprinting());
        assertFalse(Boolean.TRUE.equals(sink.keys.get(PadTouchZoneHandler.VK_LSHIFT)));
        assertTrue(Boolean.TRUE.equals(sink.keys.get(0x44))); // D

        h.up(0, 1_080L);
        assertFalse(held.isKeyHeld(0x57));
        assertFalse(held.isKeyHeld(0x44));
        assertFalse(held.isKeyHeld(PadTouchZoneHandler.VK_LSHIFT));
    }

    @Test public void heldInputsRefcountAcrossZoneAndButtonAvoidsStuckOrPrematureRelease() {
        RecordingSink sink = new RecordingSink();
        PadHeldInputs held = new PadHeldInputs(sink);
        held.key(0x57, true);
        held.key(0x57, true);
        int transitionsBeforeFirstUp = sink.keyTransitions;
        held.key(0x57, false);
        assertTrue("Still held while refcount == 1", held.isKeyHeld(0x57));
        assertEquals("No premature host release when second source still holds", transitionsBeforeFirstUp, sink.keyTransitions);

        held.key(0x57, false);
        assertFalse(held.isKeyHeld(0x57));
        assertEquals(transitionsBeforeFirstUp + 1, sink.keyTransitions);
    }

    @Test public void lookMouseRespectsInvertYTapClickAndTwoSlotAimHandoffZeroJump() {
        RecordingSink sink = new RecordingSink();
        PadTouchZoneHandler.AimForwarder aim = new PadTouchZoneHandler.AimForwarder();
        aim.setSinks(sink, sink);
        PadTouchZone zone = new PadTouchZone("l1", PadTouchZone.Kind.LOOK_MOUSE, 0.5f, 0f, 1f, 1f);
        zone.invertY = true;
        zone.setSensitivityPercent(200);
        zone.tapAction = PadTouchZone.TapAction.LEFT_CLICK;
        PadTouchZoneHandler h = new PadTouchZoneHandler(zone, new PadHeldInputs(sink), aim, 1);
        h.setSinks(sink, sink);

        float travel = PadTouchZoneHandler.defaultTravelRadius(800f, 720f, 2f);
        // Quick tap emits tap callback cleanly.
        h.down(0, 900f, 360f, 900f, 360f, 800f, 720f, travel, 100L);
        h.up(0, 140L);
        assertEquals(1, sink.taps.size());
        assertEquals(Integer.valueOf(MouseButtonPacket.BUTTON_LEFT), sink.taps.get(0));

        // Slot 1 (Look zone) starts at (900, 360) and drags (+20, +10) -> with invertY=true, dy < 0.
        h.down(0, 900f, 360f, 900f, 360f, 800f, 720f, travel, 200L);
        h.move(0, 920f, 370f, 920f, 370f, 210_000_000L, 2f);
        assertFalse(sink.moves.isEmpty());
        int[] firstDelta = sink.moves.get(sink.moves.size() - 1);
        assertTrue("dx positive", firstDelta[0] > 0);
        assertTrue("dy inverted negative on downwards drag", firstDelta[1] < 0);

        // Slot 2 (Fire button drag-aim) engages simultaneously far away at (1400, 500) -> NO jump!
        int beforeFireMoves = sink.moves.size();
        aim.begin(2, 1400f, 500f);
        assertEquals("Begin on second slot must emit zero jump delta", beforeFireMoves, sink.moves.size());
        assertEquals(2, aim.activeCount());

        // Move slot 2 by (+10, 0), then lift slot 1 -> remaining slot 2 continues cleanly without jumping.
        aim.move(2, 1410f, 500f, 220_000_000L, 2f);
        h.up(0, 225L);
        assertEquals(1, aim.activeCount());
        int countAfterLift = sink.moves.size();
        aim.move(2, 1420f, 500f, 235_000_000L, 2f);
        assertTrue("Remaining finger keeps emitting clean incremental deltas", sink.moves.size() > countAfterLift);
        assertTrue(sink.moves.get(sink.moves.size() - 1)[0] > 0);
        assertTrue(Math.abs(sink.moves.get(sink.moves.size() - 1)[0]) < 150);
        aim.end(2);
        assertEquals(0, aim.activeCount());
    }

    @Test public void lookStickDrivesAnalogStickAndDecaysAfterRelease() {
        RecordingSink sink = new RecordingSink();
        PadTouchZoneHandler.AimForwarder aim = new PadTouchZoneHandler.AimForwarder();
        aim.setSinks(sink, sink);
        PadTouchZone zone = new PadTouchZone("ls", PadTouchZone.Kind.LOOK_STICK, 0.5f, 0f, 1f, 1f);
        PadTouchZoneHandler h = new PadTouchZoneHandler(zone, new PadHeldInputs(sink), aim, 1);
        h.setSinks(sink, sink);

        float travel = PadTouchZoneHandler.defaultTravelRadius(800f, 720f, 2f);
        h.down(0, 900f, 360f, 900f, 360f, 800f, 720f, travel, 100L);
        h.move(0, 940f, 360f, 940f, 360f, 120_000_000L, 2f);
        h.move(0, 980f, 360f, 980f, 360f, 136_000_000L, 2f);
        assertTrue(sink.lastStickX > 0f);
        h.up(0, 140L);
        assertEquals(0f, h.stickX(), 1e-4f);
        assertEquals(0f, h.stickY(), 1e-4f);
        assertTrue(h.isDecayActive());
        h.tickDecay(500_000_000L);
        assertFalse(h.isDecayActive());
        assertEquals(0f, sink.lastStickX, 0f);
        assertEquals(0f, sink.lastStickY, 0f);
    }

    @Test public void moveZoneDoesNotOverwriteLookStickSinkAndAimCancelZeroesImmediately() {
        RecordingSink lookSink = new RecordingSink();
        RecordingSink moveSink = new RecordingSink();
        PadTouchZoneHandler.AimForwarder aim = new PadTouchZoneHandler.AimForwarder();
        PadTouchZone lookZone = new PadTouchZone("ls", PadTouchZone.Kind.LOOK_STICK, 0.5f, 0f, 1f, 1f);
        PadTouchZoneHandler look = new PadTouchZoneHandler(lookZone, new PadHeldInputs(lookSink), aim, 1);
        look.setSinks(lookSink, lookSink);

        PadTouchZone moveZone = new PadTouchZone("ms", PadTouchZone.Kind.MOVE_STICK, 0f, 0f, 0.5f, 1f);
        PadTouchZoneHandler move = new PadTouchZoneHandler(moveZone, new PadHeldInputs(moveSink), aim, 2);
        move.setSinks(null, moveSink);

        float travel = PadTouchZoneHandler.defaultTravelRadius(800f, 720f, 2f);
        aim.begin(99, 100f, 100f);
        aim.move(99, 130f, 100f, 120_000_000L, 2f);
        aim.move(99, 190f, 100f, 136_000_000L, 2f);
        assertTrue(lookSink.lastStickRight);
        assertTrue("LOOK_STICK sink must remain untouched after MOVE_STICK setSinks", lookSink.lastStickX > 0f);
        assertEquals("MOVE_STICK sink must not receive right-stick look output", 0f, moveSink.lastStickX, 0f);

        aim.cancel(99);
        assertFalse(aim.isDecaying());
        assertEquals(0, aim.activeCount());
        assertEquals(0f, lookSink.lastStickX, 0f);
        assertEquals(0f, lookSink.lastStickY, 0f);
        move.down(0, 100f, 300f, 100f, 300f, 800f, 720f, travel, 200L);
        move.release();
    }
}
