package com.limelight.binding.input.pad;

import android.app.Application;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.HashMap;
import java.util.Map;

import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.DENSITY;
import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.HEIGHT;
import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.MEMBERS;
import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.RIGHT_STICK;
import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.WIDTH;
import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.assertBox;
import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.assertCenter;
import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.box;
import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.layout;
import static org.junit.Assert.*;

/**
 * Geometry, opacity, diamond-cluster detection, element mapping, and end-to-end controller/editor verification for
 * classic-to-modern layout migration.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi",
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class)
public class ClassicLayoutMigrationGeometryTest {
    /** The Retroid's layout after the upgrade: Nintendo-swapped classic buttons, the cluster never placed. */
    private static final String DEVICE = "v1|LEFT_STICK,0.3953,0.2927,0.868,100,0|RIGHT_STICK,0.8740,0.7629,1.072,100,0|"
            + "DPAD,0.1404,0.7771,1.403,100,0|FACE,-,-,1.000,100,1|LB,0.2219,0.4927,1.051,100,0,0.670,1.051|"
            + "RB,0.7771,0.4927,1.051,100,0,0.670,1.051|LT,0.0516,0.4927,1.051,100,0,0.670,1.051|"
            + "RT,0.9479,0.4927,1.051,100,0,0.670,1.051|SELECT,0.2964,0.9371,1.523,100,0,1.523,0.887|"
            + "START,0.5734,0.7420,1.523,100,0,1.523,0.887|HOME,0.4583,0.9371,1.523,100,0,1.523,0.887|"
            + "A,0.9479,0.2215,0.921,100,0|B,0.8740,0.3600,0.921,100,0|X,0.8740,0.0829,0.921,100,0|"
            + "Y,0.7995,0.2215,0.921,100,0";

    private final ClassicLayoutMigrationFixture f = new ClassicLayoutMigrationFixture();

    @Before
    public void setUp() {
        f.reset();
    }

    /**
     * The classic controller's default layout on this 1600x720 screen, as it saved it: its 128x72 grid scaled to the
     * height (ten pixels a cell) and the right-hand controls moved by the 320 pixels this screen is wider than 16:9.
     */
    private static Map<Integer, float[]> classicDefault() {
        Map<Integer, float[]> boxes = new HashMap<>();
        boxes.put(1, new float[] {40, 410, 300, 300});    // D-pad
        boxes.put(6, new float[] {1380, 210, 100, 100});  // A, bottom
        boxes.put(7, new float[] {1480, 110, 100, 100});  // B, right
        boxes.put(8, new float[] {1280, 110, 100, 100});  // X, left
        boxes.put(9, new float[] {1380, 10, 100, 100});   // Y, top
        boxes.put(2, new float[] {10, 310, 120, 90});     // LT
        boxes.put(3, new float[] {1470, 310, 120, 90});   // RT
        boxes.put(4, new float[] {240, 310, 120, 90});    // LB
        boxes.put(5, new float[] {1240, 310, 120, 90});   // RB
        boxes.put(12, new float[] {60, 40, 260, 260});    // left stick
        boxes.put(13, new float[] {1300, 420, 260, 260}); // right stick
        boxes.put(10, new float[] {340, 640, 120, 70});   // Back
        boxes.put(11, new float[] {1150, 640, 120, 70});  // Start
        boxes.put(16, new float[] {810, 640, 120, 70});   // Guide
        return boxes;
    }

    @Test public void theClassicDefaultLayoutBecomesTheClusterWhereItsButtonsWere() {
        Map<Integer, float[]> classic = classicDefault();
        PadLayout layout = ClassicLayoutMigration.migrate(classic, WIDTH, HEIGHT, DENSITY);
        // Its four face buttons still stand in their diamond: one cluster over the box they span.
        assertTrue(layout.shown(PadLayout.Control.FACE));
        assertFalse(layout.isSplit(PadLayout.Control.FACE));
        assertBox("face", new float[] {1280, 10, 300, 300}, box(layout, PadLayout.Control.FACE));
        assertBox("D-pad", classic.get(1), box(layout, PadLayout.Control.DPAD));
        assertBox("left stick", classic.get(12), box(layout, PadLayout.Control.LEFT_STICK));
        assertBox("right stick", classic.get(13), box(layout, PadLayout.Control.RIGHT_STICK));
        // The small classic buttons grow around their place to this controller's full touch (48 dp, 72 dp pills),
        // kept on the screen: each new box holds the classic one.
        for (int id : new int[] {2, 3, 4, 5, 10, 11, 16}) {
            PadLayout.Control control = ClassicLayoutMigration.control(id);
            float[] was = classic.get(id), now = box(layout, control);
            assertTrue(control.name(), now[0] <= was[0] + .5f && now[1] <= was[1] + .5f);
            assertTrue(control.name(), now[0] + now[2] >= was[0] + was[2] - .5f);
            assertTrue(control.name(), now[1] + now[3] >= was[1] + was[3] - .5f);
            assertTrue(control.name(), now[2] < was[2] * 1.5f);
        }
        assertFalse("a control the default leaves out is shown", layout.spec(PadLayout.Control.HOME).hidden);
        // The pointer touchpad did not exist then and keeps the default (hidden until added).
        assertTrue(layout.spec(PadLayout.Control.POINTER_PAD).hidden);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            assertEquals(control.name(), PadLayout.MAX_OPACITY, layout.spec(control).opacity);
        }
    }

    @Test public void eachClassicBoxKeepsItsOwnWidthAndHeight() {
        Map<Integer, float[]> boxes = new HashMap<>();
        boxes.put(12, new float[] {0, 120, 700, 600});    // left stick, wider than tall
        boxes.put(1, new float[] {720, 500, 400, 160});   // D-pad, flat
        boxes.put(17, new float[] {700, 20, 200, 300});   // Touchpad button, taller than wide
        PadLayout layout = ClassicLayoutMigration.migrate(boxes, WIDTH, HEIGHT, DENSITY);
        for (Map.Entry<Integer, float[]> entry : boxes.entrySet()) {
            PadLayout.Control control = ClassicLayoutMigration.control(entry.getKey());
            assertBox(control.name(), entry.getValue(), box(layout, control));
        }
        PadLayout.Spec stick = layout.spec(PadLayout.Control.LEFT_STICK);
        assertNotEquals(stick.widthScale, stick.heightScale, .01f);
        // A box past the screen's edge or size comes back onto it, at most the whole screen.
        boxes.clear();
        boxes.put(13, new float[] {-100, 0, 2000, 900});
        assertBox("whole screen", new float[] {0, 0, WIDTH, HEIGHT},
                box(ClassicLayoutMigration.migrate(boxes, WIDTH, HEIGHT, DENSITY), PadLayout.Control.RIGHT_STICK));
    }

    @Test public void eachControlKeepsItsOwnOpacity() {
        Map<Integer, float[]> boxes = classicDefault();
        boxes.put(12, new float[] {60, 40, 260, 260, 40});
        boxes.put(1, new float[] {40, 410, 300, 300, 5});
        PadLayout layout = ClassicLayoutMigration.migrate(boxes, WIDTH, HEIGHT, DENSITY);
        assertEquals(40, layout.spec(PadLayout.Control.LEFT_STICK).opacity);
        assertEquals("never below this controller's floor", PadLayout.MIN_OPACITY,
                layout.spec(PadLayout.Control.DPAD).opacity);
        assertEquals("saved before opacity: opaque", PadLayout.MAX_OPACITY,
                layout.spec(PadLayout.Control.RIGHT_STICK).opacity);

        // The saved preferences carry the opacity too.
        f.classic.edit().putString("12", "{\"LEFT\":60,\"TOP\":40,\"WIDTH\":260,\"HEIGHT\":260,\"OPACITY\":35}")
                .putString("13", RIGHT_STICK).commit();
        Map<Integer, float[]> saved = ClassicLayoutMigration.boxes(f.classic);
        assertEquals(35, saved.get(12)[ClassicLayoutMigration.OPACITY], 0);
        assertEquals(100, saved.get(13)[ClassicLayoutMigration.OPACITY], 0);
    }

    @Test public void faceButtonsMovedApartBecomeTheClustersMembersWhereThePlayerPutThem() {
        Map<Integer, float[]> boxes = classicDefault();
        boxes.put(6, new float[] {1400, 400, 80, 80});     // A, moved down beside the right stick
        boxes.put(7, new float[] {1480, 400, 80, 80, 60}); // B, next to it and fainter
        PadLayout layout = ClassicLayoutMigration.migrate(boxes, WIDTH, HEIGHT, DENSITY);
        assertTrue(layout.isSplit(PadLayout.Control.FACE));
        assertFalse(layout.shown(PadLayout.Control.FACE));
        int[] ids = {6, 7, 8, 9};
        for (int i = 0; i < ids.length; i++) {
            assertTrue(MEMBERS[i].name(), layout.shown(MEMBERS[i]));
            assertCenter(MEMBERS[i].name(), boxes.get(ids[i]), box(layout, MEMBERS[i]));
        }
        assertEquals(60, layout.spec(PadLayout.Control.B).opacity);
        assertEquals(PadLayout.MAX_OPACITY, layout.spec(PadLayout.Control.A).opacity);

        // In their diamond but at different opacities they stay four, each as faint as it was.
        boxes = classicDefault();
        boxes.put(9, new float[] {1380, 10, 100, 100, 50});
        layout = ClassicLayoutMigration.migrate(boxes, WIDTH, HEIGHT, DENSITY);
        assertTrue(layout.isSplit(PadLayout.Control.FACE));
        assertEquals(50, layout.spec(PadLayout.Control.Y).opacity);

        // A button the classic layout did not save shows over its place in the default cluster.
        boxes = classicDefault();
        boxes.remove(9);
        layout = ClassicLayoutMigration.migrate(boxes, WIDTH, HEIGHT, DENSITY);
        assertTrue(layout.shown(PadLayout.Control.Y));
        assertFalse(layout.spec(PadLayout.Control.Y).placed());
        assertTrue(layout.spec(PadLayout.Control.A).placed());
    }

    @Test public void theDiamondAllowsAQuarterButtonAndNoMore() {
        float[][] diamond = {{1380, 210, 100, 100}, {1480, 110, 100, 100}, {1280, 110, 100, 100}, {1380, 10, 100, 100}};
        assertTrue(ClassicLayoutMigration.classicDiamond(diamond));
        // The classic grid's rounding and a nudge stay the cluster.
        diamond[1] = new float[] {1490, 115, 100, 100};
        assertTrue(ClassicLayoutMigration.classicDiamond(diamond));
        // Half a button off, a button resized apart, or a row of four does not.
        diamond[1] = new float[] {1530, 110, 100, 100};
        assertFalse(ClassicLayoutMigration.classicDiamond(diamond));
        diamond[1] = new float[] {1480, 110, 160, 100};
        assertFalse(ClassicLayoutMigration.classicDiamond(diamond));
        float[][] row = {{1000, 300, 100, 100}, {1100, 300, 100, 100}, {1200, 300, 100, 100}, {1300, 300, 100, 100}};
        assertFalse(ClassicLayoutMigration.classicDiamond(row));
    }

    @Test public void everyNintendoSwappedLayoutSavesItsArrangement() {
        assertTrue(ClassicLayoutMigration.migrate(classicDefault(), WIDTH, HEIGHT, DENSITY, true).facesSwapped());
        assertFalse(ClassicLayoutMigration.migrate(classicDefault(), WIDTH, HEIGHT, DENSITY, false).facesSwapped());
        assertTrue(ClassicLayoutMigration.swappedDefaults(WIDTH, HEIGHT, DENSITY).serialize().contains("|faces=swapped"));
        // Classic boxes without face buttons leave the cluster whole: in the Nintendo arrangement, B at the bottom.
        Map<Integer, float[]> noFaces = new HashMap<>();
        noFaces.put(12, new float[] {60, 40, 260, 260});
        PadLayout whole = ClassicLayoutMigration.migrate(noFaces, WIDTH, HEIGHT, DENSITY, true);
        assertTrue(whole.shown(PadLayout.Control.FACE));
        assertTrue(whole.facesSwapped());
        // A layout for one game, whole or split, swapped by the carry-over; once is enough.
        PadLayout game = PadLayout.parse("v1|FACE,0.6000,0.5000,1.000,100,0");
        assertTrue(ClassicLayoutMigration.swapFaceButtons(game, WIDTH, HEIGHT, DENSITY));
        assertTrue(game.facesSwapped());
        assertFalse("already swapped", ClassicLayoutMigration.swapFaceButtons(game, WIDTH, HEIGHT, DENSITY));
        PadLayout split = new PadLayout();
        split.split(PadLayout.Control.FACE, WIDTH, HEIGHT, DENSITY);
        assertTrue(ClassicLayoutMigration.swapFaceButtons(split, WIDTH, HEIGHT, DENSITY));
        assertTrue(split.facesSwapped());
        // Joined, a preset applied, the bottom still sends B.
        split.applyPreset(PadLayout.Preset.DEFAULT);
        float[] face = box(split, PadLayout.Control.FACE);
        assertEquals(ControllerPacket.B_FLAG, PadInput.face(face[2] / 2, face[3] * .78f, face[2], face[3],
                split.facesSwapped()));
    }

    @Test public void withTheNintendoLayoutEachPlaceKeepsTheButtonItSent() {
        // Classic sent B from the bottom button, A from the right one, Y from the left and X from the top.
        PadLayout layout = ClassicLayoutMigration.migrate(classicDefault(), WIDTH, HEIGHT, DENSITY, true);
        assertTrue(layout.isSplit(PadLayout.Control.FACE));
        Map<Integer, float[]> classic = classicDefault();
        assertCenter("bottom sends B", classic.get(6), box(layout, PadLayout.Control.B));
        assertCenter("right sends A", classic.get(7), box(layout, PadLayout.Control.A));
        assertCenter("left sends Y", classic.get(8), box(layout, PadLayout.Control.Y));
        assertCenter("top sends X", classic.get(9), box(layout, PadLayout.Control.X));
        // The glyphs stay Xbox, so each button still shows the letter it sends, as on the classic controller.
        assertEquals(PadLayout.Glyph.XBOX, layout.glyph());
    }

    /** The reported layout: controls stretched over the screen at opacity 0, so a stray touch never reaches the stream. */
    @Test public void aFullScreenClassicLayoutAtOpacityZeroComesOutTheSame() {
        int[][] classicBoxes = {
                {1, 0, 0, 640, 240},       // D-pad over the top left
                {12, 0, 240, 640, 480},    // left stick under it
                {10, 640, 0, 160, 720},    // Back and Start, the middle column
                {11, 800, 0, 160, 720},
                {8, 960, 0, 320, 180},     // X, Y, A and B as a grid over the top right
                {9, 1280, 0, 320, 180},
                {6, 960, 180, 320, 180},
                {7, 1280, 180, 320, 180},
                {13, 960, 360, 640, 360},  // right stick under them
        };
        for (int[] b : classicBoxes) f.putClassic(b[0], b[1], b[2], b[3], b[4], 10);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.oscOpacity = 0;
        ModernController controller = f.controller(f.frame(), prefs);
        assertTrue(controller.layout().isSplit(PadLayout.Control.FACE));
        for (int[] b : classicBoxes) {
            PadLayout.Control control = ClassicLayoutMigration.control(b[0]);
            if (control == null) control = MEMBERS[b[0] - 6];
            View view = controller.view(control);
            assertEquals(control.name(), View.VISIBLE, view.getVisibility());
            assertBox(control.name(), new float[] {b[1], b[2], b[3], b[4]},
                    new float[] {view.getLeft(), view.getTop(), view.getWidth(), view.getHeight()});
            assertEquals(control.name(), 10, controller.layout().spec(control).opacity);
            assertEquals(control.name(), 0f, view.getAlpha(), 0);
        }
        controller.destroy();
    }

    @Test public void aMigratedNintendoLayoutSendsFromEachPlaceWhatTheClassicButtonSent() {
        for (Map.Entry<Integer, float[]> entry : classicDefault().entrySet()) {
            float[] b = entry.getValue();
            f.putClassic(entry.getKey(), (int) b[0], (int) b[1], (int) b[2], (int) b[3], 100);
        }
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.flipFaceButtons = true;
        ModernController controller = f.controller(f.frame(), prefs);
        // The classic bottom button (1380, 210, 100, 100) sent B: its place still does.
        PadControlView bottom = controller.view(PadLayout.Control.B);
        assertEquals(1380 + 50, bottom.getLeft() + bottom.getWidth() / 2, 2);
        assertEquals(210 + 50, bottom.getTop() + bottom.getHeight() / 2, 2);
        bottom.down(0, bottom.getWidth() / 2f, bottom.getHeight() / 2f, 100);
        assertEquals(ControllerPacket.B_FLAG, (int) f.buttons.get(f.buttons.size() - 1));
        bottom.up(0, 200);
        assertEquals(0, (int) f.buttons.get(f.buttons.size() - 1));
        controller.destroy();

        // Without the setting the same classic layout is the plain cluster: its bottom sends A.
        f.store.edit().clear().commit();
        prefs.flipFaceButtons = false;
        controller = f.controller(f.frame(), prefs);
        assertFalse(controller.layout().isSplit(PadLayout.Control.FACE));
        controller.destroy();
    }

    @Test public void anEmptyOrBrokenClassicLayoutLeavesTheModernDefaultsAlone() {
        assertTrue(ClassicLayoutMigration.migrate(null, WIDTH, HEIGHT, DENSITY).isDefault());
        assertTrue(ClassicLayoutMigration.migrate(new HashMap<>(), WIDTH, HEIGHT, DENSITY).isDefault());
        Map<Integer, float[]> broken = new HashMap<>();
        broken.put(12, null);
        broken.put(13, new float[] {0, 0, 0, 0});
        broken.put(999, new float[] {10, 10, 10, 10});
        assertTrue(ClassicLayoutMigration.migrate(broken, WIDTH, HEIGHT, DENSITY).isDefault());
        assertTrue(ClassicLayoutMigration.migrate(classicDefault(), 0, 0, DENSITY).isDefault());
    }

    @Test public void eachClassicElementMapsToTheControlItWas() {
        assertEquals(PadLayout.Control.LEFT_STICK, ClassicLayoutMigration.control(12));
        assertEquals(PadLayout.Control.RIGHT_STICK, ClassicLayoutMigration.control(13));
        assertEquals(PadLayout.Control.L3, ClassicLayoutMigration.control(14));
        assertEquals(PadLayout.Control.R3, ClassicLayoutMigration.control(15));
        assertEquals(PadLayout.Control.SELECT, ClassicLayoutMigration.control(10));
        assertEquals(PadLayout.Control.START, ClassicLayoutMigration.control(11));
        assertEquals(PadLayout.Control.TOUCHPAD, ClassicLayoutMigration.control(17));
        assertEquals(PadLayout.Control.LEFT_PAD, ClassicLayoutMigration.control(22));
        assertEquals(PadLayout.Control.RIGHT_PAD, ClassicLayoutMigration.control(23));
        assertNull("The face buttons become the cluster or its members", ClassicLayoutMigration.control(6));
        assertNull(ClassicLayoutMigration.control(999));
    }

    /** The device report: Join and Split in the editor on a carried-over Nintendo layout keep its place and buttons. */
    @Test public void joinAndSplitKeepACarriedOverNintendoLayoutsPlaceAndButtons() {
        // The Retroid's layout after the upgrade: the classic buttons, Nintendo-swapped, where the player had them,
        // and the face cluster never placed.
        String device = DEVICE;
        f.store.edit().putBoolean(ClassicLayoutMigration.MIGRATED, true).putBoolean(ClassicLayoutMigration.STYLE_RETIRED, true)
                .putString(PadLayoutKeys.key(true, null), device).putString(PadLayoutKeys.key(false, null), device).commit();
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.flipFaceButtons = true;
        FrameLayout frame = f.frame();
        ModernController controller = f.controller(frame, prefs);
        PadLayout carried = new PadLayout();
        carried.copyFrom(controller.layout());
        float left = Float.MAX_VALUE, top = Float.MAX_VALUE, right = -Float.MAX_VALUE, bottom = -Float.MAX_VALUE;
        for (PadLayout.Control member : MEMBERS) {
            float[] b = box(carried, member);
            left = Math.min(left, b[0]);
            top = Math.min(top, b[1]);
            right = Math.max(right, b[0] + b[2]);
            bottom = Math.max(bottom, b[1] + b[3]);
        }

        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.select(PadLayout.Control.B);
        editor.join();
        controller.finishEditing(true);
        layout(frame);
        PadControlView face = controller.view(PadLayout.Control.FACE);
        // Each place of the joined cluster stands where the member sending that button stood: B at the bottom, A on
        // the right, Y on the left, X at the top. A stands at the screen's right edge and X at its top, where the
        // cluster's larger buttons have no room around them, so the cluster narrows to fit by a few pixels.
        PadLayout.Control[] atPlace = {PadLayout.Control.B, PadLayout.Control.A, PadLayout.Control.Y,
                PadLayout.Control.X};
        float[][] places = {{.5f, .78f}, {.78f, .5f}, {.22f, .5f}, {.5f, .22f}};
        for (int p = 0; p < places.length; p++) {
            float[] member = box(carried, atPlace[p]);
            float x = face.getLeft() + places[p][0] * face.getWidth(), y = face.getTop() + places[p][1] * face.getHeight();
            assertEquals(atPlace[p] + " x", member[0] + member[2] / 2, x, 11);
            assertEquals(atPlace[p] + " y", member[1] + member[3] / 2, y, 11);
        }
        assertTrue("on the screen", face.getLeft() >= 0 && face.getRight() <= WIDTH && face.getTop() >= 0);
        face.down(0, face.getWidth() / 2f, face.getHeight() * .78f, 100);
        assertEquals("the bottom still sends B", ControllerPacket.B_FLAG, (int) f.buttons.get(f.buttons.size() - 1));
        face.up(0, 200);
        face.down(0, face.getWidth() * .78f, face.getHeight() / 2f, 300);
        assertEquals("the right A", ControllerPacket.A_FLAG, (int) f.buttons.get(f.buttons.size() - 1));
        face.up(0, 400);

        controller.startEditing();
        editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        editor.select(PadLayout.Control.FACE);
        editor.split();
        controller.finishEditing(true);
        layout(frame);
        PadControlView b = controller.view(PadLayout.Control.B), a = controller.view(PadLayout.Control.A);
        assertTrue("B at the bottom again", b.getTop() > a.getTop());
        assertTrue("around where the cluster was", b.getLeft() + b.getWidth() / 2f > left && b.getTop() < bottom);
        b.down(0, b.getWidth() / 2f, b.getHeight() / 2f, 500);
        assertEquals(ControllerPacket.B_FLAG, (int) f.buttons.get(f.buttons.size() - 1));
        b.up(0, 600);
        controller.destroy();
    }

    /** All four members hidden, the face cluster added back: in the arrangement they stood in, B at the bottom. */
    @Test public void theFaceClusterAddedBackAfterItsMembersKeepsTheirButtons() {
        f.store.edit().putBoolean(ClassicLayoutMigration.MIGRATED, true).putBoolean(ClassicLayoutMigration.STYLE_RETIRED, true)
                .putString(PadLayoutKeys.key(true, null), DEVICE).putString(PadLayoutKeys.key(false, null), DEVICE).commit();
        FrameLayout frame = f.frame();
        ModernController controller = f.controller(frame, new PreferenceConfiguration());
        assertFalse("carried over before the arrangement was saved", controller.layout().facesSwapped());
        controller.startEditing();
        PadEditor editor = (PadEditor) frame.getChildAt(frame.getChildCount() - 1);
        for (PadLayout.Control member : MEMBERS) {
            editor.select(member);
            editor.toggleHidden();
        }
        editor.add(PadLayout.Control.FACE);
        assertTrue(editor.draft().facesSwapped());
        controller.finishEditing(true);
        layout(frame);
        PadControlView face = controller.view(PadLayout.Control.FACE);
        face.down(0, face.getWidth() / 2f, face.getHeight() * .78f, 100);
        assertEquals(ControllerPacket.B_FLAG, (int) f.buttons.get(f.buttons.size() - 1));
        face.up(0, 200);
        controller.destroy();
    }
}
