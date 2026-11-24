package com.limelight;

import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.StreamHotkeys;
import com.limelight.nvstream.input.KeyboardPacket;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.ui.HostKeyboard;
import com.limelight.ui.StreamMenuModel;
import com.limelight.binding.input.keyboard.PcKeyboardEngine;
import androidx.core.view.WindowInsetsCompat;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.preference.PreferenceManager;
import android.view.InputDevice;
import android.view.KeyCharacterMap;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.widget.Toast;
import java.util.HashMap;
import java.util.Map;

/** The stream's keyboard input: hardware and soft keyboards, modifier state, special keys, the PC keyboard and keyboard overlays and the host's text fields. */
final class GameKeyboardInput {
    private final Game game;
    // Whether a host text field may open the soft keyboard, whose it is, and how far the stream
    // moves up for it.
    private final HostKeyboard hostKeyboard = new HostKeyboard();
    private final GameHostTextInput hostTextInput;
    private final GamePcKeyboardInput pcKeyboard;
    // Controller keys the on-screen PC keyboard took on their way down, so their release goes to it too.
    final android.util.SparseBooleanArray pcKeyboardKeys = new android.util.SparseBooleanArray();
    // The pad whose stick or hat is steering the on-screen keyboard, or -1.
    int pcKeyboardAxisDevice = -1;

    GameKeyboardInput(Game game) {
        this.game = game;
        this.hostTextInput = new GameHostTextInput(game, hostKeyboard, this);
        this.pcKeyboard = new GamePcKeyboardInput(game, this);
    }

    void requestTextInput() {
        hostTextInput.requestTextInput();
    }

    void reportKeyboardLayout() {
        hostTextInput.reportKeyboardLayout();
    }

    boolean isSystemKeyboardVisible() {
        return hostTextInput.isSystemKeyboardVisible();
    }

    void onHostTextField(int kind, int inputType, int flags, int element) {
        hostTextInput.onHostTextField(kind, inputType, flags, element);
    }

    void noteStreamPress(View view, MotionEvent event) {
        hostTextInput.noteStreamPress(view, event);
    }

    void onWindowInsets(WindowInsetsCompat insets) {
        hostTextInput.onWindowInsets(insets);
    }

    void onKeyboardInsets(boolean visible, int imeBottom) {
        hostTextInput.onKeyboardInsets(visible, imeBottom);
    }

    void updateKeyboardPan() {
        hostTextInput.updateKeyboardPan();
    }

    private final Map<Short, Byte> pressedKeys = new HashMap<>();

    // How long a shortcut's keys stay down on the PC, as nightjar-policy's shortcuts::HOLD_MS: a chord pressed and
    // released in the same instant reaches the PC, but what reads the keys once a frame (the Steam overlay's
    // Shift + Tab, many games) never sees it held.
    static final long SHORTCUT_HOLD_MS = 100;
    private final Handler shortcutRelease = new Handler(Looper.getMainLooper());
    // The held shortcut's release, until it runs.
    private Runnable pendingShortcutRelease;

    private boolean waitingForAllModifiersUp = false;
    private int specialKeyCode = KeyEvent.KEYCODE_UNKNOWN;

    private final Map<Short, Integer> customPressedKeys = new HashMap<>();
    // The keys the modern controller's WASD stick holds. Like the classic touch keys, a key held here and by a
    // keyboard goes up on the PC only once both let go.
    private final java.util.Set<Short> stickPressedKeys = new java.util.HashSet<>();

    void setMetaKeyCaptureState(boolean enabled) {
        pcKeyboard.setMetaKeyCaptureState(enabled);
    }

    private static int getModifierMask(int androidKeyCode) {
        if (androidKeyCode == KeyEvent.KEYCODE_CTRL_LEFT ||
            androidKeyCode == KeyEvent.KEYCODE_CTRL_RIGHT) {
            return KeyboardPacket.MODIFIER_CTRL << (androidKeyCode == KeyEvent.KEYCODE_CTRL_RIGHT ? 8 : 0);
        }
        else if (androidKeyCode == KeyEvent.KEYCODE_SHIFT_LEFT ||
                 androidKeyCode == KeyEvent.KEYCODE_SHIFT_RIGHT) {
            return KeyboardPacket.MODIFIER_SHIFT << (androidKeyCode == KeyEvent.KEYCODE_SHIFT_RIGHT ? 8 : 0);
        }
        else if (androidKeyCode == KeyEvent.KEYCODE_ALT_LEFT ||
                 androidKeyCode == KeyEvent.KEYCODE_ALT_RIGHT) {
            return KeyboardPacket.MODIFIER_ALT << (androidKeyCode == KeyEvent.KEYCODE_ALT_RIGHT ? 8 : 0);
        }
        else if (androidKeyCode == KeyEvent.KEYCODE_META_LEFT ||
                androidKeyCode == KeyEvent.KEYCODE_META_RIGHT) {
            return KeyboardPacket.MODIFIER_META << (androidKeyCode == KeyEvent.KEYCODE_META_RIGHT ? 8 : 0);
        }
        else {
            return 0;
        }
    }

    // Returns true if the key stroke was consumed
    private boolean handleSpecialKeys(int androidKeyCode, boolean down) {
        int modifierMask = getModifierMask(androidKeyCode);
        int nonModifierKeyCode = modifierMask == 0 ? androidKeyCode : KeyEvent.KEYCODE_UNKNOWN;

        if (down) {
            game.modifierFlags |= modifierMask;
        }
        else {
            game.modifierFlags &= ~modifierMask;
        }

        // Handle the special combos on the key up
        if (waitingForAllModifiersUp || specialKeyCode != KeyEvent.KEYCODE_UNKNOWN) {
            if (specialKeyCode == androidKeyCode) {
                // If this is a key up for the special key itself, eat that because the host never saw the original key down
                return true;
            }
            else if (game.modifierFlags != 0) {
                // While we're waiting for modifiers to come up, eat all key downs and allow all key ups to pass
                return down;
            }
            else {
                // When all modifiers are up, perform the special action
                StreamHotkeys.Match match = specialHotkey;
                specialHotkey = null;
                if (match != null) runHotkey(match);

                // Reset special key state
                specialKeyCode = KeyEvent.KEYCODE_UNKNOWN;
                waitingForAllModifiersUp = false;
            }
        }
        // A stream hotkey: Ctrl, Alt or Shift held and a key the user's chord names, matched as the PC would see it
        else if (down && nonModifierKeyCode != KeyEvent.KEYCODE_UNKNOWN &&
                (getModifierState() & (KeyboardPacket.MODIFIER_CTRL | KeyboardPacket.MODIFIER_ALT | KeyboardPacket.MODIFIER_SHIFT)) != 0) {
            StreamHotkeys.Match match = hotkeys().at(getModifierState(), KeyboardTranslator.translateKeyCode(androidKeyCode) & 0xFF);
            if (match == null) {
                // This isn't a special combo that we consume on the client side
                return false;
            }
            // Remember that a special key combo was activated, so we can consume all key
            // events until the modifiers come up
            specialKeyCode = androidKeyCode;
            specialHotkey = match;
            waitingForAllModifiersUp = true;
            breakLoneModifierTap();
            return true;
        }

        // Not a special combo
        return false;
    }

    // The stream's hotkeys, read once per stream from the table desktop Nightjar shares and the user's changes.
    private StreamHotkeys.Table hotkeys;
    private StreamHotkeys.Match specialHotkey;

    private StreamHotkeys.Table hotkeys() {
        if (hotkeys == null) {
            try {
                hotkeys = new StreamHotkeys.Table(StreamHotkeys.load(PreferenceManager.getDefaultSharedPreferences(game)));
            } catch (StreamMenuModel.Refused e) {
                // Said once; the stream menu stays on Back, the floating button and the controller chord.
                LimeLog.severe("Stream hotkeys unavailable: " + e.getMessage());
                game.messages.alert(game.getString(R.string.stream_hotkeys_unavailable, e.getMessage()), Toast.LENGTH_LONG);
                hotkeys = StreamHotkeys.Table.EMPTY;
            }
        }
        return hotkeys;
    }

    private void runHotkey(StreamHotkeys.Match match) {
        switch (match.hotkey.id) {
            case StreamHotkeys.CAPTURE: {
                Handler h = game.getWindow().getDecorView().getHandler();
                if (h != null) {
                    h.postDelayed(game.toggleGrab, 250);
                }
                break;
            }
            case StreamHotkeys.QUIT:
                game.menuActions.leaveStream();
                break;
            case StreamHotkeys.STREAM_MENU:
                game.showStreamMenuFromController();
                break;
            case StreamHotkeys.PC_KEYBOARD:
                togglePcKeyboard();
                break;
            case StreamHotkeys.OVERLAY:
                game.overlays.togglePerformanceOverlay();
                break;
            case StreamHotkeys.CURSOR:
                game.touchInput.toggleCursorVisibility();
                break;
            default:
                LimeLog.warning("No handler for stream hotkey " + match.hotkey.id);
                return;
        }
        // An old chord (Ctrl+Alt+Shift+M for the menu) still works this release, and says once where it moved.
        if (match.hotkey.chord != null && match.firstOldUse(PreferenceManager.getDefaultSharedPreferences(game))) {
            game.messages.alert(game.getString(R.string.stream_hotkey_moved, StreamHotkeys.label(game, match.old),
                    StreamHotkeys.label(game, match.hotkey.chord)), Toast.LENGTH_LONG);
        }
    }

    // The host has already seen Ctrl, Alt and Shift go down for a client-side combo like Ctrl+Alt+Shift+K, and
    // sees them go up. Windows reads Alt or Ctrl+Shift going up with nothing between as a tap (the menu bar,
    // the input language switch), so put a key it ignores between them.
    private void breakLoneModifierTap() {
        if (!game.grabbedInput || !game.connected || game.conn == null || !game.conn.canSendInput()) return;
        short neutral = (short) (0x8000 | PcKeyboardEngine.NEUTRAL_VK);
        byte modifiers = getModifierState();
        sendKeyboardInput(neutral, KeyboardPacket.KEY_DOWN, modifiers, (byte) 0);
        sendKeyboardInput(neutral, KeyboardPacket.KEY_UP, modifiers, (byte) 0);
    }

    // We cannot simply use modifierFlags for all key event processing, because
    // some IMEs will not generate real key events for pressing Shift. Instead
    // they will simply send key events with isShiftPressed() returning true,
    // and we will need to send the modifier flag ourselves.
    private byte getModifierState(KeyEvent event) {
        // Start with the global modifier state to ensure we cover the case
        // detailed in https://github.com/moonlight-stream/moonlight-android/issues/840
        byte modifier = getModifierState();
        if (event.isShiftPressed()) {
            modifier |= KeyboardPacket.MODIFIER_SHIFT;
        }
        if (event.isCtrlPressed()) {
            modifier |= KeyboardPacket.MODIFIER_CTRL;
        }
        if (KeyboardTranslator.altHeld(event.getMetaState(), game.prefConfig.rightAltAsMeta)) {
            modifier |= KeyboardPacket.MODIFIER_ALT;
        }
        if (event.isMetaPressed()) {
            modifier |= KeyboardPacket.MODIFIER_META;
        }
        return applyKeySpecificModifiers(event.getKeyCode(), modifier);
    }

    private byte getModifierState(int keyCode) {
        return applyKeySpecificModifiers(keyCode, getModifierState());
    }

    private byte applyKeySpecificModifiers(int keyCode, byte modifier) {
        if (keyCode == KeyEvent.KEYCODE_PLUS) {
            // The host protocol has a single US =/+ virtual key, so Android's semantic plus key needs Shift.
            modifier |= KeyboardPacket.MODIFIER_SHIFT;
        }
        return modifier;
    }

    private byte getModifierState() {
        return (byte) (game.modifierFlags | (game.modifierFlags >> 8));
    }

    // The keyboard options can turn a physical Back or Right Alt into the Windows key, and sending
    // system keys turns a keyboard's Back (the Esc key of many keyboards made for Android) into Esc.
    // Physical Back opens stream menu is an explicit choice, so it keeps keyboard Back.
    private int physicalKeyCode(KeyEvent event) {
        int keyCode = event.getKeyCode();
        if (keyCode == KeyEvent.KEYCODE_DPAD_CENTER && ControllerHandler.isRemoteControlEvent(event)) {
            return KeyEvent.KEYCODE_ENTER;
        }
        boolean backAsEscape = game.prefConfig.sendsSystemKeys(game.inWindow()) && !game.prefConfig.backOpensMenu;
        boolean keyboardBack = false;
        if (keyCode == KeyEvent.KEYCODE_BACK && (game.prefConfig.backAsMeta || backAsEscape)) {
            InputDevice device = event.getDevice();
            keyboardBack = device != null && KeyboardTranslator.isKeyboardBack(keyCode,
                    (event.getFlags() & (KeyEvent.FLAG_VIRTUAL_HARD_KEY | KeyEvent.FLAG_FALLBACK)) != 0,
                    device.isVirtual(), device.getKeyboardType() == InputDevice.KEYBOARD_TYPE_ALPHABETIC,
                    ControllerHandler.isGameControllerDevice(device), event.getSource());
            // Esc only from an external keyboard: a keyboard phone's own keyboard may have no Ctrl
            // for Ctrl+Alt+Shift+M, and below Android 10 we cannot tell, so Back stays Back there.
            backAsEscape &= keyboardBack && Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && device.isExternal();
        }
        return KeyboardTranslator.remapPhysicalKey(keyCode, keyboardBack, game.prefConfig.backAsMeta, backAsEscape,
                game.prefConfig.rightAltAsMeta);
    }

    private boolean handleRemoteMenuKey(KeyEvent event) {
        if (!ControllerHandler.isRemoteControlEvent(event)) return false;
        int key = event.getKeyCode();
        if (key != KeyEvent.KEYCODE_BACK && key != KeyEvent.KEYCODE_MENU) return false;
        // These are local navigation on a TV remote, even while PC input is captured.
        if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) {
            if (key == KeyEvent.KEYCODE_BACK) game.onBack(true);
            else game.showStreamMenuFromController();
        }
        return true;
    }

    private boolean handlePhysicalBackMenuKey(KeyEvent event) {
        if (!game.connected || !game.prefConfig.backOpensMenu || event.getKeyCode() != KeyEvent.KEYCODE_BACK) return false;
        // A handheld's dedicated Back can share its gamepad device. Reserve that physical key
        // before controller remapping, while keeping Select/Start and keyboard Windows-key mode.
        if (physicalKeyCode(event) != KeyEvent.KEYCODE_BACK) return false;
        // Back as everywhere else: an open system keyboard (or the layout editor) goes first, then the menu.
        if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) game.onBack(true);
        return true;
    }

    boolean handleKeyDown(KeyEvent event) {
        if (handleRemoteMenuKey(event)) return true;
        // A key the keyboard took keeps being the keyboard's after it closed, so its repeats stay away from the game.
        if ((isPcKeyboardVisible() || pcKeyboardKeys.get(event.getKeyCode())) && handlePcKeyboardKey(event)) return true;
        // Pass-through virtual navigation keys
        if ((event.getFlags() & KeyEvent.FLAG_VIRTUAL_HARD_KEY) != 0) {
            return false;
        }

        // Handle a synthetic back button event that some Android OS versions
        // create as a result of a right-click. This event WILL repeat if
        // the right mouse button is held down, so we ignore those.
        int eventSource = event.getSource();
        if ((eventSource == InputDevice.SOURCE_MOUSE ||
                eventSource == InputDevice.SOURCE_MOUSE_RELATIVE) &&
                event.getKeyCode() == KeyEvent.KEYCODE_BACK) {

            // Like other mouse input, synthetic Back must respect capture. If mouse
            // navigation buttons are enabled, handleMotionEvent() handles the click.
            if (game.grabbedInput && !game.prefConfig.mouseNavButtons) {
                game.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_RIGHT);
            }

            // Always return true, otherwise the back press will be propagated
            // up to the parent and finish the activity.
            return true;
        }

        if (handlePhysicalBackMenuKey(event)) return true;
        // Back as Guide for a handheld's Back that is not on its gamepad device
        if (game.connected && game.controllerHandler != null && game.controllerHandler.handleHandheldBackAsGuide(event)) return true;

        boolean handled = false;

        if (!ControllerHandler.isRemoteControlEvent(event) && ControllerHandler.isGameControllerDevice(event.getDevice())) {
            // Always try the controller handler first, unless it's an alphanumeric keyboard device.
            // Otherwise, controller handler will eat keyboard d-pad events.
            handled = game.controllerHandler.handleButtonDown(event);
        }

        // Try the keyboard handler if it wasn't handled as a game controller
        if (!handled) {
            int keyCode = physicalKeyCode(event);

            // Let this method take duplicate key down events
            if (handleSpecialKeys(keyCode, true)) {
                return true;
            }

            // Pass through keyboard input if we're not grabbing
            if (!game.grabbedInput) {
                return false;
            }
            hostKeyboard.onInput(SystemClock.uptimeMillis());

            // We'll send it as a raw key event if we have a key mapping, otherwise we'll send it
            // as UTF-8 text (if it's a printable character).
            short translated = game.keyboardTranslator.translate(keyCode, event.getDeviceId());
            if (translated == 0) {
                // Make sure it has a valid Unicode representation and it's not a dead character
                // (which we don't support). If those are true, we can send it as UTF-8 text.
                //
                // NB: We need to be sure this happens before the getRepeatCount() check because
                // UTF-8 events don't auto-repeat on the host side.
                int unicodeChar = event.getUnicodeChar();
                String text = KeyboardTranslator.textForCodePoint(unicodeChar);
                if (text != null) {
                    game.conn.sendUtf8Text(text);
                    return true;
                }

                return false;
            }

            // Eat repeat down events
            if (event.getRepeatCount() > 0) {
                return true;
            }

            sendKeyboardInput(translated, KeyboardPacket.KEY_DOWN, getModifierState(event),
                    game.keyboardTranslator.hasNormalizedMapping(keyCode, event.getDeviceId()) ? 0 : MoonBridge.SS_KBE_FLAG_NON_NORMALIZED);
        }

        return true;
    }

    boolean handleKeyUp(KeyEvent event) {
        if (handleRemoteMenuKey(event)) return true;
        if ((pcKeyboardKeys.size() > 0 || isPcKeyboardVisible()) && handlePcKeyboardKey(event)) return true;
        // Pass-through virtual navigation keys
        if ((event.getFlags() & KeyEvent.FLAG_VIRTUAL_HARD_KEY) != 0) {
            return false;
        }

        // Handle a synthetic back button event that some Android OS versions
        // create as a result of a right-click.
        int eventSource = event.getSource();
        if ((eventSource == InputDevice.SOURCE_MOUSE ||
                eventSource == InputDevice.SOURCE_MOUSE_RELATIVE) &&
                event.getKeyCode() == KeyEvent.KEYCODE_BACK) {

            // Releasing capture already releases held mouse buttons. Ignore any
            // late synthetic Back release, while still consuming Android navigation.
            if (game.grabbedInput && !game.prefConfig.mouseNavButtons) {
                game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
            }

            // Always return true, otherwise the back press will be propagated
            // up to the parent and finish the activity.
            return true;
        }

        if (handlePhysicalBackMenuKey(event)) return true;
        // Back as Guide for a handheld's Back that is not on its gamepad device
        if (game.connected && game.controllerHandler != null && game.controllerHandler.handleHandheldBackAsGuide(event)) return true;

        boolean handled = false;
        if (!ControllerHandler.isRemoteControlEvent(event) && ControllerHandler.isGameControllerDevice(event.getDevice())) {
            // Always try the controller handler first, unless it's an alphanumeric keyboard device.
            // Otherwise, controller handler will eat keyboard d-pad events.
            handled = game.controllerHandler.handleButtonUp(event);
        }

        // Try the keyboard handler if it wasn't handled as a game controller
        if (!handled) {
            int keyCode = physicalKeyCode(event);
            if (handleSpecialKeys(keyCode, false)) {
                return true;
            }

            // Pass through keyboard input if we're not grabbing
            if (!game.grabbedInput) {
                return false;
            }

            short translated = game.keyboardTranslator.translate(keyCode, event.getDeviceId());
            if (translated == 0) {
                // If we sent this event as UTF-8 on key down, also report that it was handled
                // when we get the key up event for it.
                int unicodeChar = event.getUnicodeChar();
                return (unicodeChar & KeyCharacterMap.COMBINING_ACCENT) == 0 && (unicodeChar & KeyCharacterMap.COMBINING_ACCENT_MASK) != 0;
            }

            sendKeyboardInput(translated, KeyboardPacket.KEY_UP, getModifierState(event),
                    game.keyboardTranslator.hasNormalizedMapping(keyCode, event.getDeviceId()) ? 0 : MoonBridge.SS_KBE_FLAG_NON_NORMALIZED);
        }

        return true;
    }

    boolean handleKeyMultiple(KeyEvent event) {
        // We can receive keys from a software keyboard that don't correspond to any existing
        // KEYCODE value. Android will give those to us as an ACTION_MULTIPLE KeyEvent.
        //
        // Despite the fact that the Android docs say this is unused since API level 29, these
        // events are still sent as of Android 13 for the above case.
        //
        // For other cases of ACTION_MULTIPLE, we will not report those as handled so hopefully
        // they will be passed to us again as regular singular key events.
        if (event.getKeyCode() != KeyEvent.KEYCODE_UNKNOWN) {
            return false;
        }

        return game.handleTextInput(event.getCharacters());
    }

    boolean handleTextInput(CharSequence text) {
        if (!game.grabbedInput || game.conn == null || !game.conn.canSendInput()) {
            return false;
        }
        return KeyboardTranslator.sendTextInput(text, game.conn::sendUtf8Text,
                (key, direction) -> sendKeyboardInput(key, direction, (byte) 0,
                        MoonBridge.SS_KBE_FLAG_NON_NORMALIZED));
    }

    void toggleKeyboard() {
        toggleKeyboard(false);
    }

    void toggleKeyboard(boolean openBeforeMenu) {
        hostTextInput.toggleKeyboard(openBeforeMenu);
    }

    void togglePcKeyboard() {
        togglePcKeyboard(false);
    }

    void togglePcKeyboard(boolean systemKeyboardBeforeMenu) {
        pcKeyboard.togglePcKeyboard(systemKeyboardBeforeMenu);
    }

    boolean isPcKeyboardVisible() {
        return pcKeyboard.isPcKeyboardVisible();
    }

    void hidePcKeyboard() {
        pcKeyboard.hidePcKeyboard();
    }

    /**
     * A key of the modern controller's WASD stick, a Windows virtual-key code in the PC keyboard's form. It reaches
     * the PC only when no keyboard holds the same key, so either one letting go leaves it down while the other still
     * holds it; {@link #releaseKeyboardInput} lets it go with every other held key.
     */
    void sendOnScreenKey(int vk, boolean down) {
        if (!game.connected || game.conn == null || !game.conn.canSendInput()) return;
        short key = (short) (0x8000 | vk);
        synchronized (pressedKeys) {
            boolean changed = down ? stickPressedKeys.add(key) : stickPressedKeys.remove(key);
            if (changed && !pressedKeys.containsKey(key) && !customPressedKeys.containsKey(key)) {
                game.conn.sendKeyboardInput(key, down ? KeyboardPacket.KEY_DOWN : KeyboardPacket.KEY_UP,
                        (byte) (getModifierState() | customModifierState()), (byte) 0);
            }
        }
    }

    void sendPcKeyboardKey(int vk, boolean down, byte modifiers) {
        if (!game.connected || game.conn == null || !game.conn.canSendInput()) return;
        sendKeyboardInput((short) (0x8000 | vk), down ? KeyboardPacket.KEY_DOWN : KeyboardPacket.KEY_UP, modifiers, (byte) 0);
    }

    void closeSystemKeyboard() {
        hostTextInput.closeSystemKeyboard();
    }

    void hideSystemKeyboard() {
        hostTextInput.hideSystemKeyboard();
    }

    private boolean handlePcKeyboardKey(KeyEvent event) {
        return pcKeyboard.handlePcKeyboardKey(event);
    }

    void releasePcKeyboardPads() {
        pcKeyboard.releasePcKeyboardPads();
    }

    void handlePcKeyboardMotion(MotionEvent event) {
        pcKeyboard.handlePcKeyboardMotion(event);
    }

    void toggleKeyboardOverlay() {
        pcKeyboard.toggleKeyboardOverlay();
    }

    boolean isKeyboardOverlayVisible() {
        return pcKeyboard.isKeyboardOverlayVisible();
    }

    void hideKeyboardOverlay() {
        pcKeyboard.hideKeyboardOverlay();
    }

    /**
     * A shortcut chosen in the stream menu, run once the menu has closed: input is captured again for it, and a shortcut
     * that still cannot reach the PC says why instead of doing nothing.
     */
    void sendMenuShortcut(int... keys) {
        if (!game.foreground || !game.connected) return;
        if (!game.conn.canSendInput()) {
            android.widget.Toast.makeText(game, R.string.stream_shortcut_no_keyboard, android.widget.Toast.LENGTH_SHORT).show();
            return;
        }
        if (!game.grabbedInput) game.touchInput.setInputGrabState(true);
        if (!sendKeyboardShortcut(keys)) {
            android.widget.Toast.makeText(game, R.string.stream_shortcut_not_sent, android.widget.Toast.LENGTH_SHORT).show();
        }
    }

    /**
     * Presses {@code keys} on the PC, modifiers first whatever order they were saved in (older saved shortcuts kept
     * the typed order), holds them for {@link #SHORTCUT_HOLD_MS} and releases them in reverse; false when the stream
     * cannot take keys now.
     */
    boolean sendKeyboardShortcut(int... keys) {
        if (!game.foreground || !game.connected || !game.grabbedInput || game.streamMenu != null || !game.conn.canSendInput()) {
            return false;
        }
        // A shortcut still held goes up first, so this one is pressed in full.
        releaseShortcutNow();
        keys = modifiersFirst(keys);
        byte modifiers = getModifierState();
        int[] sent = new int[keys.length];
        int count = 0;
        for (int key : keys) {
            short code = game.keyboardTranslator.translate(key, -1);
            int mask = getModifierMask(key);
            modifiers |= (byte) (mask | (mask >> 8));
            if (!pressedKeys.containsKey(code)) {
                sendKeyboardInput(code, KeyboardPacket.KEY_DOWN, modifiers, (byte) 0);
                sent[count++] = key;
            }
        }
        int[] held = java.util.Arrays.copyOf(sent, count);
        pendingShortcutRelease = () -> {
            pendingShortcutRelease = null;
            releaseShortcut(held);
        };
        shortcutRelease.postDelayed(pendingShortcutRelease, SHORTCUT_HOLD_MS);
        return true;
    }

    private void releaseShortcutNow() {
        Runnable release = pendingShortcutRelease;
        if (release == null) return;
        shortcutRelease.removeCallbacks(release);
        release.run();
    }

    // Lets go of a shortcut's keys in reverse, each with the modifiers still held then: the chord's own pressed before
    // it, and the physical ones.
    private void releaseShortcut(int[] keys) {
        for (int i = keys.length - 1; i >= 0; i--) {
            byte modifiers = getModifierState();
            for (int j = 0; j < i; j++) {
                int mask = getModifierMask(keys[j]);
                modifiers |= (byte) (mask | (mask >> 8));
            }
            sendKeyboardInput(game.keyboardTranslator.translate(keys[i], -1), KeyboardPacket.KEY_UP, modifiers, (byte) 0);
        }
    }

    // A stable sort, as nightjar-policy's shortcuts::normalize_keys: modifiers keep their order, and so do the other keys.
    static int[] modifiersFirst(int[] keys) {
        int[] ordered = new int[keys.length];
        int at = 0;
        for (int key : keys) if (getModifierMask(key) != 0) ordered[at++] = key;
        for (int key : keys) if (getModifierMask(key) == 0) ordered[at++] = key;
        return ordered;
    }

    void keyboardEvent(boolean buttonDown, short keyCode) {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            game.runOnUiThread(() -> game.keyboardEvent(buttonDown, keyCode));
            return;
        }
        short keyMap = game.keyboardTranslator.translate(keyCode, -1);
        if (keyMap != 0) {
            // handleSpecialKeys() takes the Android keycode
            if (handleSpecialKeys(keyCode, buttonDown)) {
                return;
            }
            if (!game.grabbedInput) {
                return;
            }

            if (buttonDown) {
                sendKeyboardInput(keyMap, KeyboardPacket.KEY_DOWN, getModifierState(keyCode), (byte)0);
            }
            else {
                sendKeyboardInput(keyMap, KeyboardPacket.KEY_UP, getModifierState(keyCode), (byte)0);
            }
        }
    }

    private void sendKeyboardInput(short key, byte direction, byte modifiers, byte flags) {
        synchronized (pressedKeys) {
            if (direction == KeyboardPacket.KEY_DOWN) {
                pressedKeys.put(key, flags);
            } else {
                pressedKeys.remove(key);
            }
            if (!customPressedKeys.containsKey(key) && !stickPressedKeys.contains(key)) {
                game.conn.sendKeyboardInput(key, direction, (byte)(modifiers | customModifierState()), flags);
            }
        }
    }

    private byte customModifierState() {
        int modifiers = 0;
        for (int key : customPressedKeys.values()) {
            int mask = getModifierMask(key);
            modifiers |= mask | (mask >> 8);
        }
        return (byte)modifiers;
    }

    void sendCustomKey(int keyCode, boolean down) {
        short key = game.keyboardTranslator.translate(keyCode, -1);
        synchronized (pressedKeys) {
            if (down) customPressedKeys.put(key, keyCode); else customPressedKeys.remove(key);
            if (!pressedKeys.containsKey(key) && !stickPressedKeys.contains(key)) {
                game.conn.sendKeyboardInput(key, down ? KeyboardPacket.KEY_DOWN : KeyboardPacket.KEY_UP,
                        (byte)(getModifierState() | customModifierState()), (byte)0);
            }
        }
    }

    void releaseKeyboardInput() {
        if (game.gameTouchControls != null) game.gameTouchControls.releaseInput();
        pcKeyboard.releaseAll();
        // A shortcut still held goes up below with the other keys, once.
        if (pendingShortcutRelease != null) {
            shortcutRelease.removeCallbacks(pendingShortcutRelease);
            pendingShortcutRelease = null;
        }
        synchronized (pressedKeys) {
            // The stick's keys first: one a keyboard also holds goes up once, with the keyboard's below.
            for (short key : stickPressedKeys) {
                if (!pressedKeys.containsKey(key) && !customPressedKeys.containsKey(key)) {
                    game.conn.sendKeyboardInput(key, KeyboardPacket.KEY_UP, (byte) 0, (byte) 0);
                }
            }
            stickPressedKeys.clear();
            for (Map.Entry<Short, Byte> key : pressedKeys.entrySet()) {
                game.conn.sendKeyboardInput(key.getKey(), KeyboardPacket.KEY_UP, (byte) 0, key.getValue());
            }
            pressedKeys.clear();
        }
        game.touchInput.releaseDesktopInput();
        game.modifierFlags = 0;
        specialKeyCode = KeyEvent.KEYCODE_UNKNOWN;
        waitingForAllModifiersUp = false;
        game.getWindow().getDecorView().removeCallbacks(game.toggleGrab);
    }
}
