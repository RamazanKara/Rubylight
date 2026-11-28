package com.limelight.binding.input.pad;

import android.os.SystemClock;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.Game;
import com.limelight.R;
import com.limelight.binding.input.HeldInputState;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.AdaptiveLayout;
import com.limelight.ui.StreamView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/**
 * Integration tests covering Mobile FPS Touch Zones and Custom KBM Buttons across strict view
 * Z-order, 4-finger multi-touch isolation, simultaneous Look + Aim-while-holding-Fire, Hold vs Toggle
 * release semantics, Safe Fit 90<->270 asymmetric cutout flips, preset invariants across all 5
 * surfaces, and Studio editor round-trips.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 33)
public class PadFpsIntegrationTest {

    private static final class RecordingSink implements HeldInputState.Sink, ModernController.PointerSink {
        final Map<Integer, Boolean> keys = new HashMap<>();
        final Map<Integer, Boolean> mice = new HashMap<>();
        final List<int[]> moves = new ArrayList<>();
        final List<int[]> scrolls = new ArrayList<>();

        @Override
        public void key(int vk, boolean down) {
            keys.put(vk, down);
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
        public void button(int button, boolean down) {
            mice.put(button, down);
        }

        @Override
        public void scroll(int vertical, int horizontal) {
            scrolls.add(new int[] {vertical, horizontal});
        }
    }

    private static final class DummyConnection extends NvConnection {
        DummyConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }
    }

    private static ModernController setupGameController(RecordingSink sink, PadLayout seed) {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setTheme(R.style.AppTheme);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.width = 1920;
        prefs.height = 1080;
        prefs.fps = 60;
        prefs.onscreenController = true;
        prefs.onlyL3R3 = false;
        prefs.oscOpacity = 100;
        ReflectionHelpers.setField(game, "prefConfig", prefs);
        ReflectionHelpers.setField(game, "conn", new DummyConnection());
        ReflectionHelpers.setField(game, "app", new NvApp("FPSGame", 7, false));
        StreamView stream = new StreamView(game);
        FrameLayout parent = new FrameLayout(game);
        parent.addView(stream);
        parent.measure(View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY));
        parent.layout(0, 0, 1600, 720);
        ReflectionHelpers.setField(game, "streamView", stream);

        ReflectionHelpers.callInstanceMethod(game, "showOnscreenController");
        ModernController mc = ReflectionHelpers.getField(game, "modernController");
        if (seed != null) {
            mc.save(seed, false);
        }
        if (sink != null) {
            mc.setPointerSink(sink);
            ReflectionHelpers.setField(mc.heldInputs(), "sink", sink);
        }
        FrameLayout frame = mc.frame();
        layoutFrame(frame, 1600, 720);
        return mc;
    }

    private static void layoutFrame(FrameLayout frame, int w, int h) {
        frame.measure(View.MeasureSpec.makeMeasureSpec(w, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(h, View.MeasureSpec.EXACTLY));
        frame.layout(0, 0, w, h);
    }

    @Test public void emptyLayoutAddsZeroExtraOverlayViewsToPreserveChildCounts() {
        PadLayout clean = new PadLayout();
        ModernController mc = setupGameController(null, clean);
        assertTrue(mc.customOverlay().zoneViews().isEmpty());
        assertTrue(mc.customOverlay().buttonViews().isEmpty());
        mc.destroy();
    }

    @Test public void zOrderPlacesZonesBelowControlsAndCustomButtonsAboveBoth() {
        PadLayout fps = new PadLayout();
        PadFpsPresets.apply(fps, PadFpsPresets.Preset.FPS_KBM);
        RecordingSink sink = new RecordingSink();
        ModernController mc = setupGameController(sink, fps);
        FrameLayout frame = mc.frame();

        int maxZoneIndex = -1;
        int minControlIndex = Integer.MAX_VALUE;
        int maxControlIndex = -1;
        int minButtonIndex = Integer.MAX_VALUE;

        for (int i = 0; i < frame.getChildCount(); i++) {
            View child = frame.getChildAt(i);
            if (child instanceof PadTouchZoneView) {
                maxZoneIndex = Math.max(maxZoneIndex, i);
            } else if (child instanceof PadControlView || child instanceof PadReachLayer) {
                minControlIndex = Math.min(minControlIndex, i);
                maxControlIndex = Math.max(maxControlIndex, i);
            } else if (child instanceof PadCustomButtonView || child instanceof PadCustomButtonView.HaloLayer) {
                minButtonIndex = Math.min(minButtonIndex, i);
            }
        }

        assertTrue(maxZoneIndex >= 0);
        assertTrue(maxZoneIndex < minControlIndex);
        assertTrue(maxControlIndex < minButtonIndex);

        // Tapping a custom button positioned inside the right-half Look zone delivers the
        // touch to the button rather than starting a camera drag on the underlying zone.
        PadCustomButtonView fireView = mc.customOverlay().buttonViews().get(0);
        assertEquals("fire", fireView.button.id);
        float fx = (fireView.getLeft() + fireView.getRight()) / 2f;
        float fy = (fireView.getTop() + fireView.getBottom()) / 2f;
        long t = SystemClock.uptimeMillis();
        MotionEvent down = MotionEvent.obtain(t, t, MotionEvent.ACTION_DOWN, fx, fy, 0);
        frame.dispatchTouchEvent(down);
        down.recycle();
        assertTrue(Boolean.TRUE.equals(sink.mice.get((int) MouseButtonPacket.BUTTON_LEFT)));
        for (PadTouchZoneView zv : mc.customOverlay().zoneViews()) {
            assertFalse(zv.handler.isActive());
        }
        mc.destroy();
    }

    @Test public void multiTouchFourFingersAndSimultaneousLookPlusDragFireZeroJump() {
        PadLayout fps = new PadLayout();
        PadFpsPresets.apply(fps, PadFpsPresets.Preset.FPS_KBM);
        RecordingSink sink = new RecordingSink();
        ModernController mc = setupGameController(sink, fps);
        FrameLayout frame = mc.frame();

        List<PadTouchZoneView> zones = mc.customOverlay().zoneViews();
        PadTouchZoneView moveZone = zones.get(0);
        PadTouchZoneView lookZone = zones.get(1);
        PadCustomButtonView fireView = findButton(mc, "fire");
        PadCustomButtonView scopeView = findButton(mc, "scope");

        long t0 = 1000L;
        // Finger 0: Move zone
        MotionEvent mDown = MotionEvent.obtain(t0, t0, MotionEvent.ACTION_DOWN, 200f, 400f, 0);
        moveZone.dispatchTouchEvent(mDown);
        mDown.recycle();

        // Finger 1: Look zone
        MotionEvent lDown = twoFinger(t0, t0 + 5, MotionEvent.ACTION_POINTER_DOWN, 1, 900f, 360f);
        lookZone.dispatchTouchEvent(lDown);
        lDown.recycle();

        // Extra second pointer on already-active Move zone is ignored cleanly (pointer-ID isolation).
        MotionEvent strayMovePtr = twoFinger(t0, t0 + 8, MotionEvent.ACTION_POINTER_DOWN, 1, 220f, 420f);
        moveZone.dispatchTouchEvent(strayMovePtr);
        strayMovePtr.recycle();

        // Finger 2: Scope button (independent pointer)
        MotionEvent sDown = MotionEvent.obtain(t0 + 10, t0 + 10, MotionEvent.ACTION_DOWN,
                scopeView.getWidth() / 2f, scopeView.getHeight() / 2f, 0);
        scopeView.dispatchTouchEvent(sDown);
        sDown.recycle();

        // Finger 3: Fire button (dragAim = true)
        MotionEvent fDown = MotionEvent.obtain(t0 + 12, t0 + 12, MotionEvent.ACTION_DOWN,
                fireView.getWidth() / 2f, fireView.getHeight() / 2f, 0);
        fireView.dispatchTouchEvent(fDown);
        fDown.recycle();

        assertTrue(Boolean.TRUE.equals(sink.mice.get((int) MouseButtonPacket.BUTTON_RIGHT)));
        assertTrue(Boolean.TRUE.equals(sink.mice.get((int) MouseButtonPacket.BUTTON_LEFT)));
        assertTrue(moveZone.handler.isActive());
        assertTrue(lookZone.handler.isActive());

        // Move look finger with historical sample batch + release look finger -> Fire drag continues smoothly.
        MotionEvent lMove = twoFinger(t0, t0 + 25, MotionEvent.ACTION_MOVE, 1, 920f, 360f);
        lMove.addBatch(t0 + 30, new MotionEvent.PointerCoords[] {
                coords(200f, 400f), coords(940f, 360f)
        }, 0);
        lookZone.dispatchTouchEvent(lMove);
        lMove.recycle();
        assertFalse(sink.moves.isEmpty());

        MotionEvent lUp = twoFinger(t0, t0 + 35, MotionEvent.ACTION_POINTER_UP, 1, 940f, 360f);
        lookZone.dispatchTouchEvent(lUp);
        lUp.recycle();

        int movesAtHandoff = sink.moves.size();
        MotionEvent fMove = MotionEvent.obtain(t0 + 12, t0 + 45, MotionEvent.ACTION_MOVE,
                fireView.getWidth() / 2f + 16f, fireView.getHeight() / 2f, 0);
        fireView.dispatchTouchEvent(fMove);
        fMove.recycle();
        assertTrue("Fire drag continues aiming cleanly after Look finger lifts", sink.moves.size() > movesAtHandoff);
        assertTrue(Math.abs(sink.moves.get(sink.moves.size() - 1)[0]) < 150);

        // Cancel Move zone -> W/A/S/D released unconditionally.
        MotionEvent mMove = MotionEvent.obtain(t0, t0 + 50, MotionEvent.ACTION_MOVE, 200f, 300f, 0);
        moveZone.dispatchTouchEvent(mMove);
        mMove.recycle();
        assertTrue(Boolean.TRUE.equals(sink.keys.get(0x57)));
        MotionEvent mCancel = MotionEvent.obtain(t0, t0 + 55, MotionEvent.ACTION_CANCEL, 200f, 300f, 0);
        moveZone.dispatchTouchEvent(mCancel);
        mCancel.recycle();
        assertFalse(Boolean.TRUE.equals(sink.keys.get(0x57)));
        mc.destroy();
    }

    @Test public void holdVsToggleAndHideDetachCancelEditorReleaseAllHeldInputs() {
        PadLayout layout = new PadLayout();
        PadCustomButton hold = PadCustomButton.key("hold_k", 0x46, "F", 0.3f, 0.5f);
        hold.toggle = false;
        PadCustomButton toggle = PadCustomButton.key("tog_k", 0x51, "Q", 0.7f, 0.5f);
        toggle.toggle = true;
        layout.addCustomButton(hold);
        layout.addCustomButton(toggle);

        RecordingSink sink = new RecordingSink();
        ModernController mc = setupGameController(sink, layout);
        PadCustomButtonView holdView = findButton(mc, "hold_k");
        PadCustomButtonView toggleView = findButton(mc, "tog_k");

        long t = 2000L;
        // Toggle tap 1 latches down; tap 2 releases.
        tapView(toggleView, t);
        assertTrue(Boolean.TRUE.equals(sink.keys.get(0x51)));
        tapView(toggleView, t + 40);
        assertFalse(Boolean.TRUE.equals(sink.keys.get(0x51)));

        // Re-latch toggle, then hide view -> must release unconditionally.
        tapView(toggleView, t + 80);
        assertTrue(Boolean.TRUE.equals(sink.keys.get(0x51)));
        toggleView.setVisibility(View.GONE);
        assertFalse(Boolean.TRUE.equals(sink.keys.get(0x51)));
        toggleView.setVisibility(View.VISIBLE);

        // Hold button pressed then ACTION_CANCEL or entering editor -> must release.
        MotionEvent down = MotionEvent.obtain(t + 120, t + 120, MotionEvent.ACTION_DOWN, 10f, 10f, 0);
        holdView.dispatchTouchEvent(down);
        down.recycle();
        assertTrue(Boolean.TRUE.equals(sink.keys.get(0x46)));

        mc.startEditing();
        ShadowLooper.idleMainLooper();
        assertFalse(Boolean.TRUE.equals(sink.keys.get(0x46)));
        mc.finishEditing(false);
        mc.destroy();
    }

    @Test public void safeFitAsymmetricCutoutFlipRepositionsZonesEdgeToEdgeAndButtonsInsideSafeInsets() {
        PadLayout fps = new PadLayout();
        PadFpsPresets.apply(fps, PadFpsPresets.Preset.FPS_KBM);
        ModernController mc = setupGameController(null, fps);
        FrameLayout frame = mc.frame();
        AdaptiveLayout.SafeInsetsListener observer =
                (AdaptiveLayout.SafeInsetsListener) frame.getTag(R.id.pad_safe_insets_observer);
        frame.setPadding(88, 0, 0, 0);
        observer.onSafeInsetsChanged(88, 0, 0, 0);
        layoutFrame(frame, 1600, 720);

        PadTouchZoneView leftZone = mc.customOverlay().zoneViews().get(0);
        PadCustomButtonView scopeBtn = findButton(mc, "scope");
        int scopeLeftAt90 = scopeBtn.getLeft();
        FrameLayout.LayoutParams zLp1 = (FrameLayout.LayoutParams) leftZone.getLayoutParams();
        assertEquals(-88, zLp1.leftMargin); // negative margin keeps zone edge-to-edge despite padL=88
        assertEquals(0, leftZone.getLeft());

        // Flip 90 -> 270 degrees: same frame width/height (1600x720), cutout jumps from left to right.
        frame.setPadding(0, 0, 88, 0);
        observer.onSafeInsetsChanged(0, 0, 88, 0);
        layoutFrame(frame, 1600, 720);
        FrameLayout.LayoutParams zLp2 = (FrameLayout.LayoutParams) leftZone.getLayoutParams();
        assertEquals(0, zLp2.leftMargin);
        assertEquals(0, leftZone.getLeft());
        assertTrue("Custom button shifts horizontally with safe area when cutout flips sides",
                scopeBtn.getLeft() != scopeLeftAt90);
        mc.destroy();
    }

    @Test public void fpsPresetsHaveZeroOverlapAndMin48dpAcrossAllFiveSurfaces() {
        Object[][] surfaces = {
                {1600f, 720f, 2.0f},
                {720f, 1600f, 2.0f},
                {1280f, 800f, 1.0f},
                {2184f, 1968f, 2.625f},
                {1968f, 2184f, 2.625f}
        };
        float[] a = new float[4];
        float[] b = new float[4];
        for (PadFpsPresets.Preset preset : PadFpsPresets.Preset.values()) {
            PadLayout layout = new PadLayout();
            PadFpsPresets.apply(layout, preset);
            assertEquals(2, layout.touchZones().size());
            assertTrue(layout.customButtons().size() >= 7);
            for (Object[] s : surfaces) {
                float w = (Float) s[0], h = (Float) s[1], d = (Float) s[2];
                float minPx = 48f * d - 0.5f;
                List<float[]> boxes = new ArrayList<>();
                for (PadCustomButton btn : layout.customButtons()) {
                    float[] box = new float[4];
                    btn.bounds(w, h, d, box);
                    assertTrue(preset + " button " + btn.id + " width >= 48dp", box[2] >= minPx);
                    assertTrue(preset + " button " + btn.id + " height >= 48dp", box[3] >= minPx);
                    boxes.add(box);
                }
                for (PadLayout.Control control : PadLayout.Control.values()) {
                    if (control.steam || layout.spec(control).hidden) continue;
                    float[] box = new float[4];
                    layout.bounds(control, w, h, d, box);
                    assertTrue(preset + " control " + control + " width >= 48dp", box[2] >= minPx);
                    assertTrue(preset + " control " + control + " height >= 48dp", box[3] >= minPx);
                    boxes.add(box);
                }
                for (int i = 0; i < boxes.size(); i++) {
                    System.arraycopy(boxes.get(i), 0, a, 0, 4);
                    for (int j = i + 1; j < boxes.size(); j++) {
                        System.arraycopy(boxes.get(j), 0, b, 0, 4);
                        boolean overlap = a[0] < b[0] + b[2] - 0.5f && a[0] + a[2] > b[0] + 0.5f
                                && a[1] < b[1] + b[3] - 0.5f && a[1] + a[3] > b[1] + 0.5f;
                        assertFalse(preset + " items overlap at " + w + "x" + h + ": " + i + " vs " + j, overlap);
                    }
                }
            }
        }
    }

    @Test public void editorRoundTripsFpsZonesAndCustomButtonsAndSearchableKeyPickerWorks() {
        List<PadKeyPickerSheet.Entry> shiftMatches = PadKeyPickerSheet.filterKeyEntries("shift");
        assertTrue(shiftMatches.size() >= 2);
        List<PadKeyPickerSheet.Entry> hexMatches = PadKeyPickerSheet.filterKeyEntries("0xa2");
        assertFalse(hexMatches.isEmpty());
        assertEquals(0xA2, hexMatches.get(0).code);

        PadLayout empty = new PadLayout();
        ModernController mc = setupGameController(null, empty);
        mc.startEditing();
        ShadowLooper.idleMainLooper();
        PadEditor editor = ReflectionHelpers.getField(mc, "editor");
        editor.addTouchZone();
        editor.selectedZone().setSensitivityPercent(250);
        editor.selectedZone().invertY = true;
        editor.addCustomButton();
        editor.duplicateSelectedButton();
        assertEquals(2, editor.draft().customButtons().size());
        mc.finishEditing(true);
        ShadowLooper.idleMainLooper();

        PadLayout saved = mc.layout();
        assertEquals(1, saved.touchZones().size());
        assertEquals(250, saved.touchZones().get(0).sensitivityPercent);
        assertTrue(saved.touchZones().get(0).invertY);
        assertEquals(2, saved.customButtons().size());
        mc.destroy();
    }

    @Test public void editorPresetDispatchSupportsAllFivePresetsIncludingFpsKbmAndFpsGamepad() {
        PadLayout empty = new PadLayout();
        ModernController mc = setupGameController(null, empty);
        String[] presetLabels = mc.frame().getResources().getStringArray(R.array.pad_preset_names);
        assertEquals(5, presetLabels.length);

        mc.startEditing();
        ShadowLooper.idleMainLooper();
        PadEditor editor = ReflectionHelpers.getField(mc, "editor");
        for (int i = 0; i < presetLabels.length; i++) {
            editor.applyPresetChoice(i);
        }
        assertEquals(2, editor.draft().touchZones().size());
        assertEquals(PadTouchZone.Kind.LOOK_STICK, editor.draft().touchZones().get(1).kind);
        assertTrue(editor.draft().customButtons().size() >= 7);

        editor.applyPresetChoice(3); // FPS (Keyboard & mouse)
        assertEquals(PadTouchZone.Kind.LOOK_MOUSE, editor.draft().touchZones().get(1).kind);
        mc.finishEditing(true);
        ShadowLooper.idleMainLooper();
        assertEquals(PadTouchZone.Kind.LOOK_MOUSE, mc.layout().touchZones().get(1).kind);
        mc.destroy();
    }

    @Test public void dragAimButtonInLookStickModeDecaysRightStickBackToZeroOnRelease() {
        PadLayout fpsGamepad = new PadLayout();
        PadFpsPresets.apply(fpsGamepad, PadFpsPresets.Preset.FPS_GAMEPAD);
        ModernController mc = setupGameController(null, fpsGamepad);
        android.app.Activity host = Robolectric.buildActivity(android.app.Activity.class).setup().visible().get();
        host.setContentView(mc.frame(), new FrameLayout.LayoutParams(1600, 720));
        ShadowLooper.idleMainLooper();
        com.limelight.binding.input.LayoutGamepad gamepad = ReflectionHelpers.getField(mc, "gamepad");
        PadCustomButtonView fireView = findButton(mc, "fire");

        long t0 = 5000L;
        MotionEvent down = MotionEvent.obtain(t0, t0, MotionEvent.ACTION_DOWN,
                fireView.getWidth() / 2f, fireView.getHeight() / 2f, 0);
        fireView.dispatchTouchEvent(down);
        down.recycle();

        MotionEvent m1 = MotionEvent.obtain(t0, t0 + 16, MotionEvent.ACTION_MOVE,
                fireView.getWidth() / 2f + 24f, fireView.getHeight() / 2f, 0);
        fireView.dispatchTouchEvent(m1);
        m1.recycle();

        MotionEvent m2 = MotionEvent.obtain(t0, t0 + 32, MotionEvent.ACTION_MOVE,
                fireView.getWidth() / 2f + 80f, fireView.getHeight() / 2f, 0);
        fireView.dispatchTouchEvent(m2);
        m2.recycle();

        short deflectedRx = ReflectionHelpers.getField(gamepad, "sentRx");
        assertTrue("Fast drag-aim on fire deflects right stick in LOOK_STICK mode",
                Math.abs((int) deflectedRx) > 0);

        MotionEvent up = MotionEvent.obtain(t0, t0 + 40, MotionEvent.ACTION_UP,
                fireView.getWidth() / 2f + 80f, fireView.getHeight() / 2f, 0);
        fireView.dispatchTouchEvent(up);
        up.recycle();

        for (int i = 0; i < 40; i++) {
            ShadowLooper.idleMainLooper(20, java.util.concurrent.TimeUnit.MILLISECONDS);
        }
        short settledRx = ReflectionHelpers.getField(gamepad, "sentRx");
        short settledRy = ReflectionHelpers.getField(gamepad, "sentRy");
        assertEquals("Decay ticks must settle rightStickX back to 0", 0, (int) settledRx);
        assertEquals("Decay ticks must settle rightStickY back to 0", 0, (int) settledRy);
        mc.destroy();
    }

    @Test public void actionCancelOnDragAimButtonInLookStickZeroesRightStickImmediately() {
        PadLayout fpsGamepad = new PadLayout();
        PadFpsPresets.apply(fpsGamepad, PadFpsPresets.Preset.FPS_GAMEPAD);
        ModernController mc = setupGameController(null, fpsGamepad);
        com.limelight.binding.input.LayoutGamepad gamepad = ReflectionHelpers.getField(mc, "gamepad");
        PadTouchZoneHandler.AimForwarder aim = ReflectionHelpers.getField(mc.customOverlay(), "aimForwarder");
        PadCustomButtonView fireView = findButton(mc, "fire");

        long t0 = 6000L;
        MotionEvent down = MotionEvent.obtain(t0, t0, MotionEvent.ACTION_DOWN,
                fireView.getWidth() / 2f, fireView.getHeight() / 2f, 0);
        fireView.dispatchTouchEvent(down);
        down.recycle();

        MotionEvent m1 = MotionEvent.obtain(t0, t0 + 16, MotionEvent.ACTION_MOVE,
                fireView.getWidth() / 2f + 24f, fireView.getHeight() / 2f, 0);
        fireView.dispatchTouchEvent(m1);
        m1.recycle();

        MotionEvent m2 = MotionEvent.obtain(t0, t0 + 32, MotionEvent.ACTION_MOVE,
                fireView.getWidth() / 2f + 84f, fireView.getHeight() / 2f, 0);
        fireView.dispatchTouchEvent(m2);
        m2.recycle();

        short deflectedRx = ReflectionHelpers.getField(gamepad, "sentRx");
        assertTrue(Math.abs((int) deflectedRx) > 0);

        MotionEvent cancel = MotionEvent.obtain(t0, t0 + 40, MotionEvent.ACTION_CANCEL,
                fireView.getWidth() / 2f + 84f, fireView.getHeight() / 2f, 0);
        fireView.dispatchTouchEvent(cancel);
        cancel.recycle();

        short cancelledRx = ReflectionHelpers.getField(gamepad, "sentRx");
        short cancelledRy = ReflectionHelpers.getField(gamepad, "sentRy");
        assertFalse("ACTION_CANCEL must not enter LOOK_STICK decay", aim.isDecaying());
        assertEquals("ACTION_CANCEL must zero rightStickX immediately", 0, (int) cancelledRx);
        assertEquals("ACTION_CANCEL must zero rightStickY immediately", 0, (int) cancelledRy);
        mc.destroy();
    }

    @Test public void lookStickFollowedByMoveStickOrMoveWasdKeepsStickSlotsIsolated() {
        // Case A: LOOK_STICK added first, MOVE_STICK added second.
        PadLayout hybridStick = new PadLayout();
        hybridStick.addTouchZone(new PadTouchZone("look", PadTouchZone.Kind.LOOK_STICK, 0.5f, 0f, 1f, 1f));
        hybridStick.addTouchZone(new PadTouchZone("move", PadTouchZone.Kind.MOVE_STICK, 0f, 0f, 0.5f, 1f));
        PadCustomButton fire = PadCustomButton.mouse("fire", MouseButtonPacket.BUTTON_LEFT, "FIRE", 0.85f, 0.6f);
        fire.dragAim = true;
        hybridStick.addCustomButton(fire);

        ModernController mcA = setupGameController(null, hybridStick);
        com.limelight.binding.input.LayoutGamepad gamepadA = ReflectionHelpers.getField(mcA, "gamepad");
        PadTouchZoneView lookView = mcA.customOverlay().zoneViews().get(0);
        PadTouchZoneView moveStickView = mcA.customOverlay().zoneViews().get(1);

        long t0 = 7000L;
        MotionEvent lDown = MotionEvent.obtain(t0, t0, MotionEvent.ACTION_DOWN, 200f, 300f, 0);
        lookView.dispatchTouchEvent(lDown);
        lDown.recycle();
        MotionEvent lM1 = MotionEvent.obtain(t0, t0 + 16, MotionEvent.ACTION_MOVE, 224f, 300f, 0);
        lookView.dispatchTouchEvent(lM1);
        lM1.recycle();
        MotionEvent lM2 = MotionEvent.obtain(t0, t0 + 32, MotionEvent.ACTION_MOVE, 290f, 300f, 0);
        lookView.dispatchTouchEvent(lM2);
        lM2.recycle();

        short rxAfterLook = ReflectionHelpers.getField(gamepadA, "sentRx");
        short lxAfterLook = ReflectionHelpers.getField(gamepadA, "sentLx");
        assertTrue("LOOK_STICK deflects right stick", Math.abs((int) rxAfterLook) > 0);
        assertEquals("LOOK_STICK must not touch left stick", 0, (int) lxAfterLook);

        MotionEvent mDown = MotionEvent.obtain(t0 + 40, t0 + 40, MotionEvent.ACTION_DOWN, 200f, 300f, 0);
        moveStickView.dispatchTouchEvent(mDown);
        mDown.recycle();
        MotionEvent mMove = MotionEvent.obtain(t0 + 40, t0 + 56, MotionEvent.ACTION_MOVE, 350f, 300f, 0);
        moveStickView.dispatchTouchEvent(mMove);
        mMove.recycle();

        short lxAfterMove = ReflectionHelpers.getField(gamepadA, "sentLx");
        short rxAfterMove = ReflectionHelpers.getField(gamepadA, "sentRx");
        assertTrue("MOVE_STICK deflects left stick", Math.abs((int) lxAfterMove) > 0);
        assertEquals("MOVE_STICK must not clobber right stick", rxAfterLook, rxAfterMove);

        MotionEvent mCancel = MotionEvent.obtain(t0 + 40, t0 + 70, MotionEvent.ACTION_CANCEL, 350f, 300f, 0);
        moveStickView.dispatchTouchEvent(mCancel);
        mCancel.recycle();
        assertEquals(0, (int) (short) ReflectionHelpers.getField(gamepadA, "sentLx"));
        assertEquals("Canceling MOVE_STICK must not zero LOOK_STICK", rxAfterLook,
                (short) ReflectionHelpers.getField(gamepadA, "sentRx"));
        mcA.destroy();

        // Case B: LOOK_STICK added first, MOVE_WASD added second.
        PadLayout lookPlusWasd = new PadLayout();
        lookPlusWasd.addTouchZone(new PadTouchZone("look", PadTouchZone.Kind.LOOK_STICK, 0.5f, 0f, 1f, 1f));
        lookPlusWasd.addTouchZone(new PadTouchZone("wasd", PadTouchZone.Kind.MOVE_WASD, 0f, 0f, 0.5f, 1f));
        ModernController mcB = setupGameController(null, lookPlusWasd);
        com.limelight.binding.input.LayoutGamepad gamepadB = ReflectionHelpers.getField(mcB, "gamepad");
        PadTouchZoneView lookB = mcB.customOverlay().zoneViews().get(0);
        PadTouchZoneView wasdB = mcB.customOverlay().zoneViews().get(1);

        MotionEvent lbDown = MotionEvent.obtain(t0, t0, MotionEvent.ACTION_DOWN, 200f, 300f, 0);
        lookB.dispatchTouchEvent(lbDown);
        lbDown.recycle();
        MotionEvent lbM1 = MotionEvent.obtain(t0, t0 + 16, MotionEvent.ACTION_MOVE, 224f, 300f, 0);
        lookB.dispatchTouchEvent(lbM1);
        lbM1.recycle();
        MotionEvent lbM2 = MotionEvent.obtain(t0, t0 + 32, MotionEvent.ACTION_MOVE, 290f, 300f, 0);
        lookB.dispatchTouchEvent(lbM2);
        lbM2.recycle();
        short rxBeforeWasd = ReflectionHelpers.getField(gamepadB, "sentRx");
        assertTrue(Math.abs((int) rxBeforeWasd) > 0);

        wasdB.releaseInput();
        assertEquals("Releasing MOVE_WASD must preserve LOOK_STICK slot", rxBeforeWasd,
                (short) ReflectionHelpers.getField(gamepadB, "sentRx"));
        lookB.releaseInput();
        assertEquals(0, (int) (short) ReflectionHelpers.getField(gamepadB, "sentRx"));
        mcB.destroy();
    }

    @Test public void seekBarDpadStepInCustomInspectorCreatesUndoCheckpoint() {
        PadLayout empty = new PadLayout();
        ModernController mc = setupGameController(null, empty);
        mc.startEditing();
        ShadowLooper.idleMainLooper();
        PadEditor editor = ReflectionHelpers.getField(mc, "editor");
        editor.addTouchZone(); // adds LOOK_MOUSE zone (default sensitivity 100%)
        editor.finishEditing(true);
        ShadowLooper.idleMainLooper();

        mc.startEditing();
        ShadowLooper.idleMainLooper();
        editor = ReflectionHelpers.getField(mc, "editor");
        editor.selectZone(editor.draft().touchZones().get(0).id);
        ShadowLooper.idleMainLooper();
        assertFalse(editor.canUndo());

        PadEditorPanel panel = ReflectionHelpers.getField(editor, "panel");
        PadCustomInspector inspector = ReflectionHelpers.getField(panel, "customInspector");
        android.widget.SeekBar sensitivityBar = ReflectionHelpers.getField(inspector, "sensitivityBar");
        android.view.KeyEvent rightKey = new android.view.KeyEvent(
                android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_RIGHT);
        assertTrue(sensitivityBar.onKeyDown(android.view.KeyEvent.KEYCODE_DPAD_RIGHT, rightKey));
        ShadowLooper.idleMainLooper();

        assertTrue("D-pad SeekBar step must update sensitivity",
                editor.selectedZone().sensitivityPercent > PadTouchZone.DEFAULT_SENSITIVITY);
        assertTrue("D-pad SeekBar step must record an Undo checkpoint", editor.canUndo());
        editor.undo();
        assertEquals(PadTouchZone.DEFAULT_SENSITIVITY, editor.selectedZone().sensitivityPercent);
        mc.finishEditing(false);
        mc.destroy();
    }

    @Test public void applyZoneQuickPresetUsesGamepadSticksOnFpsGamepadAndMinimalLayouts() {
        PadLayout empty = new PadLayout();
        ModernController mc = setupGameController(null, empty);
        mc.startEditing();
        ShadowLooper.idleMainLooper();
        PadEditor editor = ReflectionHelpers.getField(mc, "editor");

        editor.applyPresetChoice(4); // FPS_GAMEPAD (unsplit controls off except SELECT/START + stick zones)
        editor.applyZoneQuickPreset(PadTouchZone.QuickPreset.SPLIT_40_60);
        assertEquals(2, editor.draft().touchZones().size());
        assertEquals(PadTouchZone.Kind.MOVE_STICK, editor.draft().touchZones().get(0).kind);
        assertEquals(PadTouchZone.Kind.LOOK_STICK, editor.draft().touchZones().get(1).kind);

        editor.applyPresetChoice(1); // MINIMAL (unsplit FACE + sticks, A hidden)
        editor.addTouchZone();
        assertEquals(1, editor.draft().touchZones().size());
        assertEquals(PadTouchZone.Kind.LOOK_STICK, editor.draft().touchZones().get(0).kind);
        editor.applyZoneQuickPreset(PadTouchZone.QuickPreset.SPLIT_50_50);
        assertEquals(PadTouchZone.Kind.MOVE_STICK, editor.draft().touchZones().get(0).kind);
        assertEquals(PadTouchZone.Kind.LOOK_STICK, editor.draft().touchZones().get(1).kind);

        mc.finishEditing(false);
        mc.destroy();
    }

    @Test public void lookStickRemainsFunctionalWhenThirdLookMouseZoneIsAdded() {
        PadLayout layout = new PadLayout();
        PadFpsPresets.apply(layout, PadFpsPresets.Preset.FPS_GAMEPAD);
        // Add a 3rd zone as LOOK_MOUSE after LOOK_STICK (mimics tapping "+ Touch zone" on FPS_GAMEPAD).
        layout.addTouchZone(new PadTouchZone("look_extra", PadTouchZone.Kind.LOOK_MOUSE, 0.25f, 0.25f, 0.75f, 0.75f));

        RecordingSink sink = new RecordingSink();
        ModernController mc = setupGameController(sink, layout);
        com.limelight.binding.input.LayoutGamepad gamepad = ReflectionHelpers.getField(mc, "gamepad");
        PadTouchZoneView lookStickView = mc.customOverlay().zoneViews().get(1);
        PadTouchZoneView lookMouseView = mc.customOverlay().zoneViews().get(2);
        PadCustomButtonView fireView = findButton(mc, "fire");

        long t0 = 8000L;
        MotionEvent lDown = MotionEvent.obtain(t0, t0, MotionEvent.ACTION_DOWN, 200f, 300f, 0);
        lookStickView.dispatchTouchEvent(lDown);
        lDown.recycle();
        MotionEvent lM1 = MotionEvent.obtain(t0, t0 + 16, MotionEvent.ACTION_MOVE, 224f, 300f, 0);
        lookStickView.dispatchTouchEvent(lM1);
        lM1.recycle();
        MotionEvent lM2 = MotionEvent.obtain(t0, t0 + 32, MotionEvent.ACTION_MOVE, 290f, 300f, 0);
        lookStickView.dispatchTouchEvent(lM2);
        lM2.recycle();
        short stickRx = ReflectionHelpers.getField(gamepad, "sentRx");
        assertTrue("LOOK_STICK must still drive right stick when a 3rd LOOK_MOUSE zone exists",
                Math.abs((int) stickRx) > 0);
        lookStickView.releaseInput();

        // Also drag LOOK_MOUSE zone -> emits mouse deltas without breaking fire drag-aim later.
        MotionEvent mDown = MotionEvent.obtain(t0 + 50, t0 + 50, MotionEvent.ACTION_DOWN, 100f, 100f, 0);
        lookMouseView.dispatchTouchEvent(mDown);
        mDown.recycle();
        MotionEvent mMove = MotionEvent.obtain(t0 + 50, t0 + 66, MotionEvent.ACTION_MOVE, 130f, 100f, 0);
        lookMouseView.dispatchTouchEvent(mMove);
        mMove.recycle();
        assertFalse("3rd LOOK_MOUSE zone emits pointer move deltas", sink.moves.isEmpty());
        lookMouseView.releaseInput();
        fireView.releaseInput();
        mc.destroy();
    }

    private static PadCustomButtonView findButton(ModernController mc, String id) {
        for (PadCustomButtonView bv : mc.customOverlay().buttonViews()) {
            if (id.equals(bv.button.id)) return bv;
        }
        throw new AssertionError("Missing custom button: " + id);
    }

    private static void tapView(View v, long t) {
        MotionEvent d = MotionEvent.obtain(t, t, MotionEvent.ACTION_DOWN, 10f, 10f, 0);
        v.dispatchTouchEvent(d);
        d.recycle();
        MotionEvent u = MotionEvent.obtain(t, t + 10, MotionEvent.ACTION_UP, 10f, 10f, 0);
        v.dispatchTouchEvent(u);
        u.recycle();
    }

    private static MotionEvent twoFinger(long downTime, long eventTime, int action, int ptrIdx, float x2, float y2) {
        MotionEvent.PointerProperties p0 = new MotionEvent.PointerProperties();
        p0.id = 0;
        p0.toolType = MotionEvent.TOOL_TYPE_FINGER;
        MotionEvent.PointerProperties p1 = new MotionEvent.PointerProperties();
        p1.id = 1;
        p1.toolType = MotionEvent.TOOL_TYPE_FINGER;
        MotionEvent.PointerCoords[] cs = {coords(200f, 400f), coords(x2, y2)};
        int masked = action == MotionEvent.ACTION_MOVE
                ? action
                : action | (ptrIdx << MotionEvent.ACTION_POINTER_INDEX_SHIFT);
        return MotionEvent.obtain(downTime, eventTime, masked, 2,
                new MotionEvent.PointerProperties[] {p0, p1}, cs, 0, 0, 1f, 1f, 0, 0, 0, 0);
    }

    private static MotionEvent.PointerCoords coords(float x, float y) {
        MotionEvent.PointerCoords c = new MotionEvent.PointerCoords();
        c.x = x;
        c.y = y;
        c.pressure = 1f;
        c.size = 1f;
        return c;
    }
}
