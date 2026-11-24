package com.limelight.binding.input.pad;

import android.app.Application;

import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.Map;

import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.DENSITY;
import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.HEIGHT;
import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.RIGHT_STICK;
import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.WIDTH;
import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.assertBox;
import static com.limelight.binding.input.pad.ClassicLayoutMigrationFixture.box;
import static org.junit.Assert.*;

/**
 * The classic controller's saved boxes become this controller's layout once per install, when it is first built
 * after the update that removed the classic controller.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class, qualifiers = "w800dp-h360dp-land-xhdpi",
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class)
public class ClassicLayoutMigrationTest {
    /** An older build's carry-over, from the first time the player tried this controller: the stick at the left. */
    private static final String EARLIER = "v1|RIGHT_STICK,0.1000,0.5000,1.000,100,0";
    private static final String GAME = "pc-1/app-7";

    private final ClassicLayoutMigrationFixture f = new ClassicLayoutMigrationFixture();

    @Before
    public void setUp() {
        f.reset();
    }

    @Test public void theClassicLayoutBecomesTheLayoutForAllGamesOnce() {
        f.classic.edit().putString("13", RIGHT_STICK).putString("16", "{\"LEFT\":760,\"TOP\":100,\"WIDTH\":80,\"HEIGHT\":80}")
                .commit();
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        assertTrue(f.store.getBoolean(ClassicLayoutMigration.MIGRATED, false));
        // The older shared key is what both orientations fall back to, for every game without its own layout.
        PadLayout.Spec stick = f.shared().spec(PadLayout.Control.RIGHT_STICK);
        assertTrue(stick.placed());
        assertEquals(1300f / WIDTH, stick.cx, .01f);
        assertEquals(500f / HEIGHT, stick.cy, .01f);
        assertFalse(f.shared().spec(PadLayout.Control.HOME).hidden);

        // Exactly once: a later run carries nothing over, whatever the classic preferences hold by then.
        f.classic.edit().putString("13", "{\"LEFT\":0,\"TOP\":0,\"WIDTH\":200,\"HEIGHT\":200}").commit();
        f.store.edit().remove(PadLayoutKeys.legacyKey(null)).commit();
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        assertFalse(f.store.contains(PadLayoutKeys.legacyKey(null)));
    }

    @Test public void withNothingClassicSavedItIsDoneAndWritesNothing() {
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, true);
        assertTrue(f.store.getBoolean(ClassicLayoutMigration.MIGRATED, false));
        assertFalse(f.store.contains(PadLayoutKeys.legacyKey(null)));
        // A classic layout that appears later is never read.
        f.classic.edit().putString("13", RIGHT_STICK).commit();
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        assertFalse(f.store.contains(PadLayoutKeys.legacyKey(null)));
    }

    @Test public void aLayoutForAllGamesArrangedInThisControllerWins() {
        f.classic.edit().putString("13", RIGHT_STICK).commit();
        String mine = "v1|FACE,0.7083,0.5556,1.300,100,0";
        f.store.edit().putString(PadLayoutKeys.legacyKey(null), mine).commit();
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        assertEquals(mine, f.store.getString(PadLayoutKeys.legacyKey(null), null));
        assertTrue(f.store.getBoolean(ClassicLayoutMigration.MIGRATED, false));

        // Both orientations with a layout of their own leave nothing for the classic one to fill.
        f.store.edit().clear().putString(PadLayoutKeys.key(true, null), mine).putString(PadLayoutKeys.key(false, null), mine)
                .commit();
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        assertFalse(f.store.contains(PadLayoutKeys.legacyKey(null)));
        assertEquals(mine, f.store.getString(PadLayoutKeys.key(true, null), null));
        assertEquals(mine, f.store.getString(PadLayoutKeys.key(false, null), null));
    }

    @Test public void theClassicLayoutFillsOnlyWhatHasNoLayoutOfItsOwn() {
        f.classic.edit().putString("13", RIGHT_STICK).commit();
        String landscape = "v1|FACE,0.7083,0.5556,1.300,100,0", game = "v1|DPAD,0.2,0.6,1.000,100,0";
        f.store.edit().putString(PadLayoutKeys.key(true, null), landscape)
                .putString(PadLayoutKeys.key(true, "pc-1/app-7"), game).commit();
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        // Portrait had no layout for all games; landscape and the game keep theirs, and they win where they exist.
        assertTrue(f.shared().spec(PadLayout.Control.RIGHT_STICK).placed());
        assertEquals(landscape, f.store.getString(PadLayoutKeys.key(true, null), null));
        assertEquals(game, f.store.getString(PadLayoutKeys.key(true, "pc-1/app-7"), null));
    }

    @Test public void aPlayerWhoPickedThisControllerOrResetTheLayoutGetsNothingClassic() {
        f.classic.edit().putString("13", RIGHT_STICK).commit();
        ClassicLayoutMigration.retireStyle(f.context, true, false);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        assertFalse(f.store.contains(PadLayoutKeys.legacyKey(null)));

        // Reset sets the same flag, so the defaults stay defaults.
        f.store.edit().clear().commit();
        ModernController.clearLayouts(f.context);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        assertFalse(f.store.contains(PadLayoutKeys.legacyKey(null)));
        assertTrue(f.store.getBoolean(ClassicLayoutMigration.MIGRATED, false));
    }

    @Test public void aCorruptClassicControlIsLeftOut() {
        f.classic.edit().putString("12", "not json").putString("1", "{\"LEFT\":5}").putInt("14", 3)
                .putString("13", RIGHT_STICK).commit();
        Map<Integer, float[]> boxes = ClassicLayoutMigration.boxes(f.classic);
        assertEquals(1, boxes.size());
        assertArrayEquals(new float[] {1200, 400, 200, 200, 100}, boxes.get(13), 0);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        assertFalse(f.shared().spec(PadLayout.Control.LEFT_STICK).placed());
        assertTrue(f.shared().spec(PadLayout.Control.RIGHT_STICK).placed());
    }

    @Test public void aPlayerWhoWentBackToTheClassicControllerGetsTheLayoutTheyKeptArranging() {
        // They tried this controller once, which carried the classic layout over and set the flag, then went back
        // to the classic controller and moved its right stick to (1300, 500).
        f.store.edit().putBoolean(ClassicLayoutMigration.MIGRATED, true)
                .putString(PadLayoutKeys.legacyKey(null), EARLIER).commit();
        f.classic.edit().putString("13", RIGHT_STICK).commit();
        f.retire("classic");
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        // The classic controller had one layout for both orientations.
        f.assertRightStickAt1300x500(f.general(true));
        f.assertRightStickAt1300x500(f.general(false));
        assertFalse(f.store.contains(PadLayoutKeys.legacyKey(null)));
        assertTrue(f.store.getBoolean(ClassicLayoutMigration.MIGRATED, false));
        assertFalse(f.store.contains(ClassicLayoutMigration.CLASSIC_STYLE));

        // Still only once: the classic layout moved later is never read.
        String carried = f.store.getString(PadLayoutKeys.key(true, null), null);
        f.classic.edit().putString("13", "{\"LEFT\":0,\"TOP\":0,\"WIDTH\":200,\"HEIGHT\":200}").commit();
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        assertEquals(carried, f.store.getString(PadLayoutKeys.key(true, null), null));
    }

    @Test public void aClassicPlayersLayoutsForAllGamesGiveWayToTheClassicArrangementAndAreKept() {
        // They tried this controller and saved layouts for all games in both orientations, then went back to the
        // classic controller and arranged it: the classic arrangement is the one they were using.
        String mine = "v1|FACE,0.7083,0.5556,1.300,100,0";
        f.store.edit().putBoolean(ClassicLayoutMigration.MIGRATED, true)
                .putString(PadLayoutKeys.legacyKey(null), EARLIER)
                .putString(PadLayoutKeys.key(true, null), mine).putString(PadLayoutKeys.key(false, null), mine).commit();
        f.classic.edit().putString("13", RIGHT_STICK).commit();
        f.retire("classic");
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        f.assertRightStickAt1300x500(f.general(true));
        f.assertRightStickAt1300x500(f.general(false));
        assertFalse(f.store.contains(PadLayoutKeys.legacyKey(null)));
        // Each one replaced is kept under its own backup key.
        assertEquals(mine, f.backup(PadLayoutKeys.key(true, null)));
        assertEquals(mine, f.backup(PadLayoutKeys.key(false, null)));
        assertEquals(EARLIER, f.store.getString(ClassicLayoutMigration.BACKUP, null));

        // A player who stayed with this controller keeps every layout as it was, and gets no backups.
        f.store.edit().clear().putBoolean(ClassicLayoutMigration.MIGRATED, true)
                .putString(PadLayoutKeys.legacyKey(null), EARLIER)
                .putString(PadLayoutKeys.key(true, null), mine).putString(PadLayoutKeys.key(false, null), mine).commit();
        f.retire("modern", true);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, true);
        assertEquals(EARLIER, f.store.getString(PadLayoutKeys.legacyKey(null), null));
        assertEquals(mine, f.store.getString(PadLayoutKeys.key(true, null), null));
        assertEquals(mine, f.store.getString(PadLayoutKeys.key(false, null), null));
        for (String key : f.store.getAll().keySet()) {
            assertFalse(key, key.endsWith(ClassicLayoutMigration.BACKUP_SUFFIX));
        }
    }

    @Test public void aClassicPlayerWithTheNintendoLayoutKeepsBAtTheBottomOfTheDefaults() {
        // Nothing classic was arranged: the classic defaults sent B from the bottom, A from the right, Y from the
        // left and X from the top.
        f.retire("classic", true);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, true);
        f.assertSwappedDefaults();
        assertFalse(f.store.contains(ClassicLayoutMigration.CLASSIC_STYLE));

        // An install that never stored a style had Classic, the default: the same.
        f.store.edit().clear().commit();
        f.retire(null, true);
        assertTrue(f.store.getBoolean(ClassicLayoutMigration.CLASSIC_STYLE, false));
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, true);
        f.assertSwappedDefaults();

        // The setting as it was when the classic controller went away decides, not a later change.
        f.store.edit().clear().commit();
        f.retire(null, true);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        f.assertSwappedDefaults();
    }

    @Test public void aFreshInstallKeepsThePlainDefaults() {
        // Its first reading of the settings comes before anyone can turn the Nintendo layout on.
        f.retire(null, false);
        assertTrue(f.store.getBoolean(ClassicLayoutMigration.STYLE_RETIRED, false));
        // Turned on later, it is read again but decides nothing more.
        android.preference.PreferenceManager.getDefaultSharedPreferences(f.context).edit()
                .putBoolean("checkbox_flip_face_buttons", true).commit();
        PreferenceConfiguration.readPreferences(f.context);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, true);
        assertFalse(f.store.contains(PadLayoutKeys.legacyKey(null)));
        assertTrue(f.store.getBoolean(ClassicLayoutMigration.MIGRATED, false));

        // A reset of the layouts keeps the style read, so the defaults stay plain.
        ModernController.clearLayouts(f.context);
        PreferenceConfiguration.readPreferences(f.context);
        assertFalse(f.store.contains(ClassicLayoutMigration.CLASSIC_STYLE));
        assertTrue(f.store.getBoolean(ClassicLayoutMigration.STYLE_RETIRED, false));
    }

    @Test public void eachOrientationGetsTheSwappedDefaultsOfItsOwnPlaces() {
        f.retire("classic", true);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, true);
        // In portrait the cluster stands elsewhere than the landscape fractions would put it.
        PadLayout portrait = PadLayout.parse(f.store.getString(PadLayoutKeys.key(false, null), null));
        PadLayout plain = new PadLayout();
        plain.split(PadLayout.Control.FACE, HEIGHT, WIDTH, DENSITY);
        assertBox("portrait: bottom sends B", box(plain, PadLayout.Control.A, HEIGHT, WIDTH),
                box(portrait, PadLayout.Control.B, HEIGHT, WIDTH));
        PadLayout landscapeOnly = new PadLayout();
        landscapeOnly.split(PadLayout.Control.FACE, WIDTH, HEIGHT, DENSITY);
        PadLayout.Spec landscapeA = landscapeOnly.spec(PadLayout.Control.A), portraitB = portrait.spec(PadLayout.Control.B);
        assertTrue("portrait has its own places",
                Math.abs(landscapeA.cx - portraitB.cx) > .01f || Math.abs(landscapeA.cy - portraitB.cy) > .01f);

        // A layout for all games the player saved in this controller gives way too, and is kept.
        String mine = "v1|FACE,0.5000,0.8000,1.000,100,0";
        f.store.edit().clear().putString(PadLayoutKeys.key(false, null), mine).commit();
        f.retire("classic", true);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, true);
        f.assertSwappedDefaults();
        assertEquals(mine, f.backup(PadLayoutKeys.key(false, null)));
        assertNull(f.backup(PadLayoutKeys.key(true, null)));
    }

    @Test public void aClassicPlayersLayoutsForOneGameKeepTheirPlacesWithTheButtonsSwapped() {
        // A whole cluster in the game's landscape layout, a split one in its older layout for both orientations,
        // hidden face buttons in its portrait layout.
        String whole = "v1|FACE,0.6000,0.5000,1.000,100,0";
        PadLayout split = new PadLayout();
        split.split(PadLayout.Control.FACE, WIDTH, HEIGHT, DENSITY);
        split.setBounds(PadLayout.Control.A, 100, 300, 120, 120, WIDTH, HEIGHT, DENSITY);
        split.setOpacity(PadLayout.Control.A, 40);
        String splitText = split.serialize();
        String hidden = "v1|FACE,-,-,1.000,100,1";
        f.store.edit().putString(PadLayoutKeys.key(true, GAME), whole).putString(PadLayoutKeys.legacyKey(GAME), splitText)
                .putString(PadLayoutKeys.key(false, GAME), hidden).commit();
        f.retire("classic", true);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, true);

        // The whole cluster is split where it stood, each place holding what the classic button there sent.
        PadLayout landscape = PadLayout.parse(f.store.getString(PadLayoutKeys.key(true, GAME), null));
        PadLayout before = PadLayout.parse(whole);
        before.split(PadLayout.Control.FACE, WIDTH, HEIGHT, DENSITY);
        assertTrue(landscape.isSplit(PadLayout.Control.FACE));
        assertBox("bottom sends B", box(before, PadLayout.Control.A), box(landscape, PadLayout.Control.B));
        assertBox("right sends A", box(before, PadLayout.Control.B), box(landscape, PadLayout.Control.A));
        assertBox("left sends Y", box(before, PadLayout.Control.X), box(landscape, PadLayout.Control.Y));
        assertBox("top sends X", box(before, PadLayout.Control.Y), box(landscape, PadLayout.Control.X));
        assertEquals(whole, f.backup(PadLayoutKeys.key(true, GAME)));
        // Split members trade their whole specs: place, size and opacity.
        PadLayout older = PadLayout.parse(f.store.getString(PadLayoutKeys.legacyKey(GAME), null));
        assertBox("B where A was", box(split, PadLayout.Control.A), box(older, PadLayout.Control.B));
        assertBox("A where B was", box(split, PadLayout.Control.B), box(older, PadLayout.Control.A));
        assertEquals(40, older.spec(PadLayout.Control.B).opacity);
        assertEquals(splitText, f.backup(PadLayoutKeys.legacyKey(GAME)));
        // Hidden face buttons have nothing to swap.
        assertEquals(hidden, f.store.getString(PadLayoutKeys.key(false, GAME), null));
        assertNull(f.backup(PadLayoutKeys.key(false, GAME)));

        // In the game, the bottom of where the cluster stood sends B.
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.flipFaceButtons = true;
        ModernController controller = f.controller(f.frame(), prefs, GAME);
        PadControlView bottom = controller.view(PadLayout.Control.B);
        float[] place = box(before, PadLayout.Control.A);
        assertEquals(place[0] + place[2] / 2, bottom.getLeft() + bottom.getWidth() / 2f, 2);
        assertEquals(place[1] + place[3] / 2, bottom.getTop() + bottom.getHeight() / 2f, 2);
        bottom.down(0, bottom.getWidth() / 2f, bottom.getHeight() / 2f, 100);
        assertEquals(ControllerPacket.B_FLAG, (int) f.buttons.get(f.buttons.size() - 1));
        bottom.up(0, 200);
        controller.destroy();
    }

    @Test public void aGamesOlderLayoutGivesPortraitItsOwnSwappedCopy() {
        // The older layout for one game is both orientations' layout for it; this game has no portrait one.
        String whole = "v1|FACE,0.6000,0.5000,1.000,100,0";
        f.store.edit().putString(PadLayoutKeys.legacyKey(GAME), whole).commit();
        f.retire("classic", true);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, true);
        PadLayout portrait = PadLayout.parse(f.store.getString(PadLayoutKeys.key(false, GAME), null));
        PadLayout before = PadLayout.parse(whole);
        before.split(PadLayout.Control.FACE, HEIGHT, WIDTH, DENSITY);
        assertTrue(portrait.isSplit(PadLayout.Control.FACE));
        assertBox("portrait: bottom sends B", box(before, PadLayout.Control.A, HEIGHT, WIDTH),
                box(portrait, PadLayout.Control.B, HEIGHT, WIDTH));
        assertBox("portrait: top sends X", box(before, PadLayout.Control.Y, HEIGHT, WIDTH),
                box(portrait, PadLayout.Control.X, HEIGHT, WIDTH));
        // Nothing was there before, so there is no backup; the older layout's own is kept.
        assertNull(f.backup(PadLayoutKeys.key(false, GAME)));
        assertEquals(whole, f.backup(PadLayoutKeys.legacyKey(GAME)));
        // Landscape still reads the older layout, swapped on landscape places.
        PadLayout landscape = PadLayout.parse(f.store.getString(PadLayoutKeys.legacyKey(GAME), null));
        PadLayout wide = PadLayout.parse(whole);
        wide.split(PadLayout.Control.FACE, WIDTH, HEIGHT, DENSITY);
        assertBox("landscape: bottom sends B", box(wide, PadLayout.Control.A), box(landscape, PadLayout.Control.B));
        assertFalse(f.store.contains(PadLayoutKeys.key(true, GAME)));
    }

    @Test public void withoutTheNintendoLayoutAClassicPlayerGetsTheClassicArrangementAndKeepsTheirGames() {
        String mine = "v1|FACE,0.7083,0.5556,1.300,100,0", game = "v1|FACE,0.6000,0.5000,1.000,100,0";
        f.store.edit().putString(PadLayoutKeys.key(true, null), mine).putString(PadLayoutKeys.key(true, GAME), game)
                .commit();
        f.classic.edit().putString("13", RIGHT_STICK).commit();
        f.retire("classic", false);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        f.assertRightStickAt1300x500(f.general(true));
        f.assertRightStickAt1300x500(f.general(false));
        assertFalse(f.general(true).spec(PadLayout.Control.FACE).placed());
        assertEquals(mine, f.backup(PadLayoutKeys.key(true, null)));
        assertEquals(game, f.store.getString(PadLayoutKeys.key(true, GAME), null));
        assertNull(f.backup(PadLayoutKeys.key(true, GAME)));

        // With nothing classic arranged, the plain defaults: the layouts for all games go, kept as backups.
        f.store.edit().clear().putString(PadLayoutKeys.key(true, null), mine).commit();
        f.classic.edit().clear().commit();
        f.retire("classic", false);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        assertFalse(f.store.contains(PadLayoutKeys.key(true, null)));
        assertEquals(mine, f.backup(PadLayoutKeys.key(true, null)));
    }

    @Test public void aClassicPlayerWithTheNintendoLayoutAndLayoutsForAllGamesGetsBAtTheBottom() {
        // Saved in both orientations, then only in landscape: either way the classic arrangement shows.
        String mine = "v1|FACE,0.7083,0.5556,1.300,100,0";
        for (boolean both : new boolean[] {true, false}) {
            f.store.edit().clear().putString(PadLayoutKeys.key(true, null), mine).commit();
            if (both) f.store.edit().putString(PadLayoutKeys.key(false, null), mine).commit();
            f.retire("classic", true);
            PreferenceConfiguration prefs = new PreferenceConfiguration();
            prefs.flipFaceButtons = true;
            f.buttons.clear();
            ModernController controller = f.controller(f.frame(), prefs);
            f.assertSwappedDefaults();
            assertEquals(mine, f.backup(PadLayoutKeys.key(true, null)));
            assertEquals(both ? mine : null, f.backup(PadLayoutKeys.key(false, null)));
            PadControlView bottom = controller.view(PadLayout.Control.B);
            PadLayout plain = new PadLayout();
            plain.split(PadLayout.Control.FACE, WIDTH, HEIGHT, DENSITY);
            float[] place = box(plain, PadLayout.Control.A);
            assertEquals(place[1] + place[3] / 2, bottom.getTop() + bottom.getHeight() / 2f, 2);
            bottom.down(0, bottom.getWidth() / 2f, bottom.getHeight() / 2f, 100);
            assertEquals(ControllerPacket.B_FLAG, (int) f.buttons.get(f.buttons.size() - 1));
            bottom.up(0, 200);
            controller.destroy();
        }
    }

    @Test public void theNintendoLayoutLeavesTheDefaultsOfEveryoneElseAlone() {
        // A player who had picked this controller: it never swapped its buttons on screen.
        f.retire("modern", true);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, true);
        assertFalse(f.store.contains(PadLayoutKeys.legacyKey(null)));
        // A classic player without the setting keeps the plain defaults.
        f.store.edit().clear().commit();
        f.retire("classic", false);
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        assertFalse(f.store.contains(PadLayoutKeys.legacyKey(null)));
    }

    @Test public void aMigratedNintendoDefaultSendsBFromTheBottom() {
        f.retire("classic", true);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.flipFaceButtons = true;
        ModernController controller = f.controller(f.frame(), prefs);
        PadControlView bottom = controller.view(PadLayout.Control.B);
        PadLayout plain = new PadLayout();
        plain.split(PadLayout.Control.FACE, WIDTH, HEIGHT, DENSITY);
        float[] place = box(plain, PadLayout.Control.A);
        assertEquals(place[0] + place[2] / 2, bottom.getLeft() + bottom.getWidth() / 2f, 2);
        assertEquals(place[1] + place[3] / 2, bottom.getTop() + bottom.getHeight() / 2f, 2);
        bottom.down(0, bottom.getWidth() / 2f, bottom.getHeight() / 2f, 100);
        assertEquals(ControllerPacket.B_FLAG, (int) f.buttons.get(f.buttons.size() - 1));
        bottom.up(0, 200);
        controller.destroy();
    }

    @Test public void aClassicPlayerWhoResetTheClassicLayoutGetsTheDefaults() {
        f.store.edit().putBoolean(ClassicLayoutMigration.MIGRATED, true)
                .putString(PadLayoutKeys.legacyKey(null), EARLIER).commit();
        f.retire("classic");
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        assertFalse(f.store.contains(PadLayoutKeys.legacyKey(null)));
        assertTrue(f.store.getBoolean(ClassicLayoutMigration.MIGRATED, false));
        // Dropped, not lost: it can be recovered by hand.
        assertEquals(EARLIER, f.store.getString(ClassicLayoutMigration.BACKUP, null));
    }

    @Test public void aLayoutTheOlderEditorSavedYieldsToTheClassicLayoutInUseAndIsKept() {
        // The editors from 5.4.1 up to 6.0 saved a layout for all games under the shared key, without the flag;
        // the player then went back to the classic controller and arranged it.
        String edited = "v1|FACE,0.2000,0.3000,1.300,100,0";
        f.store.edit().putString(PadLayoutKeys.legacyKey(null), edited).commit();
        f.classic.edit().putString("13", RIGHT_STICK).commit();
        f.retire("classic");
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        f.assertRightStickAt1300x500(f.general(true));
        assertFalse(f.general(true).spec(PadLayout.Control.FACE).placed());
        assertFalse(f.store.contains(PadLayoutKeys.legacyKey(null)));
        assertEquals(edited, f.store.getString(ClassicLayoutMigration.BACKUP, null));

        // A player who stayed with this controller keeps it where it is.
        f.store.edit().clear().putString(PadLayoutKeys.legacyKey(null), edited).commit();
        f.retire("modern");
        ClassicLayoutMigration.carryOverOnce(f.context, f.store, false);
        assertEquals(edited, f.store.getString(PadLayoutKeys.legacyKey(null), null));
        assertFalse(f.store.contains(ClassicLayoutMigration.BACKUP));
    }
}
