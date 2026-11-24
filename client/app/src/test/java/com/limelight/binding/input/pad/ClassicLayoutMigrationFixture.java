package com.limelight.binding.input.pad;

import android.content.Context;
import android.content.SharedPreferences;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.UiNavigationTest;

import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/**
 * Shared Robolectric fixture and assertions for {@link ClassicLayoutMigrationTest} and
 * {@link ClassicLayoutMigrationGeometryTest}.
 */
final class ClassicLayoutMigrationFixture {
    static final float WIDTH = 1600, HEIGHT = 720, DENSITY = 2;
    /** The classic right stick, at (1300, 500) on this 1600x720 screen. */
    static final String RIGHT_STICK = "{\"LEFT\":1200,\"TOP\":400,\"WIDTH\":200,\"HEIGHT\":200}";
    static final PadLayout.Control[] MEMBERS = PadLayout.members(PadLayout.Control.FACE);

    Context context;
    SharedPreferences store, classic;
    final List<Integer> buttons = new ArrayList<>();

    void reset() {
        context = RuntimeEnvironment.getApplication();
        store = context.getSharedPreferences(ModernController.PREFS, Context.MODE_PRIVATE);
        classic = context.getSharedPreferences(ClassicLayoutMigration.CLASSIC_PREFS, Context.MODE_PRIVATE);
        store.edit().clear().commit();
        classic.edit().clear().commit();
        buttons.clear();
    }

    PadLayout shared() {
        return PadLayout.parse(store.getString(PadLayoutKeys.legacyKey(null), null));
    }

    void retire(String style) {
        retire(style, false);
    }

    /** The first reading of the settings by this version, with this stored style (null for none) and Nintendo layout. */
    void retire(String style, boolean flip) {
        SharedPreferences.Editor edit = android.preference.PreferenceManager.getDefaultSharedPreferences(context).edit()
                .clear().putBoolean("checkbox_flip_face_buttons", flip);
        if (style != null) edit.putString("list_osc_style", style);
        edit.commit();
        PreferenceConfiguration.readPreferences(context);
    }

    /** A classic player's layout for all games in one orientation. */
    PadLayout general(boolean landscape) {
        return PadLayout.parse(store.getString(PadLayoutKeys.key(landscape, null), null));
    }

    String backup(String key) {
        return store.getString(key + ClassicLayoutMigration.BACKUP_SUFFIX, null);
    }

    void assertRightStickAt1300x500(PadLayout layout) {
        PadLayout.Spec stick = layout.spec(PadLayout.Control.RIGHT_STICK);
        assertEquals(1300f / WIDTH, stick.cx, .01f);
        assertEquals(500f / HEIGHT, stick.cy, .01f);
    }

    /**
     * Each orientation's layout for all games has the face cluster split where that orientation's defaults put it,
     * swapped: in landscape on this 1600x720 screen, in portrait on 720x1600.
     */
    void assertSwappedDefaults() {
        assertFalse("nothing for both orientations under the shared key", store.contains(PadLayoutKeys.legacyKey(null)));
        assertSwappedDefaults(PadLayout.parse(store.getString(PadLayoutKeys.key(true, null), null)), WIDTH, HEIGHT);
        assertSwappedDefaults(PadLayout.parse(store.getString(PadLayoutKeys.key(false, null), null)), HEIGHT, WIDTH);
    }

    static void assertSwappedDefaults(PadLayout layout, float width, float height) {
        assertTrue(layout.isSplit(PadLayout.Control.FACE));
        PadLayout plain = new PadLayout();
        plain.split(PadLayout.Control.FACE, width, height, DENSITY);
        String where = width > height ? "landscape: " : "portrait: ";
        assertBox(where + "bottom sends B", box(plain, PadLayout.Control.A, width, height),
                box(layout, PadLayout.Control.B, width, height));
        assertBox(where + "right sends A", box(plain, PadLayout.Control.B, width, height),
                box(layout, PadLayout.Control.A, width, height));
        assertBox(where + "left sends Y", box(plain, PadLayout.Control.X, width, height),
                box(layout, PadLayout.Control.Y, width, height));
        assertBox(where + "top sends X", box(plain, PadLayout.Control.Y, width, height),
                box(layout, PadLayout.Control.X, width, height));
        assertEquals(PadLayout.Glyph.XBOX, layout.glyph());
    }

    static float[] box(PadLayout layout, PadLayout.Control control) {
        return box(layout, control, WIDTH, HEIGHT);
    }

    static float[] box(PadLayout layout, PadLayout.Control control, float width, float height) {
        return layout.bounds(control, width, height, DENSITY, new float[4]);
    }

    static void assertBox(String what, float[] expected, float[] actual) {
        for (int i = 0; i < 4; i++) assertEquals(what + " [" + i + "]", expected[i], actual[i], 1.5f);
    }

    static void assertCenter(String what, float[] classic, float[] actual) {
        assertEquals(what, classic[0] + classic[2] / 2, actual[0] + actual[2] / 2, 1.5f);
        assertEquals(what, classic[1] + classic[3] / 2, actual[1] + actual[3] / 2, 1.5f);
    }

    FrameLayout frame() {
        UiNavigationTest.Screen screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible().get();
        FrameLayout frame = new FrameLayout(screen);
        screen.setContentView(frame, new FrameLayout.LayoutParams((int) WIDTH, (int) HEIGHT));
        layout(frame);
        return frame;
    }

    static void layout(FrameLayout frame) {
        frame.measure(View.MeasureSpec.makeMeasureSpec((int) WIDTH, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec((int) HEIGHT, View.MeasureSpec.EXACTLY));
        frame.layout(0, 0, (int) WIDTH, (int) HEIGHT);
    }

    ModernController controller(FrameLayout frame, PreferenceConfiguration prefs) {
        return controller(frame, prefs, null);
    }

    ModernController controller(FrameLayout frame, PreferenceConfiguration prefs, String gameKey) {
        ModernController controller = new ModernController(frame.getContext(), frame, new HeldInputState.Sink() {
            @Override public void key(int key, boolean down) { }
            @Override public void mouse(int button, boolean down) { }
            @Override public void gamepad(int b, short lx, short ly, short rx, short ry, byte lt, byte rt) {
                buttons.add(b);
            }
        }, prefs, gameKey);
        controller.refreshLayout();
        controller.show();
        layout(frame);
        return controller;
    }

    void putClassic(int id, int left, int top, int width, int height, int opacity) {
        classic.edit().putString(String.valueOf(id), "{\"LEFT\":" + left + ",\"TOP\":" + top + ",\"WIDTH\":" + width +
                ",\"HEIGHT\":" + height + ",\"OPACITY\":" + opacity + "}").commit();
    }
}
