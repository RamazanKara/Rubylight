package com.limelight.binding.input.pad;

import com.limelight.nvstream.input.MouseButtonPacket;

import org.junit.Test;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/**
 * Pure-JVM tests for {@link PadCustomButtonCodec}, {@link PadTouchZone}, and {@link PadCustomButton}
 * backward compatibility, percent-escaping, caps, JSON Hub Sync round-trip, and old-client safety.
 */
public class PadCustomButtonCodecTest {

    @Test public void emptyLayoutSerializesToExactV1WithoutNewTokens() {
        PadLayout empty = new PadLayout();
        assertEquals("v1", empty.serialize());
        assertTrue(empty.touchZones().isEmpty());
        assertTrue(empty.customButtons().isEmpty());
    }

    @Test public void codecRoundTripsZonesAndCustomButtonsWithEscapedLabels() {
        PadLayout layout = new PadLayout();
        PadTouchZone move = new PadTouchZone("zone_m", PadTouchZone.Kind.MOVE_WASD, 0f, 0.1f, 0.45f, 0.95f);
        move.setOpacity(15);
        move.autoSprint = true;
        move.setSprintThreshold(0.88f);
        move.setDeadzone(0.12f);
        layout.addTouchZone(move);

        PadTouchZone look = new PadTouchZone("zone_l", PadTouchZone.Kind.LOOK_MOUSE, 0.45f, 0f, 1f, 1f);
        look.setOpacity(0);
        look.setSensitivityPercent(185);
        look.invertY = true;
        look.setAccelerationCurve("strong");
        look.tapAction = PadTouchZone.TapAction.RIGHT_CLICK;
        layout.addTouchZone(look);

        PadCustomButton special = PadCustomButton.chord("b1", new int[] {0xA2, 0xA0, 0x43},
                "C=1|2,3%ü", 0.75f, 0.42f);
        special.shape = PadCustomButton.Shape.PILL;
        special.setSize(1.25f, 0.9f);
        special.setOpacity(75);
        special.toggle = true;
        special.dragAim = true;
        layout.addCustomButton(special);

        PadCustomButton wheel = PadCustomButton.scroll("b2", -1, "Dn", 0.85f, 0.72f);
        wheel.shape = PadCustomButton.Shape.RECT;
        layout.addCustomButton(wheel);

        PadCustomButton mouse = PadCustomButton.mouse("b3", MouseButtonPacket.BUTTON_X1, "Side", 0.62f, 0.62f);
        layout.addCustomButton(mouse);

        String wire = layout.serialize();
        assertFalse("Wire format must not emit ambiguous 'B' control token", wire.contains("|B,"));
        assertTrue(wire.contains("|zone="));
        assertTrue(wire.contains("|kbm="));

        PadLayout parsed = PadLayout.parse(wire);
        assertEquals(2, parsed.touchZones().size());
        PadTouchZone pm = parsed.touchZone("zone_m");
        assertEquals(PadTouchZone.Kind.MOVE_WASD, pm.kind);
        assertEquals(15, pm.opacity);
        assertTrue(pm.autoSprint);
        assertEquals(0.88f, pm.sprintThreshold, 0.005f);
        assertEquals(0.12f, pm.deadzone, 0.005f);

        PadTouchZone pl = parsed.touchZone("zone_l");
        assertEquals(PadTouchZone.Kind.LOOK_MOUSE, pl.kind);
        assertEquals(0, pl.opacity);
        assertEquals(185, pl.sensitivityPercent);
        assertTrue(pl.invertY);
        assertEquals("strong", pl.accelerationCurve);
        assertEquals(PadTouchZone.TapAction.RIGHT_CLICK, pl.tapAction);

        assertEquals(3, parsed.customButtons().size());
        PadCustomButton ps = parsed.customButton("b1");
        assertEquals(PadCustomButton.BindingKind.KEY_CHORD, ps.bindingKind);
        assertArrayEquals(new int[] {0xA2, 0xA0, 0x43}, ps.keys());
        assertEquals("C=1|2,3%ü", ps.label);
        assertEquals(PadCustomButton.Shape.PILL, ps.shape);
        assertTrue(ps.toggle);
        assertTrue(ps.dragAim);

        PadCustomButton pw = parsed.customButton("b2");
        assertEquals(PadCustomButton.BindingKind.MOUSE_SCROLL, pw.bindingKind);
        assertEquals(-1, pw.scrollDirection);
        assertEquals(PadCustomButton.Shape.RECT, pw.shape);
    }

    @Test public void olderNightjarDecoderIgnoresZoneAndKbmTokensWhileKeepingControls() {
        PadLayout layout = new PadLayout();
        layout.spec(PadLayout.Control.A).cx = 0.80f;
        layout.spec(PadLayout.Control.A).cy = 0.70f;
        layout.addTouchZone(new PadTouchZone("z1", PadTouchZone.Kind.LOOK_MOUSE, 0.5f, 0f, 1f, 1f));
        layout.addCustomButton(PadCustomButton.key("k1", 0x43, "C", 0.7f, 0.5f));
        String wire = layout.serialize();

        // Simulate an older PadLayoutCodec build that splits on '|' and ignores tokens whose
        // first comma field does not match PadLayout.Control.named(...).
        String[] parts = wire.split("\\|");
        int unknownTokens = 0;
        boolean sawControlA = false;
        for (int i = 1; i < parts.length; i++) {
            String[] fields = parts[i].split(",", -1);
            if (PadLayout.Control.named(fields[0]) != null) {
                if ("A".equals(fields[0])) sawControlA = true;
            } else {
                unknownTokens++;
            }
        }
        assertTrue(sawControlA);
        assertEquals(2, unknownTokens);
    }

    @Test public void enforcesMaxCapsOnZonesAndButtonsAndJsonSyncRoundTrip() {
        PadLayout layout = new PadLayout();
        for (int i = 0; i < 10; i++) {
            layout.addTouchZone(new PadTouchZone("z" + i, PadTouchZone.Kind.LOOK_MOUSE, 0.5f, 0f, 1f, 1f));
        }
        for (int i = 0; i < 30; i++) {
            layout.addCustomButton(PadCustomButton.key("b" + i, 0x41 + (i % 26), "", 0.5f, 0.5f));
        }
        assertEquals(PadTouchZone.MAX_ZONES, layout.touchZones().size());
        assertEquals(PadCustomButton.MAX_CUSTOM_BUTTONS, layout.customButtons().size());

        String json = PadCustomButtonCodec.toJson(layout);
        PadLayout imported = new PadLayout();
        assertTrue(PadCustomButtonCodec.applyJson(imported, json));
        assertEquals(PadTouchZone.MAX_ZONES, imported.touchZones().size());
        assertEquals(PadCustomButton.MAX_CUSTOM_BUTTONS, imported.customButtons().size());
        assertEquals("b0", imported.customButtons().get(0).id);
        assertFalse(PadCustomButtonCodec.applyJson(imported, "{bad"));
    }
}
