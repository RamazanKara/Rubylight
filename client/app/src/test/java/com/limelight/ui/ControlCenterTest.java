package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.content.res.Resources;
import android.graphics.drawable.Drawable;
import android.preference.PreferenceManager;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import androidx.core.content.ContextCompat;
import androidx.core.graphics.ColorUtils;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.color.MaterialColors;
import com.limelight.R;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.NightjarUiPolicyJni;

import org.json.JSONObject;
import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.concurrent.TimeUnit;

import static com.limelight.ui.StreamMenu.Item.*;
import static com.limelight.ui.StreamMenuTestSupport.*;
import static org.junit.Assert.*;

/**
 * The Control Center over a stream, as a player meets it. Everything the 6.x menu offered is on exactly one module or
 * under every page and still does what it did; an upright phone gets the sheet and everything wide the dock; the
 * stream's notice heads Quick and the PC's encoder load stands on Picture; a later feature's card waits for the PC's
 * gate; and it is the night palette under any theme, solid under high contrast. How it is steered is
 * {@link ControlCenterNavigationTest}'s; what it costs the stream, closed and open, {@link ControlCenterSurfaceTest}'s.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = ControlCenterTest.HANDHELD)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class ControlCenterTest {
    static final String HANDHELD = "w640dp-h360dp-land-xxhdpi";
    static final String PHONE_UPRIGHT = "w393dp-h852dp-port-xxhdpi";
    static final String TELEVISION = "w960dp-h540dp-land-television-xhdpi";

    @Before
    public void aDeviceThatDoesNotBlur() {
        quietDevice();
    }

    @After
    public void forget() {
        forgetDevice();
    }

    /** A device whose system never blurs: the panel is the baked one, as on most streams. */
    static void quietDevice() {
        ControlCenterSurface.signalsForTest = owner -> {
            ControlCenterSurfaceTest.Registry quiet = new ControlCenterSurfaceTest.Registry();
            quiet.blur = false;
            return quiet;
        };
        MenuBlurWindow.enabledForTest = false;
        GlassTier.force(RuntimeEnvironment.getApplication(), GlassTier.BAKED);
    }

    static void forgetDevice() {
        ControlCenterSurface.signalsForTest = null;
        MenuBlurWindow.enabledForTest = null;
        StreamMenu.gamepadForTest = null;
        GlassTier.force(RuntimeEnvironment.getApplication(), null);
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
    }

    /** A stream on a PC that offers all there is to offer, so every module but Clips has something. */
    static StreamMenu.Options everything(NvApp.Role role) throws Exception {
        StreamMenu.Options o = options(nativeApiHost(), role);
        o.appName = "Skybound";
        o.microphoneOffered = o.micButtonAvailable = o.menuButtonAvailable = o.mouseConnected = o.controllerMouseAvailable = true;
        o.onscreenControls = o.gameControls = o.scalingAvailable = o.zoomEnabled = o.pictureInPicture = true;
        o.externalDisplays = o.hostMenu = o.usbForwarding = o.sessionGuests = o.imageClipboard = true;
        o.nightModeOffered = o.headphoneSurroundOffered = true;
        o.hostAudio = NativeApiFixtures.audio(NativeApiFixtures.audioJson(true, true));
        o.bitrateKbps = 20000;
        o.bitrateChange = kbps -> kbps;
        return o;
    }

    static StreamMenu open(Activity activity, StreamMenu.Options state, List<StreamMenu.Item> chosen,
                                   StreamMenu.Session session) {
        activity.setTheme(R.style.StreamTheme);
        StreamMenu menu = new StreamMenu(activity, () -> state, chosen::add, null,
                new SavedShortcuts().add("Snip", "WIN+SHIFT+S"), session);
        menu.show();
        settle();
        return menu;
    }

    static void settle() {
        ShadowLooper.idleMainLooper(600, TimeUnit.MILLISECONDS);
    }

    /** Lays the Control Center's window out at the screen's size, as the system does once it shows. */
    static void layOut(Activity activity, StreamMenu menu) {
        settle();
        View window = menu.sheet().getWindow().getDecorView();
        android.util.DisplayMetrics screen = activity.getResources().getDisplayMetrics();
        window.measure(View.MeasureSpec.makeMeasureSpec(screen.widthPixels, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(screen.heightPixels, View.MeasureSpec.EXACTLY));
        window.layout(0, 0, screen.widthPixels, screen.heightPixels);
        settle();
    }

    /** As after a key press: the view's window leaves touch mode, so its cards take the focus. */
    static void byKeys(View inWindow) {
        Object root = ReflectionHelpers.callInstanceMethod(inWindow.getRootView(), "getViewRootImpl");
        ReflectionHelpers.callInstanceMethod(root, "ensureTouchMode", ReflectionHelpers.ClassParameter.from(boolean.class, false));
    }

    static int[] inWindow(View view) {
        int[] at = new int[2];
        view.getLocationInWindow(at);
        return at;
    }

    /** The rail itself: what holds the trigger caps and the list of modules. */
    private static View rail(ControlCenterDock dock, String module) {
        return (View) dock.railEntry(module).getParent().getParent().getParent();
    }

    private static int danger(ViewGroup exits) {
        int red = 0;
        for (int i = 0; i < exits.getChildCount(); i++) {
            Drawable background = exits.getChildAt(i).getBackground();
            if (background instanceof GlassControlDrawable
                    && ((GlassControlDrawable) background).kind() == GlassControlDrawable.Kind.DANGER) red++;
        }
        return red;
    }

    @Test
    public void everythingTheStreamOffersIsOnOneModuleOrUnderThePagesAndDoesWhatItDid() throws Exception {
        for (NvApp.Role role : NvApp.Role.values()) {
            try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
                Activity activity = controller.get();
                StreamMenu.Options state = everything(role);
                List<StreamMenu.Item> chosen = new ArrayList<>();
                StreamMenu menu = open(activity, state, chosen, null);
                List<StreamMenu.Entry> offered = StreamMenuCatalog.pageEntries(activity, StreamMenu.Page.MAIN, state);
                if (state.canSendInput) {
                    offered.add(StreamMenuCatalog.action(StreamMenu.Section.QUICK, SHORTCUTS, R.string.stream_menu_shortcuts,
                            R.drawable.ic_keyboard));
                }
                assertTrue(role + " offers enough to sort", offered.size() > 5);
                ControlCenterModel.Dock dock = menu.dock();
                assertNotNull(role.name(), dock);
                ViewGroup exits = menu.sheet().exits();
                for (StreamMenu.Entry entry : offered) {
                    String id = StreamMenuCatalog.id(entry.item);
                    String what = role + " " + id;
                    // The shared model: one page, or the footer.
                    int pages = 0;
                    for (ControlCenterModel.Page page : dock.pages) if (page.holds(id)) pages++;
                    assertEquals(what + " in the model", 1, pages + (dock.footer.contains(id) ? 1 : 0));
                    // What is drawn: one module's page, or under every page.
                    int shown = 0;
                    boolean under = false;
                    for (String module : menu.modules()) {
                        menu.select(module);
                        View view = menu.row(entry.item);
                        if (view == null) continue;
                        if (view.getParent() == exits) under = true;
                        else shown++;
                    }
                    assertEquals(what + " on the screen", 1, shown + (under ? 1 : 0));
                    assertEquals(what + " is a way out", dock.footer.contains(id), under);
                    // Choosing it does what the list's row did: the stream hears the item, or its page opens.
                    chosen.clear();
                    MaterialButton view = menu.reveal(entry.item);
                    assertNotNull(what, view);
                    assertTrue(what + " can be chosen", view.isClickable() && view.isFocusable() && view.isEnabled());
                    if (!under) assertEquals(what, dock.moduleOf(id), menu.module());
                    view.performClick();
                    if (entry.opensPage()) {
                        assertTrue(what, chosen.isEmpty());
                        assertNotEquals(what, StreamMenu.Page.MAIN, menu.page());
                        menu.back();
                        assertEquals(what, StreamMenu.Page.MAIN, menu.page());
                    } else {
                        assertEquals(what, Arrays.asList(entry.item), chosen);
                    }
                }
                // Under every module the same ways out, and one red control at most: Quit game.
                for (String module : menu.modules()) {
                    menu.select(module);
                    assertSame(menu.sheet().primary(), menu.row(CLOSE));
                    assertSame(exits, menu.row(DISCONNECT).getParent());
                    assertEquals(role + " " + module, role == NvApp.Role.STREAM ? 1 : 0, danger(exits));
                    assertEquals(role + " " + module, role == NvApp.Role.STREAM ? 2 : 1, exits.getChildCount());
                }
                menu.sheet().dismiss();
            }
        }
    }

    private static String policyPlacement(String form, float widthDp, float heightDp) throws Exception {
        JSONObject reply = new JSONObject(NightjarUiPolicyJni.controlCenterCall(new JSONObject().put("op", "placement")
                .put("form", form).put("width_dp", widthDp).put("height_dp", heightDp).toString()));
        return reply.getString("ok");
    }

    @Test
    public void onlyAnUprightPhoneGetsTheSheetAndTheMirrorSaysWhatThePolicySays() throws Exception {
        assertEquals(ControlCenterModel.SHEET, policyPlacement(ViewportScaler.PHONE, 393, 852));
        assertEquals(ControlCenterModel.DOCK, policyPlacement(ViewportScaler.PHONE, 852, 393));
        assertEquals(ControlCenterModel.DOCK, policyPlacement(ViewportScaler.HANDHELD, 1280, 720));
        assertEquals(ControlCenterModel.DOCK, policyPlacement(ViewportScaler.TV, 960, 540));
        float[][] sizes = {{322, 792}, {393, 852}, {852, 393}, {360, 640}, {640, 360}, {600, 600}, {673, 841}, {841, 673},
                {1280, 720}, {960, 540}, {540, 960}};
        // The forms an Android window has (ViewportScaler.form): the Java copy answers as the policy for each.
        for (String form : new String[] {ViewportScaler.PHONE, ViewportScaler.HANDHELD, ViewportScaler.TV}) {
            for (float[] size : sizes) {
                String what = form + " " + size[0] + "x" + size[1];
                String policy = policyPlacement(form, size[0], size[1]);
                assertEquals(what, policy, ControlCenterModel.placement(form, size[0], size[1]));
                assertEquals(what, policy, ControlCenterModel.placementMirror(form, size[0], size[1]));
            }
        }
    }

    @Test @Config(qualifiers = PHONE_UPRIGHT)
    public void anUprightPhoneGetsTheSheetFromTheBottomWithTheRailAcrossItsTop() throws Exception {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            StreamMenu menu = open(activity, everything(NvApp.Role.STREAM), new ArrayList<>(), null);
            layOut(activity, menu);
            ControlCenterDock dock = menu.sheet();
            Resources res = dock.getContext().getResources();
            float density = res.getDisplayMetrics().density;
            int touch = res.getDimensionPixelSize(R.dimen.nj_size_touch_min);
            assertEquals(ControlCenterModel.SHEET, dock.placement());
            assertTrue(dock.frame().sheet());
            View frame = dock.frame(), panel = dock.panel();
            assertTrue(frame.getWidth() > 0 && frame.getHeight() > frame.getWidth());
            assertEquals("as wide as the screen", frame.getWidth(), panel.getWidth());
            assertEquals("from the bottom edge", frame.getHeight(), panel.getBottom());
            assertTrue("the top of the picture stays in sight", panel.getTop() >= 56 * density);
            assertTrue("as tall whatever its page holds", panel.getHeight() <= Math.round(frame.getHeight() * 0.72f));

            List<String> modules = menu.modules();
            assertTrue(modules.toString(), modules.size() >= 5);
            MaterialButton first = dock.railEntry(modules.get(0)), second = dock.railEntry(modules.get(1));
            assertEquals("the rail runs across", first.getTop(), second.getTop());
            assertTrue(second.getLeft() >= first.getRight());
            View rail = rail(dock, modules.get(0)), page = dock.page().scroll();
            assertEquals("the rail is as wide as the sheet", panel.getWidth(), rail.getWidth());
            assertTrue("over the page", inWindow(rail)[1] + rail.getHeight() <= inWindow(page)[1]);
            assertTrue("the page has room", page.getHeight() > touch * 3);
            for (String module : modules) {
                MaterialButton entry = dock.railEntry(module);
                assertTrue(module + " is a finger wide", entry.getWidth() >= touch && entry.getHeight() >= touch);
                assertEquals(activity.getString(ControlCenterCards.label(module)), String.valueOf(entry.getContentDescription()));
                assertEquals(module, module.equals(menu.module()), entry.isSelected());
            }
            // Under the page, in sight: Return to stream as wide as the sheet's footer, then the ways out.
            MaterialButton back = dock.primary();
            assertTrue(back.isShown() && back.getHeight() >= touch && back.getWidth() > panel.getWidth() / 2);
            assertTrue(inWindow(back)[1] >= inWindow(page)[1] + page.getHeight());
            assertTrue(inWindow(dock.exits())[1] >= inWindow(back)[1] + back.getHeight());
            assertTrue(inWindow(dock.exits())[1] + dock.exits().getHeight() <= inWindow(frame)[1] + frame.getHeight());
            for (int i = 0; i < dock.exits().getChildCount(); i++) {
                assertTrue(dock.exits().getChildAt(i).getHeight() >= touch);
            }
            for (int i = 0; i < menu.grid().getChildCount(); i++) {
                View card = menu.grid().getChildAt(i);
                assertTrue(card.getHeight() >= res.getDimensionPixelSize(R.dimen.nj_size_control_center_card));
                assertTrue(card.getWidth() >= touch);
            }
            menu.sheet().dismiss();
        }
    }

    @Test @Config(qualifiers = TELEVISION)
    public void aTvGetsTheDockInsideItsSafeAreaWithTheRailDownItsSide() throws Exception {
        StreamMenu.gamepadForTest = true;
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            StreamMenu menu = open(activity, everything(NvApp.Role.STREAM), new ArrayList<>(), null);
            layOut(activity, menu);
            ControlCenterDock dock = menu.sheet();
            Resources res = dock.getContext().getResources();
            int safe = res.getDimensionPixelSize(R.dimen.nj_layout_safe_tv);
            assertEquals(ControlCenterModel.DOCK, dock.placement());
            assertFalse(dock.frame().sheet());
            View frame = dock.frame(), panel = dock.panel();
            // A TV's dock and rail are wider than a tablet's: they carry the 10-foot type.
            assertEquals(res.getDimensionPixelSize(R.dimen.nj_size_control_center_width_tv), panel.getWidth());
            assertEquals("at the end edge, inside the overscan", frame.getWidth() - safe, panel.getRight());
            assertEquals(safe, panel.getTop());
            assertEquals(frame.getHeight() - safe, panel.getBottom());

            List<String> modules = menu.modules();
            MaterialButton first = dock.railEntry(modules.get(0)), second = dock.railEntry(modules.get(1));
            assertEquals("the rail runs down", first.getLeft(), second.getLeft());
            assertTrue(second.getTop() >= first.getBottom());
            View rail = rail(dock, modules.get(0)), page = dock.page().scroll();
            assertEquals(res.getDimensionPixelSize(R.dimen.nj_size_control_center_rail_tv), rail.getWidth());
            assertEquals("at the dock's start side", inWindow(panel)[0], inWindow(rail)[0]);
            assertTrue("the page beside it", inWindow(page)[0] >= inWindow(rail)[0] + rail.getWidth());
            assertEquals(2, menu.grid().columns());
            for (String module : modules) {
                android.text.Layout name = dock.railEntry(module).getLayout();
                assertEquals(module + "'s name whole on the rail", 0, name.getEllipsisCount(name.getLineCount() - 1));
            }
            for (int i = 0; i < menu.grid().getChildCount(); i++) {
                if (!(menu.grid().getChildAt(i) instanceof TileGrid.Tile)) continue;
                TileGrid.Tile card = (TileGrid.Tile) menu.grid().getChildAt(i);
                // A card hyphenates only a word wider than the room its cap leaves.
                assertEquals(card.getText() + " beside its cap, no word broken", android.text.Layout.HYPHENATION_FREQUENCY_NONE,
                        card.getHyphenationFrequency());
            }
            menu.sheet().dismiss();
        }
    }

    /** A phone on its side and a handheld: the dock at its narrower width, clear of the edges. */
    @Test @Config(qualifiers = "w852dp-h393dp-land-xxhdpi")
    public void aPhoneOnItsSideGetsTheDockAtItsCompactWidth() throws Exception {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            StreamMenu menu = open(activity, everything(NvApp.Role.STREAM), new ArrayList<>(), null);
            layOut(activity, menu);
            ControlCenterDock dock = menu.sheet();
            Resources res = dock.getContext().getResources();
            int margin = res.getDimensionPixelSize(R.dimen.control_center_margin);
            assertEquals(ControlCenterModel.DOCK, dock.placement());
            View frame = dock.frame(), panel = dock.panel();
            assertEquals(res.getDimensionPixelSize(R.dimen.nj_size_control_center_width_compact), panel.getWidth());
            assertEquals(frame.getWidth() - margin, panel.getRight());
            assertEquals(margin, panel.getTop());
            assertEquals(frame.getHeight() - margin, panel.getBottom());
            assertTrue("most of the picture stays in sight", panel.getLeft() > frame.getWidth() / 2);
            assertTrue("vertical fade softens bottom edge", dock.page().scroll().isVerticalFadingEdgeEnabled());
            assertTrue(dock.page().scroll().getVerticalFadingEdgeLength() >= Math.round(16 * res.getDisplayMetrics().density));
            assertFalse(dock.page().content().getClipToPadding());
            for (int i = 0; i < menu.grid().getChildCount(); i++) {
                if (!(menu.grid().getChildAt(i) instanceof TileGrid.Tile)) continue;
                TileGrid.Tile card = (TileGrid.Tile) menu.grid().getChildAt(i);
                assertEquals(card.getText() + ": never hyphenates mid-word on compact dock",
                        android.text.Layout.HYPHENATION_FREQUENCY_NONE, card.getHyphenationFrequency());
                String firstLine = card.getText().toString().split("\n", 2)[0];
                if (!firstLine.contains(" ")) {
                    int line0End = card.getLayout().getLineEnd(0);
                    assertTrue("single-word label '" + firstLine + "' stays unbroken on line 0",
                            line0End >= firstLine.length());
                }
            }
            menu.sheet().dismiss();
        }
    }

    /** A handheld's cards are narrow: a name whose word does not fit beside its cap takes the cap up into the corner. */
    @Test
    public void aNarrowCardsNameStaysWholeAndTheRailKeepsInsideTheDocksCorners() throws Exception {
        // With a controller there, so the cards draw their caps.
        StreamMenu.gamepadForTest = true;
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            StreamMenu menu = open(activity, everything(NvApp.Role.STREAM), new ArrayList<>(), null);
            layOut(activity, menu);
            boolean moved = false;
            for (int i = 0; i < menu.grid().getChildCount(); i++) {
                if (!(menu.grid().getChildAt(i) instanceof TileGrid.Tile)) continue;
                TileGrid.Tile card = (TileGrid.Tile) menu.grid().getChildAt(i);
                assertEquals(card.getText() + ": no word broken", android.text.Layout.HYPHENATION_FREQUENCY_NONE,
                        card.getHyphenationFrequency());
                moved |= card.capInCorner();
                if (card.capInCorner()) {
                    assertEquals("its name has the room the cap left", card.getPaddingStart(), card.getPaddingEnd());
                    assertTrue(card.getText() + " starts under the cap, not beside it",
                            card.getTotalPaddingTop() + card.getLayout().getLineTop(0) >= card.capFoot());
                }
            }
            assertTrue("Keyboard's cap went up into the corner", moved);
            View rail = rail(menu.sheet(), menu.modules().get(0));
            float[] corners = ((android.graphics.drawable.GradientDrawable) rail.getBackground()).getCornerRadii();
            float r = activity.getResources().getDimension(R.dimen.nj_radius_panel);
            assertEquals("the start corners are the dock's", r, corners[0], 0.01f);
            assertEquals(r, corners[7], 0.01f);
            assertEquals("the page side is square", 0f, corners[2], 0.01f);
            menu.sheet().dismiss();
        }
    }

    @Test
    public void theStreamsNoticeHeadsQuickAndThePcsEncoderLoadStandsOnPicture() throws Exception {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            StreamMenu.Options state = everything(NvApp.Role.STREAM);
            state.launchNotice = activity.getString(R.string.launch_warning_virtual_display_unavailable);
            List<StreamMenu.Item> chosen = new ArrayList<>();
            StreamMenu menu = open(activity, state, chosen, null);
            assertEquals(ControlCenterModel.QUICK, menu.module());
            TextView notice = menu.launchNotice();
            assertNotNull(notice);
            assertEquals("in the stream's own words", state.launchNotice, notice.getText().toString());
            assertSame("at the top of the page", notice, menu.sheet().page().content().getChildAt(0));
            assertFalse("a line, with nothing to press", notice.isClickable() || notice.isFocusable());
            assertNull("the encoder is the picture's", menu.encoderLoad());

            menu.select(ControlCenterModel.PICTURE);
            assertNull("the notice is Quick's", menu.launchNotice());
            assertNull("nothing while the PC reports no load", menu.encoderLoad());
            state.encoderLoad = "12.5";
            menu.refresh();
            TextView load = menu.encoderLoad();
            assertNotNull(load);
            assertEquals(activity.getString(R.string.hardware_gpu_encoder, "12.5"), load.getText().toString());
            assertTrue(load.getText().toString(), load.getText().toString().contains("12.5"));
            assertFalse("read, not pressed", load.isClickable() || load.isFocusable());
            assertTrue("with the page's rows", inScroll(load));
            state.encoderLoad = "40.0";
            menu.refresh();
            assertSame("the line is updated, not rebuilt", load, menu.encoderLoad());
            assertEquals(activity.getString(R.string.hardware_gpu_encoder, "40.0"), load.getText().toString());
            state.encoderLoad = null;
            menu.refresh();
            assertNull("gone when the PC stops reporting", menu.encoderLoad());
            assertEquals(ControlCenterModel.PICTURE, menu.module());
            menu.select(ControlCenterModel.QUICK);
            assertNotNull(menu.launchNotice());
            assertTrue(chosen.isEmpty());
            menu.sheet().dismiss();

            // A stream with nothing to say starts Quick with its cards.
            StreamMenu quiet = open(activity, everything(NvApp.Role.STREAM), chosen, null);
            assertNull(quiet.launchNotice());
            assertSame(quiet.grid(), quiet.sheet().page().content().getChildAt(0));
            quiet.sheet().dismiss();
        }
    }

    @Test
    public void aLaterFeaturesCardWaitsForThePcsGateAndThisClientOffersNoneToday() throws Exception {
        List<StreamMenuModel.Offer> offers = new ArrayList<>();
        for (String id : new String[] {"keyboard", "save_clip", "quick_suspend", "disconnect"}) {
            offers.add(new StreamMenuModel.Offer(id, null));
        }
        java.util.Set<String> none = Collections.emptySet();
        ControlCenterModel.Dock plain = ControlCenterModel.dock(offers, null, null, none);
        assertEquals(Arrays.asList(ControlCenterModel.QUICK), plain.modules());
        assertNull(plain.moduleOf("save_clip"));
        ControlCenterModel.Dock gated = ControlCenterModel.dock(offers, null, null, Collections.singleton("stream_dvr"));
        assertEquals(Arrays.asList(ControlCenterModel.QUICK, ControlCenterModel.CLIPS), gated.modules());
        assertEquals(ControlCenterModel.CLIPS, gated.moduleOf("save_clip"));
        assertNull("one feature opens only its own cards", gated.moduleOf("quick_suspend"));
        assertEquals(ControlCenterModel.CLIPS, gated.step(ControlCenterModel.QUICK, true));
        assertEquals(Arrays.asList(StreamMenuModel.CLOSE, "disconnect"), gated.footer);

        // No PC has these features yet and the stream offers none of their cards: whatever a PC says, no empty module.
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            StreamMenu.Options state = everything(NvApp.Role.STREAM);
            assertTrue("a stream names no gate by itself", state.gates.isEmpty());
            state.gates = new HashSet<>(Arrays.asList("stream_dvr", "hdr_sdr_tone_map", "frame_warp", "party_audio",
                    "input_deck", "quick_suspend"));
            StreamMenu menu = open(activity, state, new ArrayList<>(), null);
            assertNotNull("the model takes the gates", menu.dock());
            assertNull(menu.failure());
            assertFalse(menu.modules().contains(ControlCenterModel.CLIPS));
            for (String id : new String[] {"tone_map", "frame_warp", "party_mix", "save_clip", "record", "input_deck", "quick_suspend"}) {
                assertNull(id, menu.dock().moduleOf(id));
            }
            menu.sheet().dismiss();
        }
    }

    /** What a Control Center shows its colours as, under an owner of this theme. */
    private static int[] colours(int theme) throws Exception {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            activity.setTheme(theme);
            StreamMenu.Options state = everything(NvApp.Role.STREAM);
            state.overlayShown = true;
            StreamMenu menu = new StreamMenu(activity, () -> state, item -> { }, null, null, null);
            menu.show();
            settle();
            Context context = menu.sheet().getContext();
            MaterialButton card = menu.row(KEYBOARD), on = menu.row(OVERLAY);
            ControlCenterCards.Tint rest = (ControlCenterCards.Tint) card.getBackground();
            ControlCenterCards.Tint lit = (ControlCenterCards.Tint) on.getBackground();
            int[] colours = {
                    menu.sheet().surface().glass().fillColor(),
                    MaterialColors.getColor(context, com.google.android.material.R.attr.colorSurface, 0),
                    MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurface, 0),
                    MaterialColors.getColor(context, androidx.appcompat.R.attr.colorPrimary, 0),
                    card.getCurrentTextColor(), rest.fill(), rest.rim(), lit.fill(), lit.rim(),
                    menu.sheet().primary().getCurrentTextColor(),
                    ((TextView) menu.sheet().exits().getChildAt(0)).getCurrentTextColor(),
                    menu.sheet().subtitle().getCurrentTextColor(),
                    menu.reveal(HOST_STATUS).getCurrentTextColor(),
            };
            menu.sheet().dismiss();
            return colours;
        }
    }

    @Test @Config(qualifiers = "w640dp-h360dp-land-notnight-xxhdpi")
    public void underADayThemeItIsTheNightPalette() throws Exception {
        int[] day = colours(R.style.AppTheme), night = colours(R.style.StreamTheme);
        assertArrayEquals("the app's day theme changes nothing over a stream", night, day);
        Context app = RuntimeEnvironment.getApplication();
        assertEquals("the baked panel", ContextCompat.getColor(app, R.color.nj_surface_control_center_baked_fill), day[0]);
        assertTrue("a dark surface", ColorUtils.calculateLuminance(day[1]) < 0.1);
        assertTrue("light ink on it", ColorUtils.calculateLuminance(day[2]) > 0.6);
        assertEquals(ContextCompat.getColor(app, R.color.night_primary), day[3]);
        assertEquals(ContextCompat.getColor(app, R.color.nj_surface_control_center_label), day[4]);
        assertEquals(ContextCompat.getColor(app, R.color.nj_surface_control_center_card_fill), day[5]);
        assertEquals(ContextCompat.getColor(app, R.color.nj_surface_control_center_card_rim), day[6]);
        assertEquals("a switch that is on", ContextCompat.getColor(app, R.color.nj_surface_control_center_card_fill_on), day[7]);
        assertEquals(ContextCompat.getColor(app, R.color.nj_surface_control_center_card_rim_on), day[8]);
        assertTrue("a row's ink is light", ColorUtils.calculateLuminance(day[12]) > 0.6);
    }

    @Test
    public void highContrastDrawsItsCardsOnBlackInsideATwoDpRim() throws Exception {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putBoolean(AppTheme.HIGH_CONTRAST, true).commit();
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            StreamMenu.Options state = everything(NvApp.Role.STREAM);
            state.overlayShown = true;
            StreamMenu menu = open(activity, state, new ArrayList<>(), null);
            float density = activity.getResources().getDisplayMetrics().density;
            ControlCenterSurface surface = menu.sheet().surface();
            assertEquals(ControlCenterModel.SOLID, surface.kind());
            assertNull(surface.blurWindow());
            assertEquals("solid black", 0xFF000000, surface.glass().fillColor());
            assertEquals(2 * density, surface.glass().rimWidth(), 0.01f);
            ControlCenterCards.Tint rest = (ControlCenterCards.Tint) menu.row(KEYBOARD).getBackground();
            assertEquals("no tint over the black", 0, rest.fill());
            assertEquals(2 * density, rest.rimWidth(), 0.01f);
            assertEquals(ContextCompat.getColor(activity, R.color.nj_hc_color_rim), rest.rim());
            ControlCenterCards.Tint lit = (ControlCenterCards.Tint) menu.row(OVERLAY).getBackground();
            assertEquals("on is said by the rim", ContextCompat.getColor(activity, R.color.nj_hc_color_moon), lit.rim());
            assertTrue(menu.row(OVERLAY).getText().toString().endsWith("\n" + activity.getString(R.string.native_on)));
            menu.sheet().dismiss();
        }
    }

    @Test
    @Config(qualifiers = "w322dp-h792dp-port-xhdpi")
    @GraphicsMode(GraphicsMode.Mode.NATIVE)
    public void foldCoverScreenStaysSheetWithUncutCardLabelsAndCompactTypography() throws Exception {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            StreamMenu menu = open(activity, everything(NvApp.Role.STREAM), new ArrayList<>(), null);
            assertEquals("narrow upright cover stays sheet", ControlCenterModel.SHEET, menu.sheet().placement());
            assertTrue("cover screen under 600dp uses tight compact dock",
                    Boolean.TRUE.equals(ReflectionHelpers.getField(menu.sheet(), "compact")));
            View decor = menu.sheet().getWindow().getDecorView();
            float density = activity.getResources().getDisplayMetrics().density;
            float scaled = activity.getResources().getDisplayMetrics().scaledDensity;
            int wPx = Math.round(322 * density);
            int hPx = Math.round(792 * density);
            decor.measure(View.MeasureSpec.makeMeasureSpec(wPx, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(hPx, View.MeasureSpec.EXACTLY));
            decor.layout(0, 0, wPx, hPx);

            MaterialButton firstCard = menu.row(KEYBOARD);
            assertNotNull(firstCard.getLayout());
            assertEquals("card text fits without ellipsis on 322dp cover", 0,
                    firstCard.getLayout().getEllipsisCount(Math.max(0, firstCard.getLayout().getLineCount() - 1)));
            assertTrue("non-TV card label dialed down from 14sp", firstCard.getTextSize() / scaled <= 13.5f);
            android.text.Spanned spanned = (android.text.Spanned) menu.row(OVERLAY).getText();
            assertEquals("card detail text uses sp TextAppearance instead of dp AbsoluteSizeSpan", 0,
                    spanned.getSpans(0, spanned.length(), android.text.style.AbsoluteSizeSpan.class).length);
            ActionSheetRows.HeaderViews header = ReflectionHelpers.getField(menu.sheet(), "header");
            assertTrue("non-TV heading dialed down to ~16sp", header.heading.getTextSize() / scaled <= 16.5f);
            for (int i = 0; i < menu.sheet().exits().getChildCount(); i++) {
                MaterialButton exit = (MaterialButton) menu.sheet().exits().getChildAt(i);
                assertNotNull(exit.getLayout());
                assertEquals("exit button " + i + " fits without ellipsis", 0,
                        exit.getLayout().getEllipsisCount(0));
            }
            menu.sheet().dismiss();
        }
    }

    @Test
    @Config(qualifiers = "w673dp-h841dp-port-xhdpi")
    public void unfoldedFoldGetsWideRightDockWithExitIconsAndUpdatesOnResize() throws Exception {
        try (var controller = Robolectric.buildActivity(Activity.class).setup().visible()) {
            Activity activity = controller.get();
            StreamMenu menu = open(activity, everything(NvApp.Role.STREAM), new ArrayList<>(), null);
            ControlCenterDock dock = menu.sheet();
            float density = activity.getResources().getDisplayMetrics().density;
            assertEquals("unfolded 673dp fold gets right-edge Dock even when taller than wide",
                    ControlCenterModel.DOCK, dock.placement());
            assertFalse("673x841 fold does not force tight compact dock",
                    Boolean.TRUE.equals(ReflectionHelpers.getField(dock, "compact")));
            assertTrue("wide dock has exit icons", ((MaterialButton) dock.exits().getChildAt(0)).getIcon() != null);

            View decor = dock.getWindow().getDecorView();
            decor.measure(View.MeasureSpec.makeMeasureSpec(Math.round(673 * density), View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(Math.round(841 * density), View.MeasureSpec.EXACTLY));
            decor.layout(0, 0, decor.getMeasuredWidth(), decor.getMeasuredHeight());
            assertEquals("wide dock uses 408dp width", Math.round(408f * density),
                    dock.panel().getWidth(), 2f);
            int tallMinHeight = menu.row(KEYBOARD).getMinimumHeight();
            int tallCardHeight = menu.row(KEYBOARD).getHeight();

            dock.onResized(Math.round(322 * density), Math.round(792 * density));
            assertEquals("closing fold switches back to Sheet", ControlCenterModel.SHEET, dock.placement());
            assertTrue("narrow cover sets tight compact mode",
                    Boolean.TRUE.equals(ReflectionHelpers.getField(dock, "compact")));

            dock.onResized(Math.round(841 * density), Math.round(400 * density));
            assertEquals("low height landscape switches back to Dock", ControlCenterModel.DOCK, dock.placement());
            decor.measure(View.MeasureSpec.makeMeasureSpec(Math.round(841 * density), View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(Math.round(400 * density), View.MeasureSpec.EXACTLY));
            decor.layout(0, 0, decor.getMeasuredWidth(), decor.getMeasuredHeight());
            int lowMinHeight = menu.row(KEYBOARD).getMinimumHeight();
            int lowCardHeight = menu.row(KEYBOARD).getHeight();
            assertTrue("crossing <440dp height tightens minHeight (" + lowMinHeight + " < " + tallMinHeight + ")",
                    lowMinHeight < tallMinHeight);
            assertTrue("crossing <440dp height tightens laid-out cardHeight (" + lowCardHeight + " < " + tallCardHeight + ")",
                    lowCardHeight < tallCardHeight);
            dock.dismiss();
        }
    }
}
