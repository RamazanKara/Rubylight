package com.limelight;

import android.app.Application;
import android.content.ComponentName;
import android.content.Context;
import android.os.Looper;
import android.os.SystemClock;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowManager;
import android.view.inputmethod.InputMethodManager;
import android.widget.FrameLayout;

import androidx.core.graphics.Insets;
import androidx.core.view.WindowInsetsCompat;

import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.HostKeyboard;
import com.limelight.ui.HostTextField;
import com.limelight.ui.LegacyKeyboardInsets;
import com.limelight.ui.StreamView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowInputDevice;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.logging.Handler;
import java.util.logging.LogRecord;
import java.util.logging.Logger;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/** The soft keyboard that follows the host's text fields, as Game drives it. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = GameHostKeyboardTest.Bridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameHostKeyboardTest {
    /** No native engine; the host takes no keyboard layout report. */
    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class Bridge {
        @Implementation protected static void __staticInitializer__() { }
        @Implementation protected static boolean hostSupportsControlMessage(int messageType) { return false; }
    }

    @Test
    public void theKeyboardNeverResizesOrPansTheStreamWindow() throws Exception {
        Game game = Robolectric.buildActivity(Game.class).get();
        int softInputMode = game.getPackageManager().getActivityInfo(
                new ComponentName(game, Game.class), 0).softInputMode;
        // A resized window would resize the video surface; the stream moves up by itself instead.
        assertEquals(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_NOTHING,
                softInputMode & WindowManager.LayoutParams.SOFT_INPUT_MASK_ADJUST);
        assertEquals(WindowManager.LayoutParams.SOFT_INPUT_STATE_HIDDEN,
                softInputMode & WindowManager.LayoutParams.SOFT_INPUT_MASK_STATE);
    }

    @Test
    public void aRaiseRightAfterATapOpensTheKeyboard() {
        Game game = game();
        StreamView stream = stream(game);
        tap(game, stream, 100);
        raise(game, HostTextField.KIND_RAISE);
        assertTrue(imeShown(game));
        assertNotNull(stream.getHostTextField());
    }

    @Test
    public void aRaiseWithoutInputOnTheStreamLeavesTheKeyboardClosedButTypesTheField() {
        Game game = game();
        StreamView stream = stream(game);
        raise(game, HostTextField.KIND_RAISE);
        assertFalse("a game field that takes the focus by itself", imeShown(game));
        // Opened by hand, the keyboard still types into the field.
        assertNotNull(stream.getHostTextField());
    }

    @Test
    public void aRaiseLongAfterTheTapLeavesTheKeyboardClosed() {
        Game game = game();
        StreamView stream = stream(game);
        tap(game, stream, 100);
        ShadowSystemClock.advanceBy(Duration.ofMillis(HostKeyboard.INPUT_WINDOW_MS + 1));
        raise(game, HostTextField.KIND_RAISE);
        assertFalse(imeShown(game));
    }

    @Test
    public void aKeyOnTheStreamCountsAsTheUsersInput() {
        Game game = game();
        stream(game);
        game.handleKeyDown(keyboardKey(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_TAB));
        raise(game, HostTextField.KIND_UPDATE);
        assertTrue("the focus moved to another field after the key", imeShown(game));
    }

    @Test
    @Config(sdk = 36)
    public void theStreamMovesUpOverTheKeyboardWithoutResizingAndBackWhenItCloses() {
        Game game = game();
        StreamView stream = stream(game);
        tap(game, stream, 1800);
        raise(game, HostTextField.KIND_RAISE);
        HostKeyboard keyboard = ReflectionHelpers.getField(game.keyboardInput, "hostKeyboard");
        assertTrue(keyboard.hostOpened());

        insets(game, true, 1000);
        float margin = HostKeyboard.MARGIN_DP * game.getResources().getDisplayMetrics().density;
        assertEquals(1400 - margin - 1800, stream.getTranslationY(), 0.5f);
        assertEquals("the video surface keeps its size", 2400, stream.getHeight());
        assertEquals(1080, stream.getWidth());

        // The user closes it: the stream moves back, the keyboard is theirs now, and the field
        // does not reopen it after another tap.
        insets(game, false, 0);
        assertEquals(0, stream.getTranslationY(), 0);
        assertFalse(keyboard.hostOpened());
        tap(game, stream, 1800);
        assertTrue(keyboard.dismissedHere());
        assertFalse(keyboard.mayOpen(SystemClock.uptimeMillis()));
    }

    @Test
    @Config(sdk = 36)
    public void aNewConnectionMovesTheStreamBack() {
        Game game = game();
        StreamView stream = stream(game);
        tap(game, stream, 1800);
        raise(game, HostTextField.KIND_RAISE);
        insets(game, true, 1000);
        assertTrue(stream.getTranslationY() < 0);
        ReflectionHelpers.setField(game, "conn", null);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "requestTextInput");
        assertEquals(0, stream.getTranslationY(), 0);
    }

    /**
     * Android 10 tells the adjustNothing window nothing of the keyboard; the legacy probe's
     * measurement moves the stream up and back, and closing it is the user's dismissal, which
     * the host's element id keeps for that field only.
     */
    @Test
    public void onAndroid10TheLegacyProbeMovesTheStreamAndAClosedFieldStaysClosedByItsId() {
        Game game = game();
        StreamView stream = stream(game);
        LegacyKeyboardInsets legacy = new LegacyKeyboardInsets(game, (View) stream.getParent(),
                (visible, bottom) -> ReflectionHelpers.callInstanceMethod(game.keyboardInput, "onKeyboardInsets",
                        ClassParameter.from(boolean.class, visible), ClassParameter.from(int.class, bottom)));
        ReflectionHelpers.setField(game, "legacyKeyboard", legacy);
        HostKeyboard keyboard = ReflectionHelpers.getField(game.keyboardInput, "hostKeyboard");
        tap(game, stream, 1800);
        raise(game, HostTextField.KIND_RAISE, 7);
        assertTrue(keyboard.hostOpened());

        // The window's own insets say nothing below Android 11.
        insets(game, true, 1000);
        assertEquals(0, stream.getTranslationY(), 0);
        probe(game, legacy, 1400);
        float margin = HostKeyboard.MARGIN_DP * game.getResources().getDisplayMetrics().density;
        assertEquals(1400 - margin - 1800, stream.getTranslationY(), 0.5f);
        assertEquals("the video surface keeps its size", 2400, stream.getHeight());

        // The user closes it on element 7: back down, and the host repeating 7 leaves it closed.
        probe(game, legacy, 2400);
        assertEquals(0, stream.getTranslationY(), 0);
        assertFalse(keyboard.hostOpened());
        tap(game, stream, 1800);
        raise(game, HostTextField.KIND_UPDATE, 7);
        assertTrue(keyboard.dismissedHere());
        assertFalse(keyboard.hostOpened());
        // Another element right after a tap opens it.
        tap(game, stream, 1800);
        raise(game, HostTextField.KIND_UPDATE, 9);
        assertTrue(keyboard.hostOpened());
    }

    /**
     * The user closed the keyboard on element 7: the host lowering and raising 7 again, even
     * right after a tap, leaves it closed and says why; element 9 after a tap opens it.
     */
    @Test
    @Config(sdk = 36)
    public void aKeyboardClosedOnAnElementStaysClosedWhenTheHostRaisesItAgain() {
        Game game = game();
        StreamView stream = stream(game);
        HostKeyboard keyboard = ReflectionHelpers.getField(game.keyboardInput, "hostKeyboard");
        List<String> log = new ArrayList<>();
        Handler handler = new Handler() {
            @Override public void publish(LogRecord record) { log.add(record.getMessage()); }
            @Override public void flush() { }
            @Override public void close() { }
        };
        Logger logger = Logger.getLogger(LimeLog.class.getName());
        logger.addHandler(handler);
        try {
            tap(game, stream, 1800);
            raise(game, HostTextField.KIND_RAISE, 7);
            insets(game, true, 1000);
            insets(game, false, 0);
            assertFalse(keyboard.hostOpened());

            raise(game, HostTextField.KIND_LOWER, 0);
            tap(game, stream, 1800);
            raise(game, HostTextField.KIND_RAISE, 7);
            assertTrue(keyboard.dismissedHere());
            assertFalse(keyboard.hostOpened());
            assertTrue(log.toString(), log.contains("Keyboard stays closed: closed for this field"));

            tap(game, stream, 1800);
            raise(game, HostTextField.KIND_UPDATE, 9);
            assertFalse(keyboard.dismissedHere());
            assertTrue(keyboard.hostOpened());
        } finally {
            logger.removeHandler(handler);
        }
    }

    private static Game game() {
        Game game = GameKeyboardOverlayTest.game(new GameKeyboardOverlayTest.RecordingConnection());
        ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());
        ReflectionHelpers.setField(game, "keyboardTranslator", new KeyboardTranslator());
        game.setTheme(R.style.AppTheme);
        return game;
    }

    private static StreamView stream(Game game) {
        StreamView stream = new StreamView(game);
        stream.setFocusableInTouchMode(true);
        stream.setInputCallbacks(game);
        FrameLayout parent = new FrameLayout(game);
        parent.addView(stream, new FrameLayout.LayoutParams(1080, 2400));
        game.setContentView(parent);
        parent.measure(View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(2400, View.MeasureSpec.EXACTLY));
        parent.layout(0, 0, 1080, 2400);
        ReflectionHelpers.setField(game, "streamView", stream);
        return stream;
    }

    private static void tap(Game game, StreamView stream, float y) {
        long now = SystemClock.uptimeMillis();
        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, 500, y, 0);
        down.setSource(InputDevice.SOURCE_TOUCHSCREEN);
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "noteStreamPress",
                ClassParameter.from(View.class, stream), ClassParameter.from(MotionEvent.class, down));
        down.recycle();
        shadowOf(Looper.getMainLooper()).idle();
    }

    private static void raise(Game game, int kind) {
        raise(game, kind, 0);
    }

    private static void raise(Game game, int kind, int element) {
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "onHostTextField",
                ClassParameter.from(int.class, kind), ClassParameter.from(int.class, HostTextField.TYPE_TEXT),
                ClassParameter.from(int.class, 0), ClassParameter.from(int.class, element));
    }

    /** The legacy probe's window is visible down to {@code visibleBottom} of the 2400 px stream window. */
    private static void probe(Game game, LegacyKeyboardInsets legacy, int visibleBottom) {
        ReflectionHelpers.callInstanceMethod(legacy, "onFrame", ClassParameter.from(int.class, 2400),
                ClassParameter.from(int.class, visibleBottom), ClassParameter.from(int.class, 2400));
        shadowOf(Looper.getMainLooper()).idle();
    }

    private static void insets(Game game, boolean visible, int bottom) {
        WindowInsetsCompat insets = new WindowInsetsCompat.Builder()
                .setInsets(WindowInsetsCompat.Type.ime(), Insets.of(0, 0, 0, bottom))
                .setVisible(WindowInsetsCompat.Type.ime(), visible).build();
        ReflectionHelpers.callInstanceMethod(game.keyboardInput, "onWindowInsets",
                ClassParameter.from(WindowInsetsCompat.class, insets));
        shadowOf(Looper.getMainLooper()).idle();
    }

    private static boolean imeShown(Game game) {
        InputMethodManager input = (InputMethodManager) game.getSystemService(Context.INPUT_METHOD_SERVICE);
        return shadowOf(input).isSoftInputVisible();
    }

    private static KeyEvent keyboardKey(int action, int keyCode) {
        InputDevice device = ShadowInputDevice.makeInputDeviceNamed("Keyboard");
        ReflectionHelpers.setField(device, "mId", 31);
        ReflectionHelpers.setField(device, "mSources", InputDevice.SOURCE_KEYBOARD);
        ReflectionHelpers.setField(device, "mKeyboardType", InputDevice.KEYBOARD_TYPE_ALPHABETIC);
        ReflectionHelpers.setField(device, "mMotionRanges", new ArrayList<>());
        KeyEvent event = new KeyEvent(0, 0, action, keyCode, 0, 0, 31, 0, 0, InputDevice.SOURCE_KEYBOARD);
        shadowOf(event).setDevice(device);
        return event;
    }
}
