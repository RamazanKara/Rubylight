package com.limelight;

import android.content.ComponentName;
import android.os.Build;
import android.os.SystemClock;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowManager;
import android.widget.FrameLayout;

import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;

import com.limelight.binding.input.ControllerButtonMap;
import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.keyboard.PcKeyboardController;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.ui.DesktopControlsPanel;
import com.limelight.ui.KeyboardOverlay;
import com.limelight.ui.PcKeyboardOverlay;

import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;

/**
 * On-screen PC keyboard and desktop-controls keyboard overlays, gamepad/remote navigation while the
 * PC keyboard is shown, and platform meta-key capture for {@link GameKeyboardInput}.
 */
final class GamePcKeyboardInput {
    private final Game game;
    private final GameKeyboardInput owner;

    GamePcKeyboardInput(Game game, GameKeyboardInput owner) {
        this.game = game;
        this.owner = owner;
    }

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

    /** {@code systemKeyboardBeforeMenu}: as for {@link GameKeyboardInput#toggleKeyboard(boolean)}, the system keyboard may not be back yet. */
    void togglePcKeyboard(boolean systemKeyboardBeforeMenu) {
        if (!game.foreground || !game.connected || game.isFinishing() || game.streamMenu != null || !game.conn.canSendInput()) {
            return;
        }
        if (isPcKeyboardVisible()) {
            hidePcKeyboard();
            return;
        }
        // One keyboard at a time: the extra-keys panel and the system keyboard make room.
        hideKeyboardOverlay();
        if (systemKeyboardBeforeMenu) owner.closeSystemKeyboard();
        else owner.hideSystemKeyboard();
        if (game.pcKeyboard == null) {
            game.pcKeyboard = new PcKeyboardOverlay(game, game.prefConfig.pcKeyboardLayout, game.prefConfig.pcKeyboardSize,
                    game.prefConfig.pcKeyboardOpacity, owner::sendPcKeyboardKey, this::hidePcKeyboard);
            game.pcKeyboard.setHaptics(game.prefConfig.vibrateOsc);
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
        owner.pcKeyboardAxisDevice = -1;
        game.pcKeyboard.setVisibility(View.GONE);
        if (game.controllerHandler != null) game.controllerHandler.setPadCapture(null);
        if (game.grabbedInput && !game.cursorVisible && !isKeyboardOverlayVisible() && game.inputCaptureProvider != null) {
            game.inputCaptureProvider.hideCursor();
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

    private static PcKeyboardController.Pad padForKey(int keyCode, boolean flip) {
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
    boolean handlePcKeyboardKey(KeyEvent event) {
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
                if (event.getRepeatCount() > 0 && owner.pcKeyboardKeys.get(keyCode)) return true;
                owner.pcKeyboardKeys.delete(keyCode);
                return false;
            }
            boolean taken = owner.pcKeyboardKeys.get(keyCode);
            owner.pcKeyboardKeys.delete(keyCode);
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
            owner.pcKeyboardKeys.put(keyCode, true);
            // The keyboard repeats keys itself.
            if (event.getRepeatCount() > 0) return true;
        } else {
            owner.pcKeyboardKeys.delete(keyCode);
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
        owner.pcKeyboardKeys.clear();
        owner.pcKeyboardAxisDevice = -1;
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
            if (device == owner.pcKeyboardAxisDevice) game.pcKeyboard.controller().stick(0, 0, event.getEventTime());
            owner.pcKeyboardAxisDevice = -1;
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
        if (owner.pcKeyboardAxisDevice != -1 && owner.pcKeyboardAxisDevice != source
                && PcKeyboardController.stickDirection(x, y, null) == null) {
            return;
        }
        keyboard.stick(x, y, time);
        owner.pcKeyboardAxisDevice = keyboard.stickActive() ? source : -1;
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
                    if (game.touchInput.canSendDesktopInput()) owner.sendKeyboardShortcut(keys);
                }
                @Override public void move(float dx, float dy) { game.touchInput.sendDesktopMotion(dx, dy); }
                @Override public void scroll(float vertical, float horizontal) { game.touchInput.sendDesktopScroll(vertical, horizontal); }
                @Override public void mouseButton(int button, boolean down) { game.touchInput.sendDesktopButton(button, down); }
            }, () -> {
                hideKeyboardOverlay();
                game.toggleKeyboard();
            }, this::hideKeyboardOverlay);
            game.keyboardOverlay.setHaptics(game.prefConfig.vibrateOsc);
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

    void releaseAll() {
        // Sticky modifiers and held keys of the PC keyboard go up with everything else.
        if (game.pcKeyboard != null) game.pcKeyboard.releaseInputs();
        owner.pcKeyboardKeys.clear();
        owner.pcKeyboardAxisDevice = -1;
    }
}
