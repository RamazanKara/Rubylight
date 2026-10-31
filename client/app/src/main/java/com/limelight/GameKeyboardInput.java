package com.limelight;

import com.limelight.binding.input.ControllerButtonMap;
import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.KeyboardLayoutTag;
import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.input.KeyboardPacket;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.ui.HostKeyboard;
import com.limelight.ui.HostTextField;
import com.limelight.ui.KeyboardOverlay;
import com.limelight.ui.PcKeyboardOverlay;
import com.limelight.binding.input.keyboard.PcKeyboardController;
import com.limelight.binding.input.keyboard.PcKeyboardEngine;
import com.limelight.ui.DesktopControlsPanel;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;
import android.content.ComponentName;
import android.content.Context;
import android.content.res.Configuration;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.InputDevice;
import android.view.KeyCharacterMap;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.widget.FrameLayout;
import android.view.inputmethod.InputMethodManager;
import android.view.inputmethod.InputMethodSubtype;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.util.HashMap;
import java.util.Map;

/** The stream's keyboard input: hardware and soft keyboards, modifier state, special keys, the PC keyboard and keyboard overlays and the host's text fields. */
final class GameKeyboardInput {
    private final Game game;

    GameKeyboardInput(Game game) {
        this.game = game;
    }

    // Controller keys the on-screen PC keyboard took on their way down, so their release goes to it too.
    private final android.util.SparseBooleanArray pcKeyboardKeys = new android.util.SparseBooleanArray();
    // The pad whose stick or hat is steering the on-screen keyboard, or -1.
    private int pcKeyboardAxisDevice = -1;

    // Asks a host that announces text field notices (0x553B) to say when a text field gains or loses
    // focus, so the soft keyboard follows it (setting "Keyboard follows text fields"), and reports
    // the keyboard layout to a host that takes it (0x553E). The C engine supports neither.
    void requestTextInput() {
        reportedKeyboardLayout = null;
        hostKeyboard.reset();
        updateKeyboardPan();
        game.streamView.setHostTextField(null);
        reportKeyboardLayout();
        if (game.conn == null || !game.conn.canSendInput() || !game.prefConfig.keyboardFollowsTextFields ||
                !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_TEXT_INPUT)) {
            return;
        }
        if (!MoonBridge.sendTextInputRequest(true)) {
            LimeLog.warning("Could not ask the host for text field notices");
        }
    }

    void reportKeyboardLayout() {
        if (!game.connected || game.conn == null ||
                !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_KEYBOARD_LAYOUT)) {
            return;
        }
        String tag = currentKeyboardLayout();
        if (tag == null || tag.equals(reportedKeyboardLayout)) return;
        if (MoonBridge.sendKeyboardLayout(tag)) {
            reportedKeyboardLayout = tag;
            LimeLog.info("Reported keyboard layout " + tag);
        } else {
            LimeLog.warning("Could not report keyboard layout " + tag);
        }
    }

    // Apps cannot read a hardware keyboard's layout; the IME's subtype says which layout it uses
    // for physical keyboards (Android 14+), else which language it types.
    private String currentKeyboardLayout() {
        InputMethodManager inputManager = (InputMethodManager) game.getSystemService(Context.INPUT_METHOD_SERVICE);
        InputMethodSubtype subtype = inputManager != null ? inputManager.getCurrentInputMethodSubtype() : null;
        if (subtype == null) return null;
        String hint = null;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            android.icu.util.ULocale locale = subtype.getPhysicalKeyboardHintLanguageTag();
            if (locale != null) hint = locale.toLanguageTag();
        }
        String language = Build.VERSION.SDK_INT >= Build.VERSION_CODES.N ? subtype.getLanguageTag() : null;
        return KeyboardLayoutTag.pick(hint, language, subtype.getLocale());
    }

    // An attached hardware keyboard types itself; the soft keyboard must not take its place.
    private boolean hasHardwareKeyboard() {
        Configuration config = game.getResources().getConfiguration();
        return config.keyboard == Configuration.KEYBOARD_QWERTY &&
                config.hardKeyboardHidden == Configuration.HARDKEYBOARDHIDDEN_NO;
    }

    private boolean isSystemKeyboardVisible() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            WindowInsets insets = game.streamView.getRootWindowInsets();
            return insets != null && insets.isVisible(WindowInsets.Type.ime());
        }
        if (game.legacyKeyboard != null) return game.legacyKeyboard.visible();
        WindowInsetsCompat insets = ViewCompat.getRootWindowInsets(game.streamView);
        return insets != null && insets.isVisible(WindowInsetsCompat.Type.ime());
    }

    // The host's text field notice, on the UI thread: a raise or an update (the focus moved to
    // another field) types the keyboard for the field, and opens it right after the user's own
    // tap, click or key on the stream unless they closed it for this field (the host's element id,
    // 0 from a host that sends none); a lower hides only a keyboard the host opened. The keyboard
    // the user opened stays open.
    void onHostTextField(int kind, int inputType, int flags, int element) {
        if (game.isFinishing() || game.streamView == null) return;
        if (kind == HostTextField.KIND_LOWER) {
            LimeLog.info("Host text field lost focus");
            game.streamView.setHostTextField(null);
            if (hostKeyboard.onLower()) hideSystemKeyboard();
            return;
        }
        if (kind != HostTextField.KIND_RAISE && kind != HostTextField.KIND_UPDATE) return;
        LimeLog.info("Host text field focused: type " + inputType + ", flags " + flags +
                ", element " + Integer.toHexString(element));
        hostKeyboard.onFocus(element);
        reportKeyboardLayout();
        if (!HostTextField.mayShowKeyboard(game.foreground, game.connected, game.streamMenu != null,
                game.conn != null && game.conn.canSendInput(), isPcKeyboardVisible(), hasHardwareKeyboard())) {
            return;
        }
        boolean visible = isSystemKeyboardVisible();
        game.streamView.setHostTextField(new HostTextField(inputType, flags));
        if (visible) return;
        long now = SystemClock.uptimeMillis();
        if (!hostKeyboard.mayOpen(now)) {
            LimeLog.info(hostKeyboard.dismissedHere() ? "Keyboard stays closed: closed for this field" :
                    "Keyboard stays closed: no tap, click or key on the stream in the last " +
                            HostKeyboard.INPUT_WINDOW_MS + " ms");
            return;
        }
        hideKeyboardOverlay();
        game.touchInput.setInputGrabState(true);
        game.streamView.requestFocus();
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            game.getWindow().getInsetsController().show(WindowInsets.Type.ime());
        } else {
            InputMethodManager inputManager = (InputMethodManager) game.getSystemService(Context.INPUT_METHOD_SERVICE);
            inputManager.showSoftInput(game.streamView, 0);
        }
        hostKeyboard.onOpened();
    }

    // The user's own tap or click on the stream (key presses note themselves in handleKeyDown): a
    // host text field raised right after it may open the keyboard, and the point it pressed stays
    // above the keyboard. A trackpad finger or a captured mouse moves the PC's pointer instead.
    void noteStreamPress(View view, MotionEvent event) {
        game.streamView.onStreamPress(event);
        int action = event.getActionMasked();
        if (action != MotionEvent.ACTION_DOWN && action != MotionEvent.ACTION_POINTER_DOWN &&
                action != MotionEvent.ACTION_BUTTON_PRESS) {
            return;
        }
        boolean finger = event.isFromSource(InputDevice.SOURCE_TOUCHSCREEN) ||
                event.isFromSource(InputDevice.SOURCE_STYLUS);
        boolean points = finger ? !TouchMode.isRelative(game.touchInput.touchMode()) :
                event.isFromSource(InputDevice.SOURCE_MOUSE) && !event.isFromSource(InputDevice.SOURCE_MOUSE_RELATIVE);
        float y = Float.NaN;
        if (points && game.streamView.getHeight() > 0) {
            y = (event.getY(event.getActionIndex()) + game.touchInput.getStreamViewOffsetY(view)) / game.streamView.getHeight();
        }
        hostKeyboard.onPress(y, SystemClock.uptimeMillis());
    }

    // Every inset dispatch on the stream window. The window takes no room from the keyboard
    // (adjustNothing): the video surface keeps its size, and the stream moves up instead. Android
    // reports the keyboard's insets to such a window from 11 on; before, legacyKeyboard measures it.
    void onWindowInsets(WindowInsetsCompat insets) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return;
        onKeyboardInsets(insets.isVisible(WindowInsetsCompat.Type.ime()),
                insets.getInsets(WindowInsetsCompat.Type.ime()).bottom);
    }

    // The keyboard showed, hid or changed height, covering imeBottom px of the content view's bottom.
    void onKeyboardInsets(boolean visible, int imeBottom) {
        if (game.streamView == null) return;
        keyboardInsetBottom = imeBottom;
        if (hostKeyboard.onVisibility(visible, SystemClock.uptimeMillis())) {
            LimeLog.info(hostKeyboard.visible() ? "Soft keyboard shown" : "Soft keyboard hidden");
        }
        // Insets come before layout: measure the stream once it is laid out, once for a burst.
        game.keyboardPanHandler.removeCallbacks(game.keyboardPan);
        game.keyboardPanHandler.post(game.keyboardPan);
    }

    // The insets are the content view's, and the content view is the stream's parent.
    void updateKeyboardPan() {
        if (game.streamView == null) return;
        float pan = 0;
        View parent = (View) game.streamView.getParent();
        if (parent != null) {
            pan = hostKeyboard.pan(parent.getHeight(), keyboardInsetBottom, game.streamView.getTop(), game.streamView.getHeight(),
                    HostKeyboard.MARGIN_DP * game.getResources().getDisplayMetrics().density);
        }
        if (pan == game.streamView.getTranslationY()) return;
        LimeLog.info(pan == 0 ? "Stream moves back down" : "Stream moves " + Math.round(-pan) + " px up for the soft keyboard");
        game.streamView.setTranslationY(pan);
    }

    private final Map<Short, Byte> pressedKeys = new HashMap<>();

    private boolean waitingForAllModifiersUp = false;
    private int specialKeyCode = KeyEvent.KEYCODE_UNKNOWN;

    private final Map<Short, Integer> customPressedKeys = new HashMap<>();

    // Whether a host text field may open the soft keyboard, whose it is, and how far the stream
    // moves up for it.
    private final HostKeyboard hostKeyboard = new HostKeyboard();
    // How much of the window's bottom the soft keyboard covers, from the last inset dispatch.
    private int keyboardInsetBottom;

    // The layout last reported to the host this connection, or null.
    private String reportedKeyboardLayout;

    void setMetaKeyCaptureState(boolean enabled) {
        // Android has native keyboard capture support starting in API 36.1
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA && Build.VERSION.SDK_INT_FULL >= Build.VERSION_CODES_FULL.BAKLAVA_1) {
            WindowManager.LayoutParams windowLayoutParams = game.getWindow().getAttributes();
            windowLayoutParams.setKeyboardCaptureEnabled(enabled);
            game.getWindow().setAttributes(windowLayoutParams);
        }
        else {
            // This uses custom APIs present on some Samsung devices to allow capture of
            // meta key events while streaming.
            try {
                Class<?> semWindowManager = Class.forName("com.samsung.android.view.SemWindowManager");
                Method getInstanceMethod = semWindowManager.getMethod("getInstance");
                Object manager = getInstanceMethod.invoke(null);

                if (manager != null) {
                    Class<?>[] parameterTypes = new Class<?>[2];
                    parameterTypes[0] = ComponentName.class;
                    parameterTypes[1] = boolean.class;
                    Method requestMetaKeyEventMethod = semWindowManager.getDeclaredMethod("requestMetaKeyEvent", parameterTypes);
                    requestMetaKeyEventMethod.invoke(manager, game.getComponentName(), enabled);
                }
                else {
                    LimeLog.warning("SemWindowManager.getInstance() returned null");
                }
            } catch (ClassNotFoundException ignored) {
            } catch (NoSuchMethodException e) {
                e.printStackTrace();
            } catch (InvocationTargetException e) {
                e.printStackTrace();
            } catch (IllegalAccessException e) {
                e.printStackTrace();
            }
        }
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
                switch (specialKeyCode) {
                    // Toggle input grab
                    case KeyEvent.KEYCODE_Z:
                        Handler h = game.getWindow().getDecorView().getHandler();
                        if (h != null) {
                            h.postDelayed(game.toggleGrab, 250);
                        }
                        break;

                    // Leave the stream
                    case KeyEvent.KEYCODE_Q:
                        game.menuActions.leaveStream();
                        break;

                    case KeyEvent.KEYCODE_M:
                        game.showStreamMenu();
                        break;

                    case KeyEvent.KEYCODE_K:
                        togglePcKeyboard();
                        break;

                    case KeyEvent.KEYCODE_S:
                        game.overlays.togglePerformanceOverlay();
                        break;

                    // Toggle cursor visibility
                    case KeyEvent.KEYCODE_C:
                        game.touchInput.toggleCursorVisibility();
                        break;

                    default:
                        break;
                }

                // Reset special key state
                specialKeyCode = KeyEvent.KEYCODE_UNKNOWN;
                waitingForAllModifiersUp = false;
            }
        }
        // Check if Ctrl+Alt+Shift is down when a non-modifier key is pressed
        else if ((getModifierState() & (KeyboardPacket.MODIFIER_CTRL | KeyboardPacket.MODIFIER_ALT | KeyboardPacket.MODIFIER_SHIFT)) ==
                (KeyboardPacket.MODIFIER_CTRL | KeyboardPacket.MODIFIER_ALT | KeyboardPacket.MODIFIER_SHIFT) &&
                (down && nonModifierKeyCode != KeyEvent.KEYCODE_UNKNOWN)) {
            switch (androidKeyCode) {
                case KeyEvent.KEYCODE_Z:
                case KeyEvent.KEYCODE_Q:
                case KeyEvent.KEYCODE_M:
                case KeyEvent.KEYCODE_K:
                case KeyEvent.KEYCODE_S:
                case KeyEvent.KEYCODE_C:
                    // Remember that a special key combo was activated, so we can consume all key
                    // events until the modifiers come up
                    specialKeyCode = androidKeyCode;
                    waitingForAllModifiersUp = true;
                    breakLoneModifierTap();
                    return true;

                default:
                    // This isn't a special combo that we consume on the client side
                    return false;
            }
        }

        // Not a special combo
        return false;
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
        boolean backAsEscape = game.prefConfig.captureSystemKeys && !game.prefConfig.backOpensMenu;
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
            if (key == KeyEvent.KEYCODE_BACK) game.onBackPressed();
            else game.showStreamMenu();
        }
        return true;
    }

    private boolean handlePhysicalBackMenuKey(KeyEvent event) {
        if (!game.connected || !game.prefConfig.backOpensMenu || event.getKeyCode() != KeyEvent.KEYCODE_BACK) return false;
        // A handheld's dedicated Back can share its gamepad device. Reserve that physical key
        // before controller remapping, while keeping Select/Start and keyboard Windows-key mode.
        if (physicalKeyCode(event) != KeyEvent.KEYCODE_BACK) return false;
        if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) game.showStreamMenu();
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
        if (!game.foreground || !game.connected || game.isFinishing() || game.streamMenu != null || !game.conn.canSendInput()) {
            return;
        }
        LimeLog.info("Toggling keyboard overlay");
        boolean visible;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            WindowInsets insets = game.streamView.getRootWindowInsets();
            visible = insets != null && insets.isVisible(WindowInsets.Type.ime());
        } else {
            WindowInsetsCompat insets = ViewCompat.getRootWindowInsets(game.streamView);
            visible = insets != null && insets.isVisible(WindowInsetsCompat.Type.ime());
        }
        if (!visible) {
            hideKeyboardOverlay();
            hidePcKeyboard();
            game.touchInput.setInputGrabState(true);
        }
        game.streamView.requestFocus();
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            if (visible) {
                game.getWindow().getInsetsController().hide(WindowInsets.Type.ime());
            } else {
                game.getWindow().getInsetsController().show(WindowInsets.Type.ime());
            }
        } else {
            InputMethodManager inputManager = (InputMethodManager) game.getSystemService(Context.INPUT_METHOD_SERVICE);
            if (visible) {
                inputManager.hideSoftInputFromWindow(game.streamView.getWindowToken(), 0);
            } else {
                inputManager.showSoftInput(game.streamView, 0);
            }
        }
    }

    void togglePcKeyboard() {
        if (!game.foreground || !game.connected || game.isFinishing() || game.streamMenu != null || !game.conn.canSendInput()) {
            return;
        }
        if (isPcKeyboardVisible()) {
            hidePcKeyboard();
            return;
        }
        // One keyboard at a time: the extra-keys panel and the system keyboard make room.
        hideKeyboardOverlay();
        hideSystemKeyboard();
        if (game.pcKeyboard == null) {
            game.pcKeyboard = new PcKeyboardOverlay(game, game.prefConfig.pcKeyboardLayout, game.prefConfig.pcKeyboardSize,
                    game.prefConfig.pcKeyboardOpacity, this::sendPcKeyboardKey, this::hidePcKeyboard);
            ViewCompat.setOnApplyWindowInsetsListener(game.pcKeyboard, (view, windowInsets) -> {
                androidx.core.graphics.Insets safe = windowInsets.getInsets(
                        WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout());
                view.setPadding(safe.left, 0, safe.right, safe.bottom);
                return windowInsets;
            });
            ((FrameLayout) game.streamView.getParent()).addView(game.pcKeyboard,
                    new FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT,
                            FrameLayout.LayoutParams.WRAP_CONTENT, android.view.Gravity.BOTTOM));
            ViewCompat.requestApplyInsets(game.pcKeyboard);
        } else {
            game.pcKeyboard.setVisibility(View.VISIBLE);
            game.pcKeyboard.bringToFront();
        }
        // The pads steer the keyboard until it closes. The capture is set before input is enabled, so no
        // report can reach the game in between; setPadCapture lets go of whatever the game held, and each
        // USB pad does the same from its own thread on its first report.
        game.controllerHandler.setPadCapture(new ControllerHandler.PadCapture() {
            @Override public void button(int flag, boolean down) {
                PcKeyboardController.Pad pad = padForFlag(flag);
                if (pad != null) game.runOnUiThread(() -> padToPcKeyboard(pad, down));
            }
            @Override public void stick(int controllerId, float x, float y) {
                // USB-driver pads have no Android device id; a negative one cannot clash with a real device.
                game.runOnUiThread(() -> {
                    if (isPcKeyboardVisible()) steerPcKeyboard(-2 - controllerId, x, y, SystemClock.uptimeMillis());
                });
            }
        });
        // A release that leaves the keyboard open (a layout switch) must not leave a held USB stick stale.
        game.pcKeyboard.controller().setOnReleased(game.controllerHandler::resyncPadCapture);
        game.touchInput.setInputGrabState(true);
        game.streamView.requestFocus();
    }

    boolean isPcKeyboardVisible() {
        return game.pcKeyboard != null && game.pcKeyboard.getVisibility() == View.VISIBLE;
    }

    /** Closing lets go of every key and modifier it pressed, and gives the pads back to the game. */
    void hidePcKeyboard() {
        if (game.pcKeyboard == null || game.pcKeyboard.getVisibility() != View.VISIBLE) return;
        game.pcKeyboard.releaseInputs();
        pcKeyboardAxisDevice = -1;
        game.pcKeyboard.setVisibility(View.GONE);
        if (game.controllerHandler != null) game.controllerHandler.setPadCapture(null);
        if (game.grabbedInput && !game.cursorVisible && !isKeyboardOverlayVisible() && game.inputCaptureProvider != null) {
            game.inputCaptureProvider.hideCursor();
        }
    }

    private void sendPcKeyboardKey(int vk, boolean down, byte modifiers) {
        if (!game.connected || game.conn == null || !game.conn.canSendInput()) return;
        sendKeyboardInput((short) (0x8000 | vk), down ? KeyboardPacket.KEY_DOWN : KeyboardPacket.KEY_UP, modifiers, (byte) 0);
    }

    private void hideSystemKeyboard() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            WindowInsets insets = game.streamView.getRootWindowInsets();
            if (insets != null && insets.isVisible(WindowInsets.Type.ime())) {
                game.getWindow().getInsetsController().hide(WindowInsets.Type.ime());
            }
        } else {
            InputMethodManager inputManager = (InputMethodManager) game.getSystemService(Context.INPUT_METHOD_SERVICE);
            inputManager.hideSoftInputFromWindow(game.streamView.getWindowToken(), 0);
        }
    }

    private void padToPcKeyboard(PcKeyboardController.Pad pad, boolean down) {
        if (isPcKeyboardVisible()) game.pcKeyboard.controller().pad(pad, down, SystemClock.uptimeMillis());
    }

    private static PcKeyboardController.Pad padForFlag(int flag) {
        switch (flag) {
            case ControllerPacket.UP_FLAG: return PcKeyboardController.Pad.UP;
            case ControllerPacket.DOWN_FLAG: return PcKeyboardController.Pad.DOWN;
            case ControllerPacket.LEFT_FLAG: return PcKeyboardController.Pad.LEFT;
            case ControllerPacket.RIGHT_FLAG: return PcKeyboardController.Pad.RIGHT;
            case ControllerPacket.A_FLAG: return PcKeyboardController.Pad.A;
            case ControllerPacket.B_FLAG: return PcKeyboardController.Pad.B;
            case ControllerPacket.X_FLAG: return PcKeyboardController.Pad.X;
            case ControllerPacket.Y_FLAG: return PcKeyboardController.Pad.Y;
            case ControllerPacket.LB_FLAG: return PcKeyboardController.Pad.LB;
            case ControllerPacket.RB_FLAG: return PcKeyboardController.Pad.RB;
            case ControllerPacket.PLAY_FLAG: return PcKeyboardController.Pad.START;
            default: return null;
        }
    }

    private PcKeyboardController.Pad padForKey(int keyCode, boolean flip) {
        switch (keyCode) {
            case KeyEvent.KEYCODE_DPAD_UP: return PcKeyboardController.Pad.UP;
            case KeyEvent.KEYCODE_DPAD_DOWN: return PcKeyboardController.Pad.DOWN;
            case KeyEvent.KEYCODE_DPAD_LEFT: return PcKeyboardController.Pad.LEFT;
            case KeyEvent.KEYCODE_DPAD_RIGHT: return PcKeyboardController.Pad.RIGHT;
            // OK on a TV remote.
            case KeyEvent.KEYCODE_DPAD_CENTER:
            case KeyEvent.KEYCODE_ENTER:
            case KeyEvent.KEYCODE_NUMPAD_ENTER: return PcKeyboardController.Pad.A;
            case KeyEvent.KEYCODE_BUTTON_A: return flip ? PcKeyboardController.Pad.B : PcKeyboardController.Pad.A;
            case KeyEvent.KEYCODE_BUTTON_B: return flip ? PcKeyboardController.Pad.A : PcKeyboardController.Pad.B;
            case KeyEvent.KEYCODE_BUTTON_X: return flip ? PcKeyboardController.Pad.Y : PcKeyboardController.Pad.X;
            case KeyEvent.KEYCODE_BUTTON_Y: return flip ? PcKeyboardController.Pad.X : PcKeyboardController.Pad.Y;
            case KeyEvent.KEYCODE_BUTTON_L1: return PcKeyboardController.Pad.LB;
            case KeyEvent.KEYCODE_BUTTON_R1: return PcKeyboardController.Pad.RB;
            // Menu is Start, as in the game's own button mapping.
            case KeyEvent.KEYCODE_MENU:
            case KeyEvent.KEYCODE_BUTTON_START: return PcKeyboardController.Pad.START;
            default: return null;
        }
    }

    /**
     * While the PC keyboard is open a controller or TV remote steers it, not the game. Everything on a
     * controller is consumed, so no button reaches the game; a physical keyboard still types to the PC.
     */
    private boolean handlePcKeyboardKey(KeyEvent event) {
        boolean remote = ControllerHandler.isRemoteControlEvent(event);
        if (!remote && !isRealGamepad(event.getDevice())) return false;
        int keyCode = event.getKeyCode();
        boolean down = event.getAction() == KeyEvent.ACTION_DOWN;
        if (!isPcKeyboardVisible()) {
            // A release of a key the keyboard took stays with it, even if it just closed. A new press
            // is the game's, so an old entry for the same key must not swallow its release.
            if (down) {
                // Held past the key repeat after the keyboard closed (B closes it): the host never saw the
                // press, so it must not see the repeats, and the release stays with the keyboard.
                if (event.getRepeatCount() > 0 && pcKeyboardKeys.get(keyCode)) return true;
                pcKeyboardKeys.delete(keyCode);
                return false;
            }
            boolean taken = pcKeyboardKeys.get(keyCode);
            pcKeyboardKeys.delete(keyCode);
            return taken;
        }
        // The user's button map for this pad replaces face-button flipping, as it does for the game.
        int mapped = remote || game.controllerHandler == null ? ControllerButtonMap.UNMAPPED : game.controllerHandler.userMappedKey(event);
        PcKeyboardController.Pad pad = mapped == ControllerButtonMap.UNMAPPED
                ? padForKey(keyCode, game.prefConfig.flipFaceButtons) : padForKey(mapped, false);
        // A pad's own Back (a handheld's Back key, or a pad that sends Back for B) closes the keyboard like B:
        // Android's TV rules name both as Back, and swallowing it would leave the pad with no way out.
        if (pad == null && !remote && (mapped == ControllerButtonMap.UNMAPPED ? keyCode : mapped) == KeyEvent.KEYCODE_BACK) {
            pad = PcKeyboardController.Pad.B;
        }
        if (pad == null) return !remote;
        if (down) {
            pcKeyboardKeys.put(keyCode, true);
            // The keyboard repeats keys itself.
            if (event.getRepeatCount() > 0) return true;
        } else {
            pcKeyboardKeys.delete(keyCode);
        }
        game.pcKeyboard.controller().pad(pad, down, event.getEventTime());
        return true;
    }

    /** A controller that really exists with gamepad or joystick sources; the synthetic or missing device does not count. */
    private static boolean isRealGamepad(InputDevice device) {
        if (device == null || device.getId() == -1) return false;
        int sources = device.getSources();
        return (sources & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD
                || (sources & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK;
    }

    /** A controller went away: its held buttons would never come up, so the keyboard lets go of them. */
    void releasePcKeyboardPads() {
        pcKeyboardKeys.clear();
        pcKeyboardAxisDevice = -1;
        if (game.pcKeyboard != null) game.pcKeyboard.controller().releasePads(SystemClock.uptimeMillis());
    }

    /**
     * The hat and the left stick steer the keyboard like the D-pad keys; both name the same direction,
     * so a pad that reports its D-pad as keys and as a hat moves once. Only one pad steers at a time:
     * a second pad resting at centre must not cancel a direction held on the first.
     */
    void handlePcKeyboardMotion(MotionEvent event) {
        int action = event.getActionMasked();
        int device = event.getDeviceId();
        if (action == MotionEvent.ACTION_CANCEL) {
            if (device == pcKeyboardAxisDevice) game.pcKeyboard.controller().stick(0, 0, event.getEventTime());
            pcKeyboardAxisDevice = -1;
            return;
        }
        if (action != MotionEvent.ACTION_MOVE) return;
        float hatX = event.getAxisValue(MotionEvent.AXIS_HAT_X), hatY = event.getAxisValue(MotionEvent.AXIS_HAT_Y);
        float x = hatX != 0 ? hatX : event.getAxisValue(MotionEvent.AXIS_X);
        float y = hatY != 0 ? hatY : event.getAxisValue(MotionEvent.AXIS_Y);
        steerPcKeyboard(device, x, y, event.getEventTime());
    }

    /** The one place a stick or hat reaches the keyboard, for Android pads and USB-driver pads alike. */
    private void steerPcKeyboard(int source, float x, float y, long time) {
        PcKeyboardController keyboard = game.pcKeyboard.controller();
        if (pcKeyboardAxisDevice != -1 && pcKeyboardAxisDevice != source
                && PcKeyboardController.stickDirection(x, y, null) == null) {
            return;
        }
        keyboard.stick(x, y, time);
        pcKeyboardAxisDevice = keyboard.stickActive() ? source : -1;
    }

    void toggleKeyboardOverlay() {
        if (!game.foreground || !game.connected || game.isFinishing() || game.streamMenu != null || !game.conn.canSendInput()) {
            return;
        }
        if (isKeyboardOverlayVisible()) {
            hideKeyboardOverlay();
            return;
        }
        hidePcKeyboard();
        if (game.keyboardOverlay == null) {
            game.keyboardOverlay = new KeyboardOverlay(game, new DesktopControlsPanel.Sink() {
                @Override public void sendKeys(int[] keys) {
                    if (game.touchInput.canSendDesktopInput()) sendKeyboardShortcut(keys);
                }
                @Override public void move(float dx, float dy) { game.touchInput.sendDesktopMotion(dx, dy); }
                @Override public void scroll(float vertical, float horizontal) { game.touchInput.sendDesktopScroll(vertical, horizontal); }
                @Override public void mouseButton(int button, boolean down) { game.touchInput.sendDesktopButton(button, down); }
            }, () -> {
                hideKeyboardOverlay();
                game.toggleKeyboard();
            }, this::hideKeyboardOverlay);
            ViewCompat.setOnApplyWindowInsetsListener(game.keyboardOverlay, (view, windowInsets) -> {
                androidx.core.graphics.Insets safe = windowInsets.getInsets(
                        WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout());
                float density = game.getResources().getDisplayMetrics().density;
                int horizontal = Math.round(4 * density), vertical = Math.round(3 * density);
                view.setPadding(safe.left + horizontal, vertical, safe.right + horizontal, safe.bottom + vertical);
                return windowInsets;
            });
            ((FrameLayout) game.streamView.getParent()).addView(game.keyboardOverlay,
                    new FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT,
                            FrameLayout.LayoutParams.WRAP_CONTENT, android.view.Gravity.BOTTOM));
            ViewCompat.requestApplyInsets(game.keyboardOverlay);
        } else {
            game.keyboardOverlay.reloadConfiguration();
            game.keyboardOverlay.setVisibility(View.VISIBLE);
            game.keyboardOverlay.bringToFront();
        }
        game.touchInput.setInputGrabState(true);
        game.streamView.requestFocus();
    }

    boolean isKeyboardOverlayVisible() {
        return game.keyboardOverlay != null && game.keyboardOverlay.getVisibility() == View.VISIBLE;
    }

    void hideKeyboardOverlay() {
        if (game.keyboardOverlay == null) return;
        game.touchInput.releaseDesktopInput();
        game.keyboardOverlay.setVisibility(View.GONE);
        if (game.grabbedInput && !game.cursorVisible && game.inputCaptureProvider != null) {
            game.inputCaptureProvider.hideCursor();
        }
    }

    private void sendKeyboardShortcut(int... keys) {
        if (!game.foreground || !game.connected || !game.grabbedInput || game.streamMenu != null || !game.conn.canSendInput()) {
            return;
        }
        byte modifiers = getModifierState();
        boolean[] sent = new boolean[keys.length];
        for (int i = 0; i < keys.length; i++) {
            short key = game.keyboardTranslator.translate(keys[i], -1);
            int mask = getModifierMask(keys[i]);
            modifiers |= (byte) (mask | (mask >> 8));
            if (!pressedKeys.containsKey(key)) {
                sendKeyboardInput(key, KeyboardPacket.KEY_DOWN, modifiers, (byte) 0);
                sent[i] = true;
            }
        }
        for (int i = keys.length - 1; i >= 0; i--) {
            if (sent[i]) {
                int mask = getModifierMask(keys[i]);
                modifiers &= ~(mask | (mask >> 8));
                modifiers |= getModifierState();
                sendKeyboardInput(game.keyboardTranslator.translate(keys[i], -1), KeyboardPacket.KEY_UP, modifiers, (byte) 0);
            }
        }
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
            if (!customPressedKeys.containsKey(key)) {
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
            if (!pressedKeys.containsKey(key)) {
                game.conn.sendKeyboardInput(key, down ? KeyboardPacket.KEY_DOWN : KeyboardPacket.KEY_UP,
                        (byte)(getModifierState() | customModifierState()), (byte)0);
            }
        }
    }

    void releaseKeyboardInput() {
        if (game.gameTouchControls != null) game.gameTouchControls.releaseInput();
        // Sticky modifiers and held keys of the PC keyboard go up with everything else.
        if (game.pcKeyboard != null) game.pcKeyboard.releaseInputs();
        pcKeyboardKeys.clear();
        pcKeyboardAxisDevice = -1;
        synchronized (pressedKeys) {
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
