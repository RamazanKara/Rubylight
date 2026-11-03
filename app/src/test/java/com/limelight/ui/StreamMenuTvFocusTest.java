package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.view.View;

import com.limelight.R;
import com.limelight.binding.input.KeyboardShortcuts;
import com.limelight.nvstream.http.ComputerDetails;
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
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static com.limelight.ui.StreamMenu.Item.*;
import static org.junit.Assert.*;

/**
 * The stream menu on a 1080p TV (nightjar#52, redesigned for nightjar#63): Back opens it with focus on its first
 * tile, Down walks the grid to Return to stream and then Disconnect, and Back again closes the menu without
 * leaving. Set NIGHTJAR_PREVIEW_OUTPUT to capture the sheet, on a TV and on a handheld held sideways.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "w960dp-h540dp-land-television-xhdpi")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class StreamMenuTvFocusTest {
    @Before
    public void answerFromThePolicySources() {
        StreamMenuModel.use(new RustStreamMenuModel());
    }

    @After
    public void forgetTheModel() {
        StreamMenuModel.use(null);
    }

    private static StreamMenu open(Activity activity, List<StreamMenu.Item> chosen) {
        activity.setTheme(R.style.StreamTheme);
        ComputerDetails host = TestHosts.details("rubylight-serverinfo.xml");
        StreamMenu.Options state = new StreamMenu.Options(NvApp.Role.STREAM, host);
        state.appName = "Open World";
        state.streamWidth = 2560;
        state.streamHeight = 1440;
        state.streamFps = 144;
        state.videoFormat = 0x200;
        state.hdr = true;
        state.webUiAddress = true;
        state.hostMenu = true;
        state.overlayShown = true;
        state.controllerMouseAvailable = true;
        state.microphoneOffered = true;
        List<KeyboardShortcuts.Shortcut> saved = new ArrayList<>();
        saved.add(new KeyboardShortcuts.Shortcut("a", "Task view", KeyboardShortcuts.parseChord("WIN+TAB")));
        saved.add(new KeyboardShortcuts.Shortcut("b", "Snip", KeyboardShortcuts.parseChord("WIN+SHIFT+S")));
        StreamMenu menu = new StreamMenu(activity, () -> state, chosen::add, null, new StreamMenu.Shortcuts() {
            @Override public List<KeyboardShortcuts.Shortcut> saved() { return new ArrayList<>(saved); }
            @Override public void save(List<KeyboardShortcuts.Shortcut> shortcuts) { }
            @Override public void send(int[] keys) { }
        });
        menu.show();
        ShadowLooper.idleMainLooper(100, TimeUnit.MILLISECONDS);
        return menu;
    }

    private static void capture(Activity activity, StreamMenu menu, String name) throws Exception {
        String output = System.getenv("NIGHTJAR_PREVIEW_OUTPUT");
        if (output == null || output.isBlank()) return;
        File directory = new File(output, "day");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        TvScreensPreviewTest.captureDialog(activity, menu.sheet(), new File(directory, name));
    }

    @Test
    public void focusStartsOnTheFirstTileAndDownWalksTheGridToReturnThenDisconnect() throws Exception {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            List<StreamMenu.Item> chosen = new ArrayList<>();
            StreamMenu menu = open(activity, chosen);
            // A remote's press leaves the sheet's own window in focus mode before its first focus lands.
            Object root = ReflectionHelpers.callInstanceMethod(menu.sheet().getWindow().getDecorView(), "getViewRootImpl");
            ReflectionHelpers.callInstanceMethod(root, "ensureTouchMode", ReflectionHelpers.ClassParameter.from(boolean.class, false));
            layOut(menu);
            // What the sheet does once it is attached, or a page is shown, outside touch mode.
            menu.sheet().focusFirstAction();
            View focused = menu.sheet().getWindow().getDecorView().findFocus();
            assertSame("focus starts on the first tile, not on Disconnect", menu.row(KEYBOARD), focused);
            assertEquals("the header says the stream's mode, as desktop Nightjar's", "1440p · 144 FPS · HEVC HDR",
                    menu.sheet().subtitle().getText().toString());
            assertSame("Right is the next tile", menu.grid().getChildAt(1), focused.focusSearch(View.FOCUS_RIGHT));

            View next = focused;
            for (int row = 0; row < 10 && next != null && next != menu.row(CLOSE); row++) {
                // As a remote's press does, the grid scrolls the focused tile into view before the next move.
                assertTrue(next.requestFocus());
                next = next.focusSearch(View.FOCUS_DOWN);
            }
            assertSame("Down through the grid reaches Return to stream", menu.row(CLOSE), next);
            assertSame("then Disconnect", menu.row(DISCONNECT), next.focusSearch(View.FOCUS_DOWN));
            assertTrue(focused.requestFocus());
            ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
            menu.row(KEYBOARD).jumpDrawablesToCurrentState();
            capture(activity, menu, "tv-stream-menu.png");

            menu.sheet().onBackPressed();
            assertFalse(menu.sheet().isShowing());
            assertTrue("Back closes the menu and nothing else", chosen.isEmpty());
        }
    }

    /**
     * A handheld such as the Retroid Pocket held sideways: compact tiles, five or more to a row, so a dozen or more
     * show above Return to stream and the ways out; then the shortcuts page.
     */
    @Test @Config(qualifiers = "w640dp-h360dp-land-xxhdpi")
    public void aHandheldShowsCompactTilesFiveOrMoreToARow() throws Exception {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            StreamMenu menu = open(activity, new ArrayList<>());
            layOut(menu);
            assertTrue("columns: " + menu.grid().columns(), menu.grid().columns() >= 5);
            float density = activity.getResources().getDisplayMetrics().density;
            View tile = menu.row(KEYBOARD);
            assertTrue("a tile is compact: " + tile.getHeight() / density + "dp", tile.getHeight() <= 64 * density);
            assertEquals("Keyboard", ((android.widget.TextView) tile).getText().toString());
            assertNoLabelCut(menu);
            capture(activity, menu, "handheld-stream-menu.png");
            menu.row(SHORTCUTS).performClick();
            layOut(menu);
            capture(activity, menu, "handheld-stream-shortcuts.png");
            menu.sheet().dismiss();
        }
    }

    /** German words run longest: every tile, Return to stream and the ways out keep their whole label. */
    @Test @Config(qualifiers = "de-w640dp-h360dp-land-xxhdpi")
    public void germanLabelsFitTheirTiles() throws Exception {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            StreamMenu menu = open(activity, new ArrayList<>());
            layOut(menu);
            assertEquals("Tastatur", ((android.widget.TextView) menu.row(KEYBOARD)).getText().toString());
            assertNoLabelCut(menu);
            capture(activity, menu, "handheld-stream-menu-de.png");
            menu.sheet().dismiss();
        }
    }

    /** Each label key of the shared model names the same English words here as on desktop Nightjar. */
    @Test
    public void everyModelLabelKeyHasItsEnglishString() {
        try (var controller = Robolectric.buildActivity(Activity.class).setup()) {
            java.util.Map<String, String> texts = new RustStreamMenuModel().labelTexts();
            assertTrue(texts.size() > 20);
            for (java.util.Map.Entry<String, String> text : texts.entrySet()) {
                int id = StreamMenu.tileLabel(text.getKey());
                assertNotEquals("no string for " + text.getKey(), 0, id);
                assertEquals(text.getKey(), text.getValue(), controller.get().getString(id));
            }
        }
    }

    private static void assertNoLabelCut(StreamMenu menu) {
        List<android.widget.TextView> labels = new ArrayList<>();
        for (int i = 0; i < menu.grid().getChildCount(); i++) labels.add((android.widget.TextView) menu.grid().getChildAt(i));
        View footer = (View) menu.row(CLOSE).getParent();
        for (int i = 0; i < ((android.view.ViewGroup) footer).getChildCount(); i++) {
            labels.add((android.widget.TextView) ((android.view.ViewGroup) footer).getChildAt(i));
        }
        for (android.widget.TextView label : labels) {
            android.text.Layout layout = label.getLayout();
            assertNotNull(label.getText().toString(), layout);
            for (int line = 0; line < layout.getLineCount(); line++) {
                assertEquals("\"" + label.getText() + "\" is cut", 0, layout.getEllipsisCount(line));
            }
            assertTrue("\"" + label.getText() + "\" runs past its lines", layout.getLineCount() <= label.getMaxLines());
        }
    }

    // The sheet's window draws no frames here: measure and lay it out at 1080p so focus search sees the rows.
    private static void layOut(StreamMenu menu) {
        ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
        View sheet = menu.sheet().getWindow().getDecorView();
        sheet.measure(View.MeasureSpec.makeMeasureSpec(1920, View.MeasureSpec.AT_MOST),
                View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.AT_MOST));
        sheet.layout(0, 0, sheet.getMeasuredWidth(), sheet.getMeasuredHeight());
        ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
    }
}
