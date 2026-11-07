package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Color;
import android.graphics.ColorFilter;
import android.graphics.PorterDuff;
import android.graphics.PorterDuffColorFilter;
import android.os.Build;
import android.view.View;
import android.view.WindowManager;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.console.ConnectionInfo;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static com.limelight.ui.StreamMenu.Item.*;
import static org.junit.Assert.*;

/**
 * The glass stream menu's own rules: closed it leaves nothing on the stream, its blur window lives only while it is open and
 * only where the tier and the system allow, one way out is red, and the bitrate slider commits through the stream's path.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w960dp-h540dp-land-xhdpi")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class StreamMenuGlassTest {
    @Before
    public void answerFromThePolicySources() {
        StreamMenuModel.use(new RustStreamMenuModel());
    }

    @After
    public void forget() {
        StreamMenuModel.use(null);
        MenuBlurWindow.enabledForTest = null;
    }

    private static StreamMenu open(Activity activity, java.util.function.Consumer<StreamMenu.Options> fill) {
        activity.setTheme(R.style.StreamTheme);
        StreamMenu.Options state = new StreamMenu.Options(NvApp.Role.STREAM, TestHosts.details("rubylight-serverinfo.xml"));
        state.appName = "Skybound";
        state.streamWidth = 1920;
        state.streamHeight = 1080;
        state.streamFps = 120;
        if (fill != null) fill.accept(state);
        StreamMenu menu = new StreamMenu(activity, () -> state, item -> { }, null, null);
        menu.show();
        ShadowLooper.idleMainLooper(100, TimeUnit.MILLISECONDS);
        return menu;
    }

    private static List<View> walk(View root) {
        List<View> all = new ArrayList<>();
        java.util.ArrayDeque<View> left = new java.util.ArrayDeque<>(List.of(root));
        while (!left.isEmpty()) {
            View view = left.pop();
            all.add(view);
            if (view instanceof android.view.ViewGroup) {
                android.view.ViewGroup group = (android.view.ViewGroup) view;
                for (int i = 0; i < group.getChildCount(); i++) left.add(group.getChildAt(i));
            }
        }
        return all;
    }

    @Test
    public void theBlurWindowAsksForAnAndroidATierThatCanAffordItAndTheSystemSwitch() {
        assertTrue(MenuBlurWindow.allowed(Build.VERSION_CODES.S, GlassTier.LIVE, true));
        assertTrue(MenuBlurWindow.allowed(34, GlassTier.LENS, true));
        assertFalse("older than the call", MenuBlurWindow.allowed(Build.VERSION_CODES.R, GlassTier.LENS, true));
        assertFalse("the system turned blur off", MenuBlurWindow.allowed(34, GlassTier.LENS, false));
        assertFalse("the baked tier never adds a window", MenuBlurWindow.allowed(34, GlassTier.BAKED, true));
    }

    /** The bar for the stream path: closed, the blur window and the panel are gone, and the activity's own window has nothing new. */
    @Test
    public void closedTheMenuLeavesNothingOnTheStream() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            int before = walk(activity.getWindow().getDecorView()).size();
            int flagsBefore = activity.getWindow().getAttributes().flags;
            MenuBlurWindow.enabledForTest = true;
            StreamMenu menu = open(activity, null);
            MenuBlurWindow blur = menu.sheet().blurWindow();
            // Robolectric's tier can land on baked; the rule itself is pinned above. When it did open, it must also close.
            menu.sheet().dismiss();
            ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
            assertFalse(menu.sheet().isShowing());
            if (blur != null) assertFalse("the blur window is removed, not hidden", blur.isShowing());
            assertNull("and forgotten", menu.sheet().blurWindow());
            assertEquals("no view left on the stream's window", before, walk(activity.getWindow().getDecorView()).size());
            assertEquals("no flag left on the stream's window", flagsBefore, activity.getWindow().getAttributes().flags);
        }
    }

    @Test
    public void whenTheSystemHasNoBlurThereIsNoSecondWindowAndTheFillIsBaked() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            MenuBlurWindow.enabledForTest = false;
            StreamMenu menu = open(controller.get(), null);
            assertNull(menu.sheet().blurWindow());
            assertEquals("opaque enough to read over any game", 0xF0, Color.alpha(menu.sheet().menuGlass().fillColor()));
            menu.sheet().dismiss();
        }
    }

    @Test
    public void thePanelDimsNothingAroundItself() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            StreamMenu menu = open(controller.get(), null);
            int flags = menu.sheet().getWindow().getAttributes().flags;
            assertEquals("the stream beside the panel stays as bright as it was", 0, flags & WindowManager.LayoutParams.FLAG_DIM_BEHIND);
            menu.sheet().dismiss();
        }
    }

    @Test
    public void theLastWayOutThatEndsSomethingIsTheOneRedControl() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            StreamMenu menu = open(controller.get(), null);
            int red = 0;
            MaterialButton last = null;
            android.view.ViewGroup footer = (android.view.ViewGroup) menu.row(DISCONNECT).getParent();
            for (int i = 0; i < footer.getChildCount(); i++) {
                MaterialButton button = (MaterialButton) footer.getChildAt(i);
                last = button;
                if (((GlassControlDrawable) button.getBackground()).kind() == GlassControlDrawable.Kind.DANGER) red++;
            }
            assertEquals(1, red);
            assertEquals(GlassControlDrawable.Kind.DANGER, ((GlassControlDrawable) last.getBackground()).kind());
            menu.sheet().dismiss();
        }
    }

    @Test
    public void theHeaderCarriesTheCoverAndTheXClosesTheMenu() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Bitmap art = Bitmap.createBitmap(4, 6, Bitmap.Config.ARGB_8888);
            StreamMenu menu = open(controller.get(), o -> o.cover = art);
            assertEquals(View.VISIBLE, menu.sheet().cover().getVisibility());
            menu.row(CLOSE).performClick();
            assertFalse(menu.sheet().isShowing());
        }
    }

    @Test
    public void withoutACoverTheHeaderIsNotLeftWithAHole() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            StreamMenu menu = open(controller.get(), null);
            assertEquals(View.GONE, menu.sheet().cover().getVisibility());
            menu.sheet().dismiss();
        }
    }

    /** A status row runs the same item as the tile it replaces, and a hotkey on a cell still reaches it. */
    @Test
    public void statusRowsCarryTheirValueAndRunTheirItem() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            StreamMenu.Options state = new StreamMenu.Options(NvApp.Role.STREAM, TestHosts.details("rubylight-serverinfo.xml"));
            state.streamFps = 120;
            state.overlayShown = true;
            List<StreamMenu.Item> chosen = new ArrayList<>();
            StreamMenu menu = new StreamMenu(activity, () -> state, chosen::add, null, null);
            menu.show();
            StreamMenuModel.Layout layout = menu.layout();
            for (int at = 0; at < layout.grid.size(); at++) {
                View view = menu.cell(at);
                assertNotNull("every cell of the model has its view: " + layout.grid.get(at).id, view);
                if (view instanceof MenuRow) {
                    MenuRow row = (MenuRow) view;
                    if ("rate_change".equals(layout.grid.get(at).id) || "bitrate".equals(layout.grid.get(at).id)) {
                        assertEquals("120 FPS", row.value());
                        assertTrue(row.getContentDescription().toString().endsWith(", 120 FPS"));
                    }
                }
            }
            // A page row opens its page; one that reports says so once.
            menu.row(RATE_CHANGE != null && menu.row(RATE_CHANGE) != null ? RATE_CHANGE : BITRATE).performClick();
            assertEquals(1, chosen.size());
            menu.sheet().dismiss();
        }
    }

    @Test
    public void theBitrateSliderCommitsOnceThroughTheStreamsPathAndNamesItsValue() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            List<Integer> sent = new ArrayList<>();
            StreamMenu menu = open(controller.get(), o -> {
                o.bitrateKbps = 40000;
                o.bitrateChange = kbps -> {
                    sent.add(kbps);
                    return kbps;
                };
            });
            StepSlider slider = menu.bitrateSlider();
            assertNotNull(slider);
            assertTrue("nothing goes out before the slider moves", sent.isEmpty());
            int at = slider.getPosition();
            long now = android.os.SystemClock.uptimeMillis();
            assertTrue(slider.onKeyDown(android.view.KeyEvent.KEYCODE_DPAD_RIGHT,
                    new android.view.KeyEvent(now, now, android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_RIGHT, 0)));
            assertEquals(at + 1, slider.getPosition());
            ShadowLooper.idleMainLooper(5, TimeUnit.SECONDS);
            assertEquals(1, sent.size());
            assertTrue("sent " + sent.get(0), sent.get(0) != 40000);
            menu.sheet().dismiss();
        }
    }

    /** A busy stream sends nothing: the slider goes back to the rate still applied, not the one it was moved to. */
    @Test
    public void aRateNotSentSnapsTheSliderBack() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            StreamMenu menu = open(controller.get(), o -> {
                o.bitrateKbps = 40000;
                o.bitrateChange = kbps -> 40000;
            });
            StepSlider slider = menu.bitrateSlider();
            int at = slider.getPosition();
            long now = android.os.SystemClock.uptimeMillis();
            slider.onKeyDown(android.view.KeyEvent.KEYCODE_DPAD_RIGHT,
                    new android.view.KeyEvent(now, now, android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_RIGHT, 0));
            assertEquals(at + 1, slider.getPosition());
            ShadowLooper.idleMainLooper(5, TimeUnit.SECONDS);
            assertEquals(at, slider.getPosition());
            assertEquals(BitrateSlider.progress(40000), slider.getPosition());
            menu.sheet().dismiss();
        }
    }

    @Test
    public void noBitrateCallbackNoSlider() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            StreamMenu menu = open(controller.get(), o -> o.bitrateKbps = 40000);
            assertNull(menu.bitrateSlider());
            menu.sheet().dismiss();
        }
    }

    /** A button style's tint arrives as a transparent filter; the design's fill must survive it, the bug that made tiles empty. */
    @Test
    public void aButtonStylesTintCannotPaintAGlassControlOver() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            for (GlassControlDrawable.Kind kind : GlassControlDrawable.Kind.values()) {
                GlassControlDrawable drawable = new GlassControlDrawable(activity, kind, 20f);
                int fill = drawable.fillNow();
                ColorFilter tint = new PorterDuffColorFilter(Color.TRANSPARENT, PorterDuff.Mode.SRC_IN);
                drawable.setColorFilter(tint);
                assertEquals(kind.name(), fill, drawable.fillNow());
                assertNull(kind.name(), drawable.getColorFilter());
            }
        }
    }

    @Test
    public void aGlassControlShowsItsRingOnlyWhenFocused() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            Activity activity = controller.get();
            activity.setTheme(R.style.StreamTheme);
            GlassControlDrawable drawable = new GlassControlDrawable(activity, GlassControlDrawable.Kind.ROW, 20f);
            drawable.setState(new int[] {android.R.attr.state_enabled});
            assertFalse(drawable.ringShown());
            drawable.setState(new int[] {android.R.attr.state_enabled, android.R.attr.state_focused});
            assertTrue(drawable.ringShown());
            assertTrue("or the system lays a grey square over the round X", drawable.hasFocusStateSpecified());
        }
    }

    /** The menu's row says the link in a few words. */
    @Test
    public void theSummarySaysSizeLinkAndLatencyInAFewWords() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            ConnectionInfo info = new ConnectionInfo("PC", 0, 1920, 1080, 120, 40000, null, 4);
            String summary = info.summary(controller.get());
            assertTrue(summary, summary.startsWith("1920 × 1080"));
            assertTrue(summary, summary.endsWith("4 ms"));
        }
    }

    /** The footer's capsules stand side by side while their words fit; the red one takes its own line when they don't. */
    @Test @Config(qualifiers = "de-w640dp-h360dp-land-xhdpi")
    public void longWordsSendTheLastWayOutToItsOwnLine() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            ActionSheet sheet = new ActionSheet(activity, "Skybound", true);
            sheet.addFooterAction(R.drawable.ic_stream_leave, "Verbindung trennen nach langer Zeit", false);
            sheet.addFooterAction(R.drawable.ic_stream_leave, "Sitzung beenden nach langer Zeit", false);
            MaterialButton red = sheet.addFooterAction(R.drawable.ic_glass_power, "Spiel schliessen nach langer Zeit", true);
            ActionSheet.FooterBar footer = (ActionSheet.FooterBar) red.getParent();
            int width = (int) (360 * activity.getResources().getDisplayMetrics().density);
            footer.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY), View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED));
            footer.layout(0, 0, width, footer.getMeasuredHeight());
            assertTrue("red under the others", red.getTop() >= footer.getChildAt(0).getBottom());
            assertEquals("on its own line, across", footer.getMeasuredWidth() - footer.getPaddingLeft() - footer.getPaddingRight(), red.getWidth());
        }
    }
}
