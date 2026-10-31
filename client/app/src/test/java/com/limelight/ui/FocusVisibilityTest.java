package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.drawable.Drawable;
import android.os.Build;
import android.preference.PreferenceManager;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.AbsListView;
import android.widget.EditText;
import android.widget.HorizontalScrollView;
import android.widget.ScrollView;

import androidx.core.graphics.ColorUtils;
import androidx.core.widget.NestedScrollView;

import com.limelight.PcView;
import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.StreamSettings;
import com.limelight.ui.ShellBar.Destination;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.List;

/**
 * Every stop of a D-pad walk shows where focus is. A focusable passes when focusing it changes how it draws,
 * or when it has a focus animator; the lists draw their focus in a selector, which is checked the same way.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@org.robolectric.annotation.GraphicsMode(org.robolectric.annotation.GraphicsMode.Mode.NATIVE)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class FocusVisibilityTest {
    private Context context;

    @Before
    public void setUp() {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        context.getSharedPreferences("FirstRun", 0).edit().putBoolean("pairing_guide_shown", true).commit();
    }

    private static Bitmap draw(View view) {
        int width = Math.max(view.getWidth(), 1);
        int height = Math.max(view.getHeight(), 1);
        Bitmap bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
        view.draw(new Canvas(bitmap));
        return bitmap;
    }

    private static void settle(View view) {
        view.jumpDrawablesToCurrentState();
        if (view.getStateListAnimator() != null) view.getStateListAnimator().jumpToCurrentState();
    }

    private static void size(View view) {
        if (view.getWidth() > 0 && view.getHeight() > 0) return;
        view.measure(View.MeasureSpec.makeMeasureSpec(400, View.MeasureSpec.AT_MOST),
                View.MeasureSpec.makeMeasureSpec(200, View.MeasureSpec.AT_MOST));
        view.layout(0, 0, Math.max(view.getMeasuredWidth(), 48), Math.max(view.getMeasuredHeight(), 48));
    }

    /** Whether focusing {@code view} is visible: a different drawing, or a focus animator. */
    private static String focusProblem(View view) {
        if (view instanceof AbsListView) return selectorProblem(((AbsListView) view).getSelector(), "list selector");
        size(view);
        if (view.hasFocus()) view.clearFocus();
        settle(view);
        Bitmap before = draw(view);
        // A D-pad walk is out of touch mode; the test does not enter it, so the view may take focus in it.
        view.setFocusableInTouchMode(true);
        if (!view.requestFocus() || !view.isFocused()) return "cannot take focus";
        settle(view);
        Bitmap after = draw(view);
        int ring = ringPixels(before, after, surfaceOf(view));
        if (ring >= Math.max(view.getWidth(), view.getHeight())) return null;
        // Robolectric does not draw a RippleDrawable's content layer; a ring inside one is checked as a selector.
        if (view.getBackground() instanceof android.graphics.drawable.RippleDrawable) {
            android.graphics.drawable.RippleDrawable ripple = (android.graphics.drawable.RippleDrawable) view.getBackground();
            for (int i = 0; i < ripple.getNumberOfLayers(); i++) {
                if (ripple.getId(i) == android.R.id.mask) continue;
                if (selectorProblem(ripple.getDrawable(i), "ripple content") == null) return null;
            }
        }
        // Scale animators do not show in view.draw: the animator itself must hold a focus state.
        if (view.getStateListAnimator() != null && view.getScaleX() != 1f) return null;
        return "draws no ring with focus (" + ring + " of " + view.getWidth() + "x" + view.getHeight() + ", scale "
                + view.getScaleX() + ")";
    }

    private static int surfaceOf(View view) {
        return com.google.android.material.color.MaterialColors.getColor(view.getContext(),
                com.google.android.material.R.attr.colorSurface, Color.BLACK);
    }

    private static String selectorProblem(Drawable selector, String what) {
        if (selector == null) return what + " is missing";
        selector = selector.getConstantState().newDrawable().mutate();
        selector.setBounds(0, 0, 200, 80);
        selector.setState(new int[] {android.R.attr.state_enabled});
        selector.jumpToCurrentState();
        Bitmap before = Bitmap.createBitmap(200, 80, Bitmap.Config.ARGB_8888);
        selector.draw(new Canvas(before));
        selector.setState(new int[] {android.R.attr.state_enabled, android.R.attr.state_focused});
        selector.jumpToCurrentState();
        Bitmap after = Bitmap.createBitmap(200, 80, Bitmap.Config.ARGB_8888);
        selector.draw(new Canvas(after));
        return ringPixels(before, after, Color.BLACK) >= 200 ? null : what + " draws no ring with focus";
    }

    /**
     * How many pixels focus turns into something at least 3:1 away from what they were, over the surface. A Material
     * state layer changes many pixels by a few percent; only a ring or an outline clears this.
     */
    private static int ringPixels(Bitmap before, Bitmap after, int surface) {
        int count = 0;
        for (int y = 0; y < before.getHeight(); y++) {
            for (int x = 0; x < before.getWidth(); x++) {
                int a = ColorUtils.compositeColors(before.getPixel(x, y), surface);
                int b = ColorUtils.compositeColors(after.getPixel(x, y), surface);
                double change = ColorUtils.calculateContrast(a, b);
                // A filled amber button rings in light text, which is only 1.6:1 from the amber it covers but 7:1 or more
                // from the surface around it, so a lesser change counts when the new colour stands out on the surface.
                if (change >= 3.0 || (change >= 1.5 && ColorUtils.calculateContrast(b, surface) >= 7.0)) count++;
            }
        }
        return count;
    }

    private static void collect(View view, List<View> out) {
        if (view.getVisibility() != View.VISIBLE) return;
        // Scroll containers are focusable by default but only pass focus on to their children.
        if (view.isFocusable() && view.isEnabled() && !(view instanceof EditText) && !(view instanceof ScrollView)
                && !(view instanceof HorizontalScrollView) && !(view instanceof NestedScrollView)) {
            out.add(view);
        }
        if (view instanceof ViewGroup && !(view instanceof AbsListView)) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) collect(group.getChildAt(i), out);
        }
    }

    private static String describe(View view) {
        String name = view.getClass().getSimpleName();
        if (view.getId() != View.NO_ID) {
            try {
                name += "#" + view.getResources().getResourceEntryName(view.getId());
            } catch (RuntimeException ignored) {
                name += "#" + view.getId();
            }
        }
        return name;
    }

    private static void assertEveryStopShowsFocus(String screen, View root) {
        List<View> stops = new ArrayList<>();
        collect(root, stops);
        assertFalse(screen + " has nothing to focus", stops.isEmpty());
        List<String> problems = new ArrayList<>();
        for (View stop : stops) {
            String problem = focusProblem(stop);
            if (problem != null) problems.add(describe(stop) + ": " + problem);
        }
        assertTrue(screen + ": " + problems, problems.isEmpty());
    }

    private void assertScreens(String qualifier) {
        try (var pcs = Robolectric.buildActivity(PcViewLifecycleTest.Library.class,
                com.limelight.console.HomeSettings.pcList(context)).setup().visible()) {
            assertEveryStopShowsFocus(qualifier + " PCs", pcs.get().getWindow().getDecorView());
        }
        try (var settings = Robolectric.buildActivity(StreamSettings.class,
                ShellBar.intentFor(context, Destination.SETTINGS)).setup().visible()) {
            View decor = settings.get().getWindow().getDecorView();
            assertEveryStopShowsFocus(qualifier + " Settings", decor);
            AbsListView list = findList(decor);
            assertNotNull(qualifier + " Settings list", list);
            assertTrue(qualifier + " Settings list: " + selectorProblem(list.getSelector(), "selector"),
                    selectorProblem(list.getSelector(), "selector") == null);
        }
    }

    private static AbsListView findList(View view) {
        if (view instanceof AbsListView) return (AbsListView) view;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                AbsListView found = findList(group.getChildAt(i));
                if (found != null) return found;
            }
        }
        return null;
    }

    /** The check is not vacuous: a focusable that draws nothing for focus is reported. */
    @Test
    public void aFocusableWithoutAFocusStateIsReported() {
        try (var screen = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            View bare = new View(screen.get());
            bare.setFocusable(true);
            ((ViewGroup) screen.get().findViewById(android.R.id.content)).addView(bare);
            assertNotNull(focusProblem(bare));
        }
    }

    @Test
    public void everyPhoneStopOnPcsAndSettingsShowsFocus() {
        assertScreens("phone");
    }

    @Test @Config(qualifiers = "television")
    public void everyTelevisionStopOnPcsAndSettingsShowsFocus() {
        assertScreens("television");
    }

    @Test @Config(qualifiers = "television")
    public void everyTelevisionStopShowsFocusInHighContrastToo() {
        PreferenceManager.getDefaultSharedPreferences(context).edit().putBoolean(AppTheme.HIGH_CONTRAST, true).commit();
        assertScreens("television high contrast");
    }

    @Test @Config(qualifiers = "television")
    public void everyTelevisionStopShowsFocusWithReducedMotion() {
        PreferenceManager.getDefaultSharedPreferences(context).edit().putString(Motion.REDUCE_MOTION, Motion.REDUCE).commit();
        assertScreens("television reduced motion");
    }

    /** The game page's quit-on-leave row takes the settings rows' outline from a remote. */
    @Test @Config(qualifiers = "television")
    public void theGamePagesSwitchRowShowsFocusOnTelevision() {
        try (var screen = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            View row = LayoutInflater.from(screen.get()).inflate(R.layout.view_detail_quit_on_leave, null);
            ((ViewGroup) screen.get().findViewById(android.R.id.content)).addView(row);
            row.setVisibility(View.VISIBLE);
            assertEveryStopShowsFocus("game page switch row", row);
        }
    }

    @Test @Config(qualifiers = "television")
    public void gridCardsShowFocusOnTelevision() {
        for (int layout : new int[] {R.layout.pc_grid_item, R.layout.app_grid_item, R.layout.app_grid_item_small}) {
            try (var screen = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
                View card = LayoutInflater.from(screen.get()).inflate(layout, null);
                ((ViewGroup) screen.get().findViewById(android.R.id.content)).addView(card);
                card.setFocusable(true);
                // The adapter takes the overflow button out of focus; AdaptiveGridView marks it by hover instead.
                View more = card.findViewById(R.id.grid_more);
                if (more != null) {
                    more.setFocusable(false);
                    size(more);
                    more.jumpDrawablesToCurrentState();
                    Bitmap plain = draw(more);
                    more.setHovered(true);
                    more.jumpDrawablesToCurrentState();
                    assertFalse("the marked overflow button shows a ring", plain.sameAs(draw(more)));
                }
                assertEveryStopShowsFocus(context.getResources().getResourceEntryName(layout), card);
            }
        }
    }

    /** On a phone the grid's selector draws the ring; on television the animated card owns it (see gridCardsShow...). */
    @Test
    public void theGridSelectorShowsFocusOnAPhone() {
        for (int layout : new int[] {R.layout.pc_grid_view, R.layout.app_grid_view, R.layout.app_grid_view_small}) {
            try (var screen = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
                View grid = LayoutInflater.from(screen.get()).inflate(layout, null);
                AbsListView list = findList(grid);
                assertNotNull(list);
                String problem = selectorProblem(list.getSelector(), "grid selector");
                assertTrue(context.getResources().getResourceEntryName(layout) + ": " + problem, problem == null);
            }
        }
    }

    @Test @Config(qualifiers = "television")
    public void theActionSheetShowsFocusOnEveryRowAndToggle() {
        try (var screen = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            ActionSheet sheet = new ActionSheet(screen.get(), "Menu");
            sheet.addAction(R.drawable.ic_play, "Play");
            sheet.addAction(R.drawable.ic_settings, "Settings");
            var row = sheet.addToggleRow();
            sheet.addToggle(row, R.drawable.ic_play, "One", false);
            sheet.addToggle(row, R.drawable.ic_settings, "Two", true);
            sheet.show();
            assertEveryStopShowsFocus("action sheet", sheet.getWindow().getDecorView());
            sheet.dismiss();
        }
    }

    /**
     * TV run on a TCL C6K: the focused Add PC pill only darkened. It takes an amber ring outside the pill, as a PC tile
     * does, and the pill keeps its 56dp height and its amber. A phone's Add PC is unchanged.
     */
    @Test @Config(qualifiers = "television")
    public void addPcShowsAnAmberRingOutsideItsPillOnTelevision() {
        try (var screen = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            ViewGroup content = screen.get().findViewById(android.R.id.content);
            View pcs = LayoutInflater.from(screen.get()).inflate(R.layout.activity_pc_view, content, false);
            content.addView(pcs);
            View add = pcs.findViewById(R.id.manuallyAddPc);
            add.setVisibility(View.VISIBLE);
            size(add);
            float density = add.getResources().getDisplayMetrics().density;
            assertEquals("the pill stays 56dp inside the ring's room", Math.round(66 * density), add.getHeight());
            assertNull(focusProblem(add));
            int amber = com.google.android.material.color.MaterialColors.getColor(add,
                    androidx.appcompat.R.attr.colorPrimary);
            Bitmap focused = draw(add);
            int x = add.getWidth() / 2;
            int ringY = Math.round(2.5f * density);
            int pillY = Math.round(10 * density);
            assertEquals("the ring is amber", amber, focused.getPixel(x, ringY));
            assertTrue("focus leaves the pill amber, not darkened",
                    ColorUtils.calculateContrast(ColorUtils.compositeColors(focused.getPixel(x, pillY), Color.BLACK), amber) < 1.1);
            add.clearFocus();
            settle(add);
            assertEquals("no ring without focus", 0, Color.alpha(draw(add).getPixel(x, ringY)));
        }
    }

    @Test
    public void addPcOnAPhoneKeepsItsPillSize() {
        try (var screen = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            ViewGroup content = screen.get().findViewById(android.R.id.content);
            View pcs = LayoutInflater.from(screen.get()).inflate(R.layout.activity_pc_view, content, false);
            content.addView(pcs);
            View add = pcs.findViewById(R.id.manuallyAddPc);
            add.setVisibility(View.VISIBLE);
            size(add);
            assertEquals(Math.round(56 * add.getResources().getDisplayMetrics().density), add.getHeight());
            assertNull(add.getForeground());
        }
    }

    @Test @Config(qualifiers = "television")
    public void dialogButtonsShowFocusOnTelevision() {
        try (var screen = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            androidx.appcompat.app.AlertDialog dialog = new NavigationDialogBuilder(screen.get())
                    .setTitle("Title").setMessage("Message")
                    .setPositiveButton(android.R.string.ok, null)
                    .setNegativeButton(android.R.string.cancel, null)
                    .setNeutralButton("More", null).show();
            for (int which : new int[] {android.content.DialogInterface.BUTTON_POSITIVE,
                    android.content.DialogInterface.BUTTON_NEGATIVE, android.content.DialogInterface.BUTTON_NEUTRAL}) {
                assertEveryStopShowsFocus("dialog button " + which, dialog.getButton(which));
            }
            dialog.dismiss();
        }
    }
}
