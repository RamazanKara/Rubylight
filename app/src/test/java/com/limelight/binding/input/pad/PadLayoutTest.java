package com.limelight.binding.input.pad;

import org.junit.Test;

import static org.junit.Assert.*;

public class PadLayoutTest {
    private static final float DENSITY = 2.75f;

    @Test public void anUneditedLayoutSavesAsTheVersionAlone() {
        PadLayout layout = new PadLayout();
        assertEquals("v1", layout.serialize());
        assertTrue(PadLayout.parse("v1").isDefault());
        assertTrue(PadLayout.parse(null).isDefault());
    }

    @Test public void savedLayoutsComeBackAsTheyWere() {
        PadLayout layout = new PadLayout();
        layout.moveTo(PadLayout.Control.FACE, 1700, 600, 2400, 1080, DENSITY);
        layout.setScale(PadLayout.Control.FACE, 1.3f);
        layout.setOpacity(PadLayout.Control.LB, 40);
        layout.spec(PadLayout.Control.HOME).hidden = true;
        PadLayout back = PadLayout.parse(layout.serialize());
        PadLayout.Spec face = back.spec(PadLayout.Control.FACE);
        assertEquals(layout.spec(PadLayout.Control.FACE).cx, face.cx, 1e-4);
        assertEquals(layout.spec(PadLayout.Control.FACE).cy, face.cy, 1e-4);
        assertEquals(1.3f, face.scale, 1e-3);
        assertEquals(40, back.spec(PadLayout.Control.LB).opacity);
        assertTrue(back.spec(PadLayout.Control.HOME).hidden);
        // Only edited controls are written; the rest follow the default for the screen.
        assertFalse(back.spec(PadLayout.Control.LEFT_STICK).placed());
        assertEquals(layout.serialize(), back.serialize());
    }

    @Test public void aResizedButUnmovedControlKeepsFollowingTheDefaultPlace() {
        PadLayout layout = new PadLayout();
        layout.setScale(PadLayout.Control.DPAD, .8f);
        PadLayout back = PadLayout.parse(layout.serialize());
        assertFalse(back.spec(PadLayout.Control.DPAD).placed());
        assertEquals(.8f, back.spec(PadLayout.Control.DPAD).scale, 1e-3);
    }

    @Test public void brokenOrUnknownEntriesAreSkippedAndTheRestKept() {
        PadLayout layout = PadLayout.parse("v1|PADDLE,0.5,0.5,1,100,0|LB,x,0.2,1,100,0|RB,-,-,1.5,70,0|LT,0.1|"
                + "RT,7,-3,9,500,1");
        assertTrue(layout.spec(PadLayout.Control.LB).isDefault());
        assertEquals(1.5f, layout.spec(PadLayout.Control.RB).scale, 1e-3);
        assertEquals(70, layout.spec(PadLayout.Control.RB).opacity);
        assertTrue(layout.spec(PadLayout.Control.LT).isDefault());
        // Values out of range are clamped, not refused.
        PadLayout.Spec rt = layout.spec(PadLayout.Control.RT);
        assertEquals(1, rt.cx, 0);
        assertEquals(0, rt.cy, 0);
        assertEquals(PadLayout.MAX_SCALE, rt.scale, 0);
        assertEquals(PadLayout.MAX_OPACITY, rt.opacity);
        assertTrue(rt.hidden);
    }

    @Test public void anotherVersionLoadsAsTheDefault() {
        assertTrue(PadLayout.parse("v2|FACE,0.5,0.5,1,100,0").isDefault());
        assertTrue(PadLayout.parse("").isDefault());
    }

    @Test public void everyControlLiesOnTheSurfaceAtEverySize() {
        float[][] surfaces = {{2400, 1080}, {1080, 2400}, {2560, 1600}, {800, 480}};
        float[] box = new float[4];
        for (float[] surface : surfaces) {
            PadLayout layout = new PadLayout();
            for (PadLayout.Control control : PadLayout.Control.values()) layout.setScale(control, PadLayout.MAX_SCALE);
            for (PadLayout.Control control : PadLayout.Control.values()) {
                layout.bounds(control, surface[0], surface[1], DENSITY, box);
                assertTrue(control + " left", box[0] >= 0);
                assertTrue(control + " top", box[1] >= 0);
                assertTrue(control + " right", box[0] + box[2] <= surface[0] + .01f);
                assertTrue(control + " bottom", box[1] + box[3] <= surface[1] + .01f);
            }
        }
    }

    @Test public void theDefaultClustersDoNotOverlapOnPhonesInBothOrientations() {
        float[][] surfaces = {{2400, 1080}, {1080, 2400}, {1920, 1200}};
        PadLayout layout = new PadLayout();
        float[] a = new float[4], b = new float[4];
        PadLayout.Control[] controls = PadLayout.Control.values();
        for (float[] surface : surfaces) {
            for (int i = 0; i < controls.length; i++) {
                for (int j = i + 1; j < controls.length; j++) {
                    layout.bounds(controls[i], surface[0], surface[1], DENSITY, a);
                    layout.bounds(controls[j], surface[0], surface[1], DENSITY, b);
                    boolean overlap = a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3];
                    assertFalse(controls[i] + " overlaps " + controls[j] + " on " + surface[0] + "x" + surface[1], overlap);
                }
            }
        }
    }

    @Test public void aTabletsControlsStopGrowingAtTheUnitCap() {
        float tablet = PadLayout.unit(2560, 1600, 2);
        assertEquals(PadLayout.MAX_UNIT_DP * 2, tablet, 0);
        assertEquals(1080, PadLayout.unit(2400, 1080, DENSITY), 0);
        // Portrait: narrow enough that the two sides fit next to each other.
        assertEquals(1080 / 1.75f, PadLayout.unit(1080, 2400, DENSITY), .01f);
    }

    @Test public void movingKeepsTheWholeControlOnTheSurface() {
        PadLayout layout = new PadLayout();
        layout.moveTo(PadLayout.Control.FACE, -500, 5000, 2400, 1080, DENSITY);
        float[] box = layout.bounds(PadLayout.Control.FACE, 2400, 1080, DENSITY, new float[4]);
        assertEquals(0, box[0], .5f);
        assertEquals(1080 - box[3], box[1], .5f);
        // The saved center is a fraction, so it lands in the same place after a rotation back and forth.
        PadLayout.Spec spec = layout.spec(PadLayout.Control.FACE);
        assertTrue(spec.cx > 0 && spec.cx < 1 && spec.cy > 0 && spec.cy < 1);
    }

    @Test public void resetDropsEveryEdit() {
        PadLayout layout = new PadLayout();
        layout.setScale(PadLayout.Control.LB, 2);
        layout.spec(PadLayout.Control.RB).hidden = true;
        PadLayout copy = layout.copy();
        layout.reset();
        assertTrue(layout.isDefault());
        assertFalse("A copy is its own", copy.isDefault());
    }
}
