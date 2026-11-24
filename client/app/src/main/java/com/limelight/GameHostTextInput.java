package com.limelight;

import android.content.Context;
import android.content.res.Configuration;
import android.os.Build;
import android.os.SystemClock;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowInsets;
import android.view.inputmethod.InputMethodManager;
import android.view.inputmethod.InputMethodSubtype;

import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;

import com.limelight.binding.input.KeyboardLayoutTag;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.ui.HostKeyboard;
import com.limelight.ui.HostTextField;

/**
 * Host text-field notices (0x553B), keyboard layout reporting (0x553E), and the soft keyboard's
 * visibility, insets and stream pan for {@link GameKeyboardInput}.
 */
final class GameHostTextInput {
    private final Game game;
    private final HostKeyboard hostKeyboard;
    private final GameKeyboardInput owner;
    // How much of the window's bottom the soft keyboard covers, from the last inset dispatch.
    private int keyboardInsetBottom;
    // The layout last reported to the host this connection, or null.
    private String reportedKeyboardLayout;

    GameHostTextInput(Game game, HostKeyboard hostKeyboard, GameKeyboardInput owner) {
        this.game = game;
        this.hostKeyboard = hostKeyboard;
        this.owner = owner;
    }

    // Asks a host that announces text field notices (0x553B) to say when a text field gains or loses
    // focus, so the soft keyboard follows it (setting "Keyboard follows text fields"), and reports
    // the keyboard layout to a host that takes it (0x553E).
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
        String language = subtype.getLanguageTag();
        return KeyboardLayoutTag.pick(hint, language, subtype.getLocale());
    }

    // An attached hardware keyboard types itself; the soft keyboard must not take its place.
    private boolean hasHardwareKeyboard() {
        Configuration config = game.getResources().getConfiguration();
        return config.keyboard == Configuration.KEYBOARD_QWERTY &&
                config.hardKeyboardHidden == Configuration.HARDKEYBOARDHIDDEN_NO;
    }

    boolean isSystemKeyboardVisible() {
        if (game.streamView == null) return false;
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
                game.conn != null && game.conn.canSendInput(), owner.isPcKeyboardVisible(), hasHardwareKeyboard())) {
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
        owner.hideKeyboardOverlay();
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

    /**
     * Opens or closes the system keyboard. {@code openBeforeMenu}: it was open when the stream menu running this
     * opened. The menu took the window's focus and the keyboard with it, and Android brings the keyboard back only
     * once the menu is gone, so asking now would open it again instead of closing it.
     */
    void toggleKeyboard(boolean openBeforeMenu) {
        if (!game.foreground || !game.connected || game.isFinishing() || game.streamMenu != null || !game.conn.canSendInput()) {
            return;
        }
        LimeLog.info("Toggling keyboard overlay");
        // Below Android 11 the window's own insets never show the keyboard (adjustNothing); the legacy probe does.
        boolean visible = openBeforeMenu || isSystemKeyboardVisible();
        if (!visible) {
            owner.hideKeyboardOverlay();
            owner.hidePcKeyboard();
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

    /**
     * Back closes the system keyboard. The stream view takes every key before the keyboard can, so a remote's Back
     * never reaches it: it opened the menu instead, and the keyboard came back each time the menu closed.
     */
    void closeSystemKeyboard() {
        LimeLog.info("Closing the soft keyboard");
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            game.getWindow().getInsetsController().hide(WindowInsets.Type.ime());
        } else {
            InputMethodManager inputManager = (InputMethodManager) game.getSystemService(Context.INPUT_METHOD_SERVICE);
            inputManager.hideSoftInputFromWindow(game.streamView.getWindowToken(), 0);
        }
    }

    void hideSystemKeyboard() {
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
}
