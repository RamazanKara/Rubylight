package com.limelight;

import com.limelight.binding.input.MouseDeltaAccumulator;
import com.limelight.binding.input.TouchArrivalWatch;
import com.limelight.binding.input.touch.AbsoluteTouchContext;
import com.limelight.binding.input.touch.PointerAcceleration;
import com.limelight.binding.input.touch.RelativeTouchContext;
import com.limelight.binding.input.touch.FingerTapGesture;
import com.limelight.binding.input.touch.TwoFingerTapGesture;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.binding.input.touch.TouchMomentum;
import com.limelight.binding.input.touch.TouchpadSurface;
import com.limelight.binding.input.touch.TrackpadArea;
import com.limelight.binding.input.touch.TrackpadContext;
import com.limelight.binding.input.touch.TrackpadPointer;
import com.limelight.binding.input.evdev.EvdevListener;
import com.limelight.binding.input.touch.TouchContext;
import com.limelight.binding.input.virtual_controller.VirtualController;
import com.limelight.computers.HostNotices;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.HostCursorShapes;
import com.limelight.ui.StreamView;
import android.annotation.SuppressLint;
import com.limelight.ui.NavigationDialogBuilder;
import android.os.Build;
import android.util.DisplayMetrics;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.widget.Toast;

/** The stream's pointer input: touch modes and contexts, the trackpad, mice and pens, the desktop pointer, on-screen controls and the mic button. */
final class GameTouchInput {
    private final Game game;

    GameTouchInput(Game game) {
        this.game = game;
    }

    private final TouchArrivalWatch touchArrivals = new TouchArrivalWatch();
    private int lastButtonState = 0;
    private final MouseDeltaAccumulator mouseDeltaX = new MouseDeltaAccumulator();
    private final MouseDeltaAccumulator mouseDeltaY = new MouseDeltaAccumulator();

    // Mouse emulation uses two touches; native touch forwards all pointers.
    private final TouchContext[] touchContextMap = new TouchContext[2];
    // Finger touches of a physical touchpad: tap-to-click, scroll and drag, with the Trackpad Sensitivity settings.
    private final TrackpadContext[] touchpadContextMap = new TrackpadContext[2];
    private int touchpadResolutionDevice = -2;

    // Undoes the desktop zoom for an event. Set again for every event while the desktop is zoomed and
    // done with before the event is handled; input arrives on the main thread only, so one serves all.
    private final android.graphics.Matrix zoomInverse = new android.graphics.Matrix();
    private final float[] zoomInverseValues = new float[9];
    private final FingerTapGesture fingerTap = new FingerTapGesture();
    private final TwoFingerTapGesture twoFingerTap = new TwoFingerTapGesture();

    private TrackpadArea trackpadArea;

    // The current finger gesture started outside the trackpad area or while touch is off.
    private boolean touchGestureIgnored;
    private boolean middleButtonUpPending;
    private final Runnable middleButtonUp = () -> {
        middleButtonUpPending = false;
        GameTouchInput.this.game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_MIDDLE);
    };
    private boolean rightButtonUpPending;
    private final Runnable rightButtonUp = () -> {
        rightButtonUpPending = false;
        GameTouchInput.this.game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
    };

    private boolean nativeTouchUnsupported;
    private boolean nativeTouchGestureActive;

    // Touchpad gestures on the PC: 0 not decided yet (the touchpad's first event decides), 1 a
    // physical touchpad's fingers go to the PC, 2 the touchpad stays a mouse.
    private int nativeTouchpadState;
    // The PC holds fingers or the click of the touchpad.
    private boolean nativeTouchpadHeld;
    // Reused by every touchpad frame: a touchpad sends 125-250 of them a second, and the host copies each out
    // before sendTouchpadFrame returns, so one set of buffers serves every event.
    private int[] nativeTouchpadIds = new int[16];
    private float[] nativeTouchpadXs = new float[16];
    private float[] nativeTouchpadYs = new float[16];
    private float[] nativeTouchpadPressures = new float[16];

    private static final int STYLUS_DOWN_DEAD_ZONE_DELAY = 100;
    private static final int STYLUS_DOWN_DEAD_ZONE_RADIUS = 20;

    private static final int STYLUS_UP_DEAD_ZONE_DELAY = 150;
    private static final int STYLUS_UP_DEAD_ZONE_RADIUS = 50;

    // Travel allowed for a three- or four-finger tap, in dp.
    private static final int FINGER_TAP_SLOP_DP = 40;

    private int desktopHeldButtons;
    private MouseDeltaAccumulator desktopDeltaX = new MouseDeltaAccumulator();
    private MouseDeltaAccumulator desktopDeltaY = new MouseDeltaAccumulator();
    private MouseDeltaAccumulator desktopScrollV = new MouseDeltaAccumulator();
    private MouseDeltaAccumulator desktopScrollH = new MouseDeltaAccumulator();

    private long lastAbsTouchUpTime = 0;
    private long lastAbsTouchDownTime = 0;
    private float lastAbsTouchUpX, lastAbsTouchUpY;
    private float lastAbsTouchDownX, lastAbsTouchDownY;

    // A USB or Bluetooth mouse, not a touchscreen or a virtual device.
    boolean hasExternalMouse() {
        for (int id : InputDevice.getDeviceIds()) {
            InputDevice device = InputDevice.getDevice(id);
            if (device == null || device.isVirtual() ||
                    (device.getSources() & InputDevice.SOURCE_MOUSE) != InputDevice.SOURCE_MOUSE) continue;
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q || device.isExternal()) return true;
        }
        return false;
    }

    // The same as Ctrl+Alt+Shift+C: show or hide this device's cursor, capturing input if needed.
    void toggleCursorVisibility() {
        if (game.inputCaptureProvider == null) return;
        if (!game.grabbedInput) {
            setInputGrabState(true);
        }
        game.cursorVisible = !game.cursorVisible;
        if (game.cursorVisible || game.keyboardInput.isKeyboardOverlayVisible() || game.keyboardInput.isPcKeyboardVisible()) {
            game.inputCaptureProvider.showCursor();
        } else {
            game.inputCaptureProvider.hideCursor();
        }
    }

    void initializeTouchContexts() {
        if (game.trackpadPointer == null) {
            game.trackpadPointer = new TrackpadPointer(game.conn, game.streamView, game.findViewById(R.id.localCursor),
                    game.prefConfig.absoluteMouseMode,
                    game.prefConfig.trackpadAcceleration ? PointerAcceleration.forCurve(game.prefConfig.trackpadAccelerationCurve) : null,
                    game.prefConfig.trackpadMomentum ? new TouchMomentum(game.prefConfig.trackpadMomentumGlide) : null);
            trackpadArea = TrackpadArea.forPreset(game.prefConfig.trackpadArea);
            if (game.desktopGestures != null) game.trackpadPointer.setViewport(game.desktopGestures.viewport());
        }
        game.trackpadPointer.stopMomentum();
        game.trackpadPointer.setReferenceSize(game.prefConfig.width, game.prefConfig.height);
        game.trackpadPointer.setLocalCursor(touchMode() == TouchMode.LOCAL_CURSOR);
        updateHostCursorShapes();
        for (int i = 0; i < touchpadContextMap.length; i++) {
            TrackpadContext touchpad = new TrackpadContext(game.conn, i, game.prefConfig.trackpadSwapAxis,
                    game.prefConfig.trackpadSensitivityX, game.prefConfig.trackpadSensitivityY,
                    game.prefConfig.mouseSpeed, game.prefConfig.absoluteMouseMode ? game.streamView : null);
            touchpad.setScrollSpeed(game.prefConfig.trackpadScrollSpeed);
            touchpadContextMap[i] = touchpad;
        }
        touchpadResolutionDevice = -2;
        for (int i = 0; i < touchContextMap.length; i++) {
            if (usesTrackpadGestures()) {
                // Same gestures as a physical touchpad: tap to click, two-finger scroll and right-click,
                // tap and touch again to drag, flick momentum.
                TrackpadContext gestures = new TrackpadContext(game.conn, i, game.prefConfig.absoluteMouseMode,
                        game.prefConfig.width, game.prefConfig.height, new TrackpadContext.MoveScale() {
                            @Override public float x() {
                                return game.prefConfig.width / (float) Math.max(1, game.streamView.getWidth()) * game.prefConfig.trackpadSpeed / 100f;
                            }
                            @Override public float y() {
                                return game.prefConfig.height / (float) Math.max(1, game.streamView.getHeight()) * game.prefConfig.trackpadSpeed / 100f;
                            }
                        });
                gestures.setScrollSpeed(game.prefConfig.trackpadScrollSpeed);
                DisplayMetrics metrics = game.streamView.getResources().getDisplayMetrics();
                gestures.setUnitsPerMm((metrics.xdpi + metrics.ydpi) / 2f / 25.4f);
                touchContextMap[i] = gestures;
                continue;
            }
            touchContextMap[i] = game.prefConfig.touchscreenTrackpad ?
                    new RelativeTouchContext(game.conn, i, game.prefConfig.width, game.prefConfig.height, game.streamView, game.prefConfig,
                            game.trackpadPointer) :
                    new AbsoluteTouchContext(game.conn, i, game.streamView,
                            () -> game.desktopGestures != null ? game.desktopGestures.viewport().scale() : 1);
        }
    }

    /**
     * Asks a host that offers cursor shapes for them while the Local cursor is in use, and stops
     * them when it is not. Without host support the Local cursor stays Nightjar's arrow.
     */
    void updateHostCursorShapes() {
        ComputerDetails details = game.conn != null ? game.conn.getHostDetails() : null;
        boolean wanted = game.connected && game.trackpadPointer != null && game.trackpadPointer.isLocalCursor() && details != null &&
                details.caps.has(HostCapabilities.Feature.CURSOR_SHAPE) &&
                MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_CURSOR_SHAPE_REQUEST);
        HostCursorShapes current = game.hostCursorShapes;
        if (wanted == (current != null)) return;
        if (wanted) {
            HostCursorShapes shapes = new HostCursorShapes(game.getResources(), game.findViewById(R.id.localCursor), game.trackpadPointer,
                    () -> game.messages.alert(R.string.stream_cursor_shape_rejected, Toast.LENGTH_LONG));
            game.hostCursorShapes = shapes;
            if (MoonBridge.sendCursorShapeRequest(true)) {
                LimeLog.info("Host cursor shapes requested");
                return;
            }
            LimeLog.warning("Host cursor shape request could not be sent; keeping the local arrow");
            game.hostCursorShapes = null;
            shapes.close();
        } else {
            game.hostCursorShapes = null;
            MoonBridge.sendCursorShapeRequest(false);
            current.close();
        }
    }

    void stopHostCursorShapes() {
        HostCursorShapes shapes = game.hostCursorShapes;
        if (shapes == null) return;
        game.hostCursorShapes = null;
        game.runOnUiThread(shapes::close);
    }

    /** The finger touch mode in effect; external displays and tests change the switches directly. */
    int touchMode() {
        if (!game.prefConfig.touchscreenTrackpad) {
            return game.nativeTouchEnabled ? TouchMode.MULTI_TOUCH : TouchMode.DIRECT;
        }
        return game.prefConfig.touchMode == TouchMode.LOCAL_CURSOR || game.prefConfig.touchMode == TouchMode.OFF ||
                game.prefConfig.touchMode == TouchMode.TOUCHPAD ? game.prefConfig.touchMode : TouchMode.TRACKPAD;
    }

    private boolean isInTrackpadArea(View view, MotionEvent event) {
        if (trackpadArea == null || trackpadArea.isFull()) return true;
        View parent = (View) game.streamView.getParent();
        if (parent == null) return true;
        float x = event.getX(0) + (view != null ? view.getX() : 0);
        float y = event.getY(0) + (view != null ? view.getY() : 0);
        return trackpadArea.contains(x, y, parent.getWidth(), parent.getHeight());
    }

    private void onFingerTap(String action) {
        if (FingerTapGesture.NONE.equals(action)) return;
        if (FingerTapGesture.DESKTOP_CONTROLS.equals(action)) {
            game.keyboardInput.toggleKeyboardOverlay();
        } else if (FingerTapGesture.PC_KEYBOARD.equals(action)) {
            game.keyboardInput.togglePcKeyboard();
        } else if (FingerTapGesture.MIDDLE_CLICK.equals(action)) {
            if (middleButtonUpPending) {
                game.bitrateHandler.removeCallbacks(middleButtonUp);
                middleButtonUp.run();
            }
            game.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_MIDDLE);
            // Held briefly, like a trackpad tap, so games that poll the buttons see it.
            middleButtonUpPending = true;
            game.bitrateHandler.postDelayed(middleButtonUp, 100);
        } else {
            game.toggleKeyboard();
        }
    }

    /** Own the whole multi-finger sequence so lifting in any order cannot leak mouse events. */
    private boolean handleFingerTapGesture(MotionEvent event) {
        int action = event.getActionMasked();
        if (action == MotionEvent.ACTION_DOWN) fingerTap.reset();
        if (!fingerTap.isStarted() && action == MotionEvent.ACTION_POINTER_DOWN &&
                event.getPointerCount() == 3 &&
                // Only fingers landing together tap: a third one beside two held thumbs is play.
                event.getEventTime() - event.getDownTime() < FingerTapGesture.TIME_LIMIT_MS &&
                (!FingerTapGesture.NONE.equals(game.prefConfig.threeFingerTap) ||
                        !FingerTapGesture.NONE.equals(game.prefConfig.fourFingerTap))) {
            fingerTap.start(event.getEventTime(), event.getPointerId(0), event.getX(0), event.getY(0));
            for (TouchContext context : touchContextMap) {
                if (context != null) {
                    context.cancelTouch();
                    context.setPointerCount(0);
                }
            }
            if (game.trackpadPointer != null) game.trackpadPointer.stopMomentum();
        }
        if (!fingerTap.isStarted()) return false;

        fingerTap.pointers(event.getPointerCount());
        int tracked = event.findPointerIndex(fingerTap.pointerId());
        if (tracked >= 0) {
            for (int i = 0; i < event.getHistorySize(); i++) {
                fingerTap.move(event.getHistoricalX(tracked, i), event.getHistoricalY(tracked, i));
            }
            fingerTap.move(event.getX(tracked), event.getY(tracked));
        }
        if (action == MotionEvent.ACTION_CANCEL) {
            fingerTap.reset();
        } else {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
                    (event.getFlags() & MotionEvent.FLAG_CANCELED) != 0) {
                fingerTap.invalidate();
            }
            if (action == MotionEvent.ACTION_UP) {
                int fingers = fingerTap.finish(event.getEventTime(),
                        FINGER_TAP_SLOP_DP * game.getResources().getDisplayMetrics().density);
                if (fingers == 3) onFingerTap(game.prefConfig.threeFingerTap);
                else if (fingers == 4) onFingerTap(game.prefConfig.fourFingerTap);
            }
        }
        return true;
    }

    void clickRightMouseButton() {
        if (rightButtonUpPending) {
            game.bitrateHandler.removeCallbacks(rightButtonUp);
            rightButtonUp.run();
        }
        game.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_RIGHT);
        rightButtonUpPending = true;
        game.bitrateHandler.postDelayed(rightButtonUp, 100);
    }

    void cancelTouchInput() {
        if (game.desktopGestures != null) game.desktopGestures.cancelTouch();
        cancelHostTouchInput();
        cancelNativeTouchpad();
    }

    // Desktop pan/zoom takes over the host gesture while retaining its own tap candidate.
    void cancelHostTouchInput() {
        for (TouchContext context : touchContextMap) {
            if (context != null) {
                context.cancelTouch();
                context.setPointerCount(0);
            }
        }
        if (nativeTouchGestureActive) {
            game.conn.sendTouchEvent(MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL, 0, 0, 0, 0, 0, 0, MoonBridge.LI_ROT_UNKNOWN);
            nativeTouchGestureActive = false;
        }
        fingerTap.reset();
        twoFingerTap.reset();
        touchGestureIgnored = false;
        if (game.trackpadPointer != null) game.trackpadPointer.stopMomentum();
        if (middleButtonUpPending) {
            game.bitrateHandler.removeCallbacks(middleButtonUp);
            middleButtonUp.run();
        }
        if (rightButtonUpPending) {
            game.bitrateHandler.removeCallbacks(rightButtonUp);
            rightButtonUp.run();
        }
    }

    /**
     * Switches the touch mode of this stream. {@code persist} also saves it as the setting; in dual-screen
     * mode only the first stream's process does, because the second's copy of the settings can be older
     * than the first's and would write them back.
     */
    void applyTouchMode(int which, boolean persist) {
        cancelTouchInput();
        game.prefConfig.touchMode = which;
        game.prefConfig.touchscreenTrackpad = TouchMode.legacyTrackpad(which);
        game.nativeTouchEnabled = which == TouchMode.MULTI_TOUCH;
        nativeTouchUnsupported = false;
        initializeTouchContexts();
        if (persist) {
            DualScreen.saveSettings(game, editor -> TouchMode.save(editor, which));
        }
    }

    void showTouchModeDialog() {
        if (game.externalPresentation != null) {
            game.messages.alert(R.string.desktop_touchpad_help, Toast.LENGTH_LONG);
            return;
        }
        String[] modes = game.getResources().getStringArray(R.array.stream_touch_modes);
        game.menuActions.showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.stream_touch_mode)
                .setSingleChoiceItems(modes, touchMode(), (dialog, which) -> {
                    applyTouchMode(which, !DualScreen.isSecond(game.getIntent()));
                    DualScreen.mirrorTouchMode(game, which);
                    dialog.dismiss();
                    if (game.nativeTouchEnabled) {
                        game.messages.alert(R.string.stream_native_touch_help, Toast.LENGTH_LONG);
                    } else if (which == TouchMode.OFF) {
                        game.messages.alert(R.string.stream_touch_off_help, Toast.LENGTH_LONG);
                    }
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    void setOnscreenControlsEnabled(boolean enabled) {
        if (!game.conn.canSendInput()) {
            return;
        }
        game.prefConfig.onscreenController = enabled;
        if (enabled) {
            game.showOnscreenController();
        } else {
            if (game.virtualController != null) {
                game.virtualController.setControllerMode(VirtualController.ControllerMode.Active);
                game.virtualController.hide();
            }
            if (game.modernController != null) {
                game.modernController.finishEditing(false);
                game.modernController.hide();
            }
        }
        // The microphone button's default place follows the modern controller's top row.
        if (game.micButton != null) game.micButton.place();
        if (game.menuButton != null) game.menuButton.place();
        DualScreen.saveSettings(game, editor -> editor.putBoolean("checkbox_show_onscreen_controls", enabled));
        // Hiding controls changes no layout by itself; ask for one so the overlay can move back.
        game.performanceOverlayScroll.requestLayout();
    }

    // Shows the microphone button when its setting is on, the host takes the microphone and input is allowed.
    void updateMicButton() {
        if (game.micButton == null) return;
        game.micButton.setSuppressed(!com.limelight.ui.MicButton.allowed(game.prefConfig.micButton, game.isHidingOverlays,
                game.conn != null && game.conn.canSendInput()));
        game.micButton.place();
    }

    void setMicButtonShown(boolean shown) {
        game.prefConfig.micButton = shown;
        DualScreen.saveSettings(game, editor -> editor.putBoolean(com.limelight.preferences.PreferenceConfiguration.MIC_BUTTON_PREF_STRING, shown));
        updateMicButton();
        game.performanceOverlayScroll.requestLayout();
    }

    // Shows the floating menu button while its setting is on and the overlays are not hidden (picture in picture).
    void updateMenuButton() {
        if (game.menuButton == null) return;
        game.menuButton.setShown(game.prefConfig.floatingMenuButton && !game.isHidingOverlays);
        game.menuButton.place();
    }

    void setMenuButtonShown(boolean shown) {
        game.prefConfig.floatingMenuButton = shown;
        DualScreen.saveSettings(game, editor -> editor.putBoolean(com.limelight.preferences.PreferenceConfiguration.FLOATING_MENU_BUTTON_PREF_STRING, shown));
        updateMenuButton();
        game.performanceOverlayScroll.requestLayout();
    }

    void showControllerLayoutDialog() {
        if (game.modernController != null) {
            // The modern controller edits in place: drag, pinch and the panel instead of modes.
            game.modernController.startEditing();
            return;
        }
        game.menuActions.showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.stream_controls_layout)
                .setItems(R.array.stream_control_layout_actions, (dialog, which) -> {
                    VirtualController.ControllerMode[] modes = {
                            VirtualController.ControllerMode.MoveButtons,
                            VirtualController.ControllerMode.ResizeButtons,
                            VirtualController.ControllerMode.Active};
                    game.virtualController.setControllerMode(modes[which]);
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    void setInputGrabState(boolean grab) {
        if (game.inputCaptureProvider == null) {
            return;
        }
        grab = grab && game.conn != null && game.conn.canSendInput();
        // Grab/ungrab the mouse cursor
        if (grab) {
            game.inputCaptureProvider.enableCapture();

            // Enabling capture may hide the cursor again, so
            // we will need to show it again.
            if (game.cursorVisible || game.keyboardInput.isKeyboardOverlayVisible() || game.keyboardInput.isPcKeyboardVisible()) {
                game.inputCaptureProvider.showCursor();
            }
        }
        else {
            if (game.gameTouchControls != null) game.gameTouchControls.releaseInput();
            cancelTouchInput();
            game.keyboardInput.releaseKeyboardInput();
            releaseMouseButtons();
            if (game.virtualController != null) {
                game.virtualController.releaseInput();
            }
            if (game.modernController != null) {
                game.modernController.releaseInput();
            }
            game.inputCaptureProvider.disableCapture();
        }

        // Grab/ungrab system keyboard shortcuts, unless the user leaves them to Android
        game.setMetaKeyCaptureState(grab && game.prefConfig.sendsSystemKeys(game.inWindow()));

        game.grabbedInput = grab;
        if (game.controllerHandler != null) {
            game.controllerHandler.setInputEnabled(grab && game.hasWindowFocus());
        }
    }

    /** The alternative finger trackpad with the physical touchpad's gestures; Trackpad and Local cursor keep their own. */
    private boolean usesTrackpadGestures() {
        return game.prefConfig.touchscreenTrackpad && touchMode() == TouchMode.TOUCHPAD;
    }

    private TouchContext getTouchContext(int actionIndex)
    {
        if (actionIndex < touchContextMap.length) {
            return touchContextMap[actionIndex];
        }
        else {
            return null;
        }
    }

    boolean canSendDesktopInput() {
        return game.foreground && game.connected && game.grabbedInput && (game.desktopWindowFocused || game.dispatchingDesktopTouch) && !game.isFinishing() &&
                game.streamMenu == null && game.conn != null && game.conn.canSendInput() && game.keyboardInput.isKeyboardOverlayVisible();
    }

    void sendDesktopMotion(float dx, float dy) {
        if (!canSendDesktopInput() || !Float.isFinite(dx) || !Float.isFinite(dy)) return;
        if (game.trackpadPointer != null) {
            if (game.streamView.getWidth() <= 0 || game.streamView.getHeight() <= 0) return;
            float zoom = game.trackpadPointer.isLocalCursor() ? game.trackpadPointer.zoomScale() : 1;
            dx *= game.prefConfig.width / (game.streamView.getWidth() * zoom);
            dy *= game.prefConfig.height / (game.streamView.getHeight() * zoom);
            game.trackpadPointer.stopMomentum();
        }
        short x = desktopDelta(desktopDeltaX, dx);
        short y = desktopDelta(desktopDeltaY, dy);
        if (x != 0 || y != 0) {
            if (game.trackpadPointer != null) game.trackpadPointer.moveBy(x, y);
            else game.conn.sendMouseMove(x, y);
        }
    }

    void sendDesktopScroll(float vertical, float horizontal) {
        if (!canSendDesktopInput() || !Float.isFinite(vertical) || !Float.isFinite(horizontal)) return;
        short v = desktopDelta(desktopScrollV, vertical);
        short h = desktopDelta(desktopScrollH, horizontal);
        if (v != 0) game.conn.sendMouseHighResScroll(v);
        if (h != 0) game.conn.sendMouseHighResHScroll(h);
    }

    private static short desktopDelta(MouseDeltaAccumulator accumulator, float value) {
        int delta = accumulator.scale(Math.max(Short.MIN_VALUE, Math.min(Short.MAX_VALUE, value)), 1);
        return (short) Math.max(Short.MIN_VALUE, Math.min(Short.MAX_VALUE, delta));
    }

    void sendDesktopButton(int button, boolean down) {
        if (button < 1 || button > 5 || game.conn == null || (down && !canSendDesktopInput())) return;
        int mask = 1 << button;
        boolean wasDown = (desktopHeldButtons & mask) != 0;
        if (wasDown == down) return;
        if (down) {
            desktopHeldButtons |= mask;
            if (game.trackpadPointer != null) game.trackpadPointer.stopMomentum();
        }
        else desktopHeldButtons &= ~mask;
        // Releases must also run after focus/grab was lost. The connection merges this panel's
        // ownership separately from physical input and per-game touch controls.
        game.conn.sendDesktopMouseButton((byte) button, down);
    }

    void releaseDesktopInput() {
        if (game.keyboardOverlay != null) game.keyboardOverlay.releaseInputs();
        // Also clear ownership if a view was removed or interrupted between its callbacks.
        for (int button = 1; button <= 5; button++) sendDesktopButton(button, false);
        desktopDeltaX = new MouseDeltaAccumulator();
        desktopDeltaY = new MouseDeltaAccumulator();
        desktopScrollV = new MouseDeltaAccumulator();
        desktopScrollH = new MouseDeltaAccumulator();
    }

    private byte getLiTouchTypeFromEvent(MotionEvent event) {
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN:
                return MoonBridge.LI_TOUCH_EVENT_DOWN;

            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP:
                if ((event.getFlags() & MotionEvent.FLAG_CANCELED) != 0) {
                    return MoonBridge.LI_TOUCH_EVENT_CANCEL;
                }
                else {
                    return MoonBridge.LI_TOUCH_EVENT_UP;
                }

            case MotionEvent.ACTION_MOVE:
                return MoonBridge.LI_TOUCH_EVENT_MOVE;

            case MotionEvent.ACTION_CANCEL:
                // ACTION_CANCEL applies to *all* pointers in the gesture, so it maps to CANCEL_ALL
                // rather than CANCEL. For a single pointer cancellation, that's indicated via
                // FLAG_CANCELED on a ACTION_POINTER_UP.
                // https://developer.android.com/develop/ui/views/touch-and-input/gestures/multi
                return MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL;

            case MotionEvent.ACTION_HOVER_ENTER:
            case MotionEvent.ACTION_HOVER_MOVE:
                return MoonBridge.LI_TOUCH_EVENT_HOVER;

            case MotionEvent.ACTION_HOVER_EXIT:
                return MoonBridge.LI_TOUCH_EVENT_HOVER_LEAVE;

            case MotionEvent.ACTION_BUTTON_PRESS:
            case MotionEvent.ACTION_BUTTON_RELEASE:
                return MoonBridge.LI_TOUCH_EVENT_BUTTON_ONLY;

            default:
               return -1;
        }
    }

    // Where a pointer is in the stream view, as a share of its width. This and the methods below run
    // for every pointer of every touch and pen event, so each returns one value and none an array.
    private float getStreamViewRelativeNormalizedX(View view, MotionEvent event, int pointerIndex) {
        float normalizedX = event.getX(pointerIndex) + getStreamViewOffsetX(view);
        normalizedX = Math.max(normalizedX, 0.0f);
        normalizedX = Math.min(normalizedX, game.streamView.getWidth());
        return normalizedX / game.streamView.getWidth();
    }

    private float getStreamViewRelativeNormalizedY(View view, MotionEvent event, int pointerIndex) {
        float normalizedY = event.getY(pointerIndex) + getStreamViewOffsetY(view);
        normalizedY = Math.max(normalizedY, 0.0f);
        normalizedY = Math.min(normalizedY, game.streamView.getHeight());
        return normalizedY / game.streamView.getHeight();
    }

    private float getStreamViewOffsetX(View view) {
        // Sibling views share the pane padding, including fold and display-cutout insets.
        return (view != null ? view.getX() : 0) - game.streamView.getX();
    }

    float getStreamViewOffsetY(View view) {
        return (view != null ? view.getY() : 0) - game.streamView.getY();
    }

    private static float normalizeValueInRange(float value, InputDevice.MotionRange range) {
        return (value - range.getMin()) / range.getRange();
    }

    private static float getPressureOrDistance(MotionEvent event, int pointerIndex) {
        InputDevice dev = event.getDevice();
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_HOVER_ENTER:
            case MotionEvent.ACTION_HOVER_MOVE:
            case MotionEvent.ACTION_HOVER_EXIT:
                // Hover events report distance
                if (dev != null) {
                    InputDevice.MotionRange distanceRange = dev.getMotionRange(MotionEvent.AXIS_DISTANCE, event.getSource());
                    if (distanceRange != null) {
                        return normalizeValueInRange(event.getAxisValue(MotionEvent.AXIS_DISTANCE, pointerIndex), distanceRange);
                    }
                }
                return 0.0f;

            default:
                // Other events report pressure
                return event.getPressure(pointerIndex);
        }
    }

    private static short getRotationDegrees(MotionEvent event, int pointerIndex) {
        InputDevice dev = event.getDevice();
        if (dev != null) {
            if (dev.getMotionRange(MotionEvent.AXIS_ORIENTATION, event.getSource()) != null) {
                short rotationDegrees = (short) Math.toDegrees(event.getOrientation(pointerIndex));
                if (rotationDegrees < 0) {
                    rotationDegrees += 360;
                }
                return rotationDegrees;
            }
        }
        return MoonBridge.LI_ROT_UNKNOWN;
    }

    private static float getContactAreaOrientation(MotionEvent event, int pointerIndex) {
        // If the orientation is unknown, we'll just assume it's at a 45 degree angle and scale it by
        // X and Y scaling factors evenly.
        if (event.getDevice() == null || event.getDevice().getMotionRange(MotionEvent.AXIS_ORIENTATION, event.getSource()) == null) {
            return (float)(Math.PI / 4);
        }
        else {
            return event.getOrientation(pointerIndex);
        }
    }

    // Hover events report the tool size, other events the contact area.
    private static boolean reportsToolSize(MotionEvent event) {
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_HOVER_ENTER:
            case MotionEvent.ACTION_HOVER_MOVE:
            case MotionEvent.ACTION_HOVER_EXIT:
                return true;

            default:
                return false;
        }
    }

    // One axis of a contact area, r pixels long at the angle theta, as a share of the stream view.
    private float getStreamViewNormalizedContactAxis(float r, float theta) {
        // Convert polar to cartesian coordinates, and normalize each to the stream view size
        float x = Math.min(Math.abs((float)(r * Math.cos(theta))), game.streamView.getWidth()) / game.streamView.getWidth();
        float y = Math.min(Math.abs((float)(r * Math.sin(theta))), game.streamView.getHeight()) / game.streamView.getHeight();

        // Convert the normalized values back into polar coordinates
        return (float)Math.sqrt(Math.pow(x, 2) + Math.pow(y, 2));
    }

    private float getStreamViewNormalizedContactAreaMajor(MotionEvent event, int pointerIndex, float orientation) {
        // The contact area major axis is parallel to the orientation, so we simply convert
        // polar to cartesian coordinates using the orientation as theta.
        return getStreamViewNormalizedContactAxis(
                reportsToolSize(event) ? event.getToolMajor(pointerIndex) : event.getTouchMajor(pointerIndex),
                orientation);
    }

    private float getStreamViewNormalizedContactAreaMinor(MotionEvent event, int pointerIndex, float orientation) {
        // The contact area minor axis is perpendicular to the contact area major axis (and thus
        // the orientation), so rotate the orientation angle by 90 degrees.
        return getStreamViewNormalizedContactAxis(
                reportsToolSize(event) ? event.getToolMinor(pointerIndex) : event.getTouchMinor(pointerIndex),
                (float)(orientation + (Math.PI / 2)));
    }

    private boolean sendPenEventForPointer(View view, MotionEvent event, byte eventType, byte toolType, int pointerIndex) {
        byte penButtons = 0;
        if ((event.getButtonState() & MotionEvent.BUTTON_STYLUS_PRIMARY) != 0) {
            penButtons |= MoonBridge.LI_PEN_BUTTON_PRIMARY;
        }
        if ((event.getButtonState() & MotionEvent.BUTTON_STYLUS_SECONDARY) != 0) {
            penButtons |= MoonBridge.LI_PEN_BUTTON_SECONDARY;
        }

        byte tiltDegrees = MoonBridge.LI_TILT_UNKNOWN;
        InputDevice dev = event.getDevice();
        if (dev != null) {
            if (dev.getMotionRange(MotionEvent.AXIS_TILT, event.getSource()) != null) {
                tiltDegrees = (byte)Math.toDegrees(event.getAxisValue(MotionEvent.AXIS_TILT, pointerIndex));
            }
        }

        float orientation = getContactAreaOrientation(event, pointerIndex);
        return game.conn.sendPenEvent(eventType, toolType, penButtons,
                getStreamViewRelativeNormalizedX(view, event, pointerIndex),
                getStreamViewRelativeNormalizedY(view, event, pointerIndex),
                getPressureOrDistance(event, pointerIndex),
                getStreamViewNormalizedContactAreaMajor(event, pointerIndex, orientation),
                getStreamViewNormalizedContactAreaMinor(event, pointerIndex, orientation),
                getRotationDegrees(event, pointerIndex), tiltDegrees) != MoonBridge.LI_ERR_UNSUPPORTED;
    }

    private static byte convertToolTypeToStylusToolType(MotionEvent event, int pointerIndex) {
        switch (event.getToolType(pointerIndex)) {
            case MotionEvent.TOOL_TYPE_ERASER:
                return MoonBridge.LI_TOOL_TYPE_ERASER;
            case MotionEvent.TOOL_TYPE_STYLUS:
                return MoonBridge.LI_TOOL_TYPE_PEN;
            default:
                return MoonBridge.LI_TOOL_TYPE_UNKNOWN;
        }
    }

    private boolean trySendPenEvent(View view, MotionEvent event) {
        byte eventType = getLiTouchTypeFromEvent(event);
        if (eventType < 0) {
            return false;
        }

        if (event.getActionMasked() == MotionEvent.ACTION_MOVE) {
            // Move events may impact all active pointers
            boolean handledStylusEvent = false;
            for (int i = 0; i < event.getPointerCount(); i++) {
                byte toolType = convertToolTypeToStylusToolType(event, i);
                if (toolType == MoonBridge.LI_TOOL_TYPE_UNKNOWN) {
                    // Not a stylus pointer, so skip it
                    continue;
                }
                else {
                    // This pointer is a stylus, so we'll report that we handled this event
                    handledStylusEvent = true;
                }

                if (!sendPenEventForPointer(view, event, eventType, toolType, i)) {
                    // Pen events aren't supported by the host
                    return false;
                }
            }
            return handledStylusEvent;
        }
        else if (event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
            // Cancel impacts all active pointers
            return game.conn.sendPenEvent(MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL, MoonBridge.LI_TOOL_TYPE_UNKNOWN, (byte)0,
                    0, 0, 0, 0, 0,
                    MoonBridge.LI_ROT_UNKNOWN, MoonBridge.LI_TILT_UNKNOWN) != MoonBridge.LI_ERR_UNSUPPORTED;
        }
        else {
            // Up, Down, and Hover events are specific to the action index
            byte toolType = convertToolTypeToStylusToolType(event, event.getActionIndex());
            if (toolType == MoonBridge.LI_TOOL_TYPE_UNKNOWN) {
                // Not a stylus event
                return false;
            }
            return sendPenEventForPointer(view, event, eventType, toolType, event.getActionIndex());
        }
    }

    private boolean sendTouchEventForPointer(View view, MotionEvent event, byte eventType, int pointerIndex) {
        float orientation = getContactAreaOrientation(event, pointerIndex);
        return game.conn.sendTouchEvent(eventType, event.getPointerId(pointerIndex),
                getStreamViewRelativeNormalizedX(view, event, pointerIndex),
                getStreamViewRelativeNormalizedY(view, event, pointerIndex),
                getPressureOrDistance(event, pointerIndex),
                getStreamViewNormalizedContactAreaMajor(event, pointerIndex, orientation),
                getStreamViewNormalizedContactAreaMinor(event, pointerIndex, orientation),
                getRotationDegrees(event, pointerIndex)) != MoonBridge.LI_ERR_UNSUPPORTED;
    }

    private boolean trySendTouchEvent(View view, MotionEvent event) {
        byte eventType = getLiTouchTypeFromEvent(event);
        if (eventType < 0) {
            return false;
        }

        if (event.getActionMasked() == MotionEvent.ACTION_MOVE) {
            // Move events may impact all active pointers
            for (int i = 0; i < event.getPointerCount(); i++) {
                if (!sendTouchEventForPointer(view, event, eventType, i)) {
                    return false;
                }
            }
            return true;
        }
        else if (event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
            // Cancel impacts all active pointers
            return game.conn.sendTouchEvent(MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL, 0,
                    0, 0, 0, 0, 0,
                    MoonBridge.LI_ROT_UNKNOWN) != MoonBridge.LI_ERR_UNSUPPORTED;
        }
        else {
            // Up, Down, and Hover events are specific to the action index
            return sendTouchEventForPointer(view, event, eventType, event.getActionIndex());
        }
    }

    // Returns true if the event was consumed
    // NB: View is only present if called from a view callback
    boolean handleMotionEvent(View view, MotionEvent event) {
        game.keyboardInput.noteStreamPress(view, event);
        if (game.gameTouchControls != null && game.gameTouchControls.isEditing() &&
                (event.getSource() & InputDevice.SOURCE_CLASS_POINTER) != 0) return true;
        if (game.desktopGestures != null && game.desktopGestures.enabled() &&
                game.desktopGestures.viewport().scale() != 1 && event.getPointerCount() > 0 &&
                (event.getSource() & InputDevice.SOURCE_CLASS_POINTER) != 0 &&
                (!game.prefConfig.touchscreenTrackpad || event.getToolType(0) != MotionEvent.TOOL_TYPE_FINGER)) {
            MotionEvent translated = MotionEvent.obtain(event);
            translated.offsetLocation(getStreamViewOffsetX(view), getStreamViewOffsetY(view));
            com.limelight.ui.DesktopViewport viewport = game.desktopGestures.viewport();
            float[] values = zoomInverseValues;
            values[0] = 1 / viewport.scale(); values[1] = 0; values[2] = -viewport.x() / viewport.scale();
            values[3] = 0; values[4] = 1 / viewport.scale(); values[5] = -viewport.y() / viewport.scale();
            values[6] = 0; values[7] = 0; values[8] = 1;
            zoomInverse.setValues(values);
            translated.transform(zoomInverse);
            try { return handleMotionEventUntransformed(game.streamView, translated); }
            finally { translated.recycle(); }
        }
        return handleMotionEventUntransformed(view, event);
    }

    private boolean handleMotionEventUntransformed(View view, MotionEvent event) {
        // Pass through mouse/touch/joystick input if we're not grabbing
        if (!game.grabbedInput) {
            return false;
        }

        int eventSource = event.getSource();
        int deviceSources = event.getDevice() != null ? event.getDevice().getSources() : 0;
        if ((eventSource & InputDevice.SOURCE_CLASS_JOYSTICK) != 0) {
            if (game.keyboardInput.isPcKeyboardVisible()) {
                game.keyboardInput.handlePcKeyboardMotion(event);
                return true;
            }
            if (game.controllerHandler.handleMotionEvent(event)) {
                return true;
            }
        }
        else if ((deviceSources & InputDevice.SOURCE_CLASS_JOYSTICK) != 0 && game.controllerHandler.tryHandleTouchpadEvent(event)) {
            return true;
        }
        else if ((eventSource & InputDevice.SOURCE_CLASS_POINTER) != 0 ||
                 (eventSource & InputDevice.SOURCE_CLASS_POSITION) != 0 ||
                 eventSource == InputDevice.SOURCE_MOUSE_RELATIVE)
        {
            // A touchpad's finger touches carry no action button; only its clicks do. Clicks are handled
            // below like a mouse, and the touches go to the trackpad gesture handling.
            boolean hasActionButton = Build.VERSION.SDK_INT < Build.VERSION_CODES.M || event.getActionButton() != 0;

            // This case is for mice and non-finger touch devices
            if (eventSource == InputDevice.SOURCE_MOUSE ||
                    ((eventSource & InputDevice.SOURCE_CLASS_POSITION) != 0 && hasActionButton) || // SOURCE_TOUCHPAD
                    eventSource == InputDevice.SOURCE_MOUSE_RELATIVE ||
                    (event.getPointerCount() >= 1 &&
                            (event.getToolType(0) == MotionEvent.TOOL_TYPE_MOUSE ||
                                    event.getToolType(0) == MotionEvent.TOOL_TYPE_STYLUS ||
                                    event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER)) ||
                    eventSource == 12290) // 12290 = Samsung DeX mode desktop mouse
            {
                int buttonState = event.getButtonState();
                int changedButtons = buttonState ^ lastButtonState;

                // The DeX touchpad on the Fold 4 sends proper right click events using BUTTON_SECONDARY,
                // but doesn't send BUTTON_PRIMARY for a regular click. Instead it sends ACTION_DOWN/UP,
                // so we need to fix that up to look like a sane input event to process it correctly.
                if (eventSource == 12290) {
                    if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
                        buttonState |= MotionEvent.BUTTON_PRIMARY;
                    }
                    else if (event.getAction() == MotionEvent.ACTION_UP) {
                        buttonState &= ~MotionEvent.BUTTON_PRIMARY;
                    }
                    else {
                        // We may be faking the primary button down from a previous event,
                        // so be sure to add that bit back into the button state.
                        buttonState |= (lastButtonState & MotionEvent.BUTTON_PRIMARY);
                    }

                    changedButtons = buttonState ^ lastButtonState;
                }

                // Ignore mouse input if we're not capturing from our input source
                if (!game.inputCaptureProvider.isCapturingActive()) {
                    // We return true here because otherwise the events may end up causing
                    // Android to synthesize d-pad events.
                    return true;
                }

                // The touchpad's click rides on its frames, with the fingers on it.
                if (eventSource == InputDevice.SOURCE_TOUCHPAD && nativeTouchpad()) {
                    sendNativeTouchpadFrame(event);
                    return true;
                }

                // Always update the position before sending any button events. If we're
                // dealing with a stylus without hover support, our position might be
                // significantly different than before.
                if (game.inputCaptureProvider.eventHasRelativeMouseAxes(event)) {
                    short deltaX = (short)mouseDeltaX.scale(game.inputCaptureProvider.getRelativeAxisX(event), game.prefConfig.mouseSpeed / 100f);
                    short deltaY = (short)mouseDeltaY.scale(game.inputCaptureProvider.getRelativeAxisY(event), game.prefConfig.mouseSpeed / 100f);

                    if (deltaX != 0 || deltaY != 0) {
                        if (game.prefConfig.absoluteMouseMode) {
                            // NB: view may be null, but we can unconditionally use streamView because we don't need to adjust
                            // relative axis deltas for the position of the streamView within the parent's coordinate system.
                            game.conn.sendMouseMoveAsMousePosition(deltaX, deltaY, (short)game.streamView.getWidth(), (short)game.streamView.getHeight());
                        }
                        else {
                            game.conn.sendMouseMove(deltaX, deltaY);
                        }
                    }
                }
                else if ((eventSource & InputDevice.SOURCE_CLASS_POSITION) != 0) {
                    // If this input device is not associated with the view itself (like a trackpad),
                    // we'll convert the device-specific coordinates to use to send the cursor position.
                    // This really isn't ideal but it's probably better than nothing.
                    //
                    // Trackpad on newer versions of Android (Oreo and later) should be caught by the
                    // relative axes case above. If we get here, we're on an older version that doesn't
                    // support pointer capture.
                    InputDevice device = event.getDevice();
                    if (device != null) {
                        InputDevice.MotionRange xRange = device.getMotionRange(MotionEvent.AXIS_X, eventSource);
                        InputDevice.MotionRange yRange = device.getMotionRange(MotionEvent.AXIS_Y, eventSource);

                        // All touchpads coordinate planes should start at (0, 0)
                        if (xRange != null && yRange != null && xRange.getMin() == 0 && yRange.getMin() == 0) {
                            int xMax = (int)xRange.getMax();
                            int yMax = (int)yRange.getMax();

                            // Touchpads must be smaller than (65535, 65535)
                            if (xMax <= Short.MAX_VALUE && yMax <= Short.MAX_VALUE) {
                                game.conn.sendMousePosition((short)event.getX(), (short)event.getY(),
                                                       (short)xMax, (short)yMax);
                            }
                        }
                    }
                }
                else if (view != null && trySendPenEvent(view, event)) {
                    // If our host supports pen events, send it directly
                    return true;
                }
                else if (view != null) {
                    // Otherwise send absolute position based on the view for SOURCE_CLASS_POINTER
                    updateMousePosition(view, event);
                }

                if (event.getActionMasked() == MotionEvent.ACTION_SCROLL) {
                    // Send the vertical scroll packet
                    game.conn.sendMouseHighResScroll((short)(event.getAxisValue(MotionEvent.AXIS_VSCROLL) * 120));
                    game.conn.sendMouseHighResHScroll((short)(event.getAxisValue(MotionEvent.AXIS_HSCROLL) * 120));
                }

                if ((changedButtons & MotionEvent.BUTTON_PRIMARY) != 0) {
                    if ((buttonState & MotionEvent.BUTTON_PRIMARY) != 0) {
                        game.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_LEFT);
                    }
                    else {
                        game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_LEFT);
                    }
                }

                // Mouse secondary or stylus primary is right click (stylus down is left click)
                if ((changedButtons & (MotionEvent.BUTTON_SECONDARY | MotionEvent.BUTTON_STYLUS_PRIMARY)) != 0) {
                    if ((buttonState & (MotionEvent.BUTTON_SECONDARY | MotionEvent.BUTTON_STYLUS_PRIMARY)) != 0) {
                        game.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_RIGHT);
                    }
                    else {
                        game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
                    }
                }

                // Mouse tertiary or stylus secondary is middle click
                if ((changedButtons & (MotionEvent.BUTTON_TERTIARY | MotionEvent.BUTTON_STYLUS_SECONDARY)) != 0) {
                    if ((buttonState & (MotionEvent.BUTTON_TERTIARY | MotionEvent.BUTTON_STYLUS_SECONDARY)) != 0) {
                        game.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_MIDDLE);
                    }
                    else {
                        game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_MIDDLE);
                    }
                }

                if (game.prefConfig.mouseNavButtons) {
                    if ((changedButtons & MotionEvent.BUTTON_BACK) != 0) {
                        if ((buttonState & MotionEvent.BUTTON_BACK) != 0) {
                            game.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_X1);
                        }
                        else {
                            game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_X1);
                        }
                    }

                    if ((changedButtons & MotionEvent.BUTTON_FORWARD) != 0) {
                        if ((buttonState & MotionEvent.BUTTON_FORWARD) != 0) {
                            game.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_X2);
                        }
                        else {
                            game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_X2);
                        }
                    }
                }

                // Handle stylus presses
                if (event.getPointerCount() == 1 && event.getActionIndex() == 0) {
                    if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
                        if (event.getToolType(0) == MotionEvent.TOOL_TYPE_STYLUS) {
                            lastAbsTouchDownTime = event.getEventTime();
                            lastAbsTouchDownX = event.getX(0) + getStreamViewOffsetX(view);
                            lastAbsTouchDownY = event.getY(0) + getStreamViewOffsetY(view);

                            // Stylus is left click
                            game.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_LEFT);
                        } else if (event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER) {
                            lastAbsTouchDownTime = event.getEventTime();
                            lastAbsTouchDownX = event.getX(0) + getStreamViewOffsetX(view);
                            lastAbsTouchDownY = event.getY(0) + getStreamViewOffsetY(view);

                            // Eraser is right click
                            game.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_RIGHT);
                        }
                    }
                    else if (event.getActionMasked() == MotionEvent.ACTION_UP || event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
                        if (event.getToolType(0) == MotionEvent.TOOL_TYPE_STYLUS) {
                            lastAbsTouchUpTime = event.getEventTime();
                            lastAbsTouchUpX = event.getX(0) + getStreamViewOffsetX(view);
                            lastAbsTouchUpY = event.getY(0) + getStreamViewOffsetY(view);

                            // Stylus is left click
                            game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_LEFT);
                        } else if (event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER) {
                            lastAbsTouchUpTime = event.getEventTime();
                            lastAbsTouchUpX = event.getX(0) + getStreamViewOffsetX(view);
                            lastAbsTouchUpY = event.getY(0) + getStreamViewOffsetY(view);

                            // Eraser is right click
                            game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
                        }
                    }
                }

                lastButtonState = buttonState;
            }
            // This case is for fingers
            else
            {
                if (eventSource == InputDevice.SOURCE_TOUCHPAD) {
                    // Like the mouse case, ignore the touchpad while we aren't capturing it; returning
                    // true keeps Android from turning the touches into d-pad events.
                    if (!game.inputCaptureProvider.isCapturingActive()) {
                        cancelNativeTouchpad();
                        return true;
                    }
                    if (nativeTouchpad()) {
                        sendNativeTouchpadFrame(event);
                        return true;
                    }
                    return handleTouchpadTouch(event);
                }

                if ((game.virtualController != null &&
                        (game.virtualController.getControllerMode() == VirtualController.ControllerMode.MoveButtons ||
                         game.virtualController.getControllerMode() == VirtualController.ControllerMode.ResizeButtons)) ||
                        (game.modernController != null && game.modernController.isEditing())) {
                    // Ignore presses when the virtual controller is being configured
                    return true;
                }

                if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
                    int mode = touchMode();
                    touchGestureIgnored = mode == TouchMode.OFF ||
                            (TouchMode.isRelative(mode) && !isInTrackpadArea(view, event));
                }
                if (touchGestureIgnored) {
                    if (event.getActionMasked() == MotionEvent.ACTION_UP ||
                            event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
                        touchGestureIgnored = false;
                    }
                    return true;
                }

                if (game.nativeTouchEnabled && !nativeTouchUnsupported) {
                    if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
                        nativeTouchGestureActive = true;
                    }
                    if (!nativeTouchGestureActive) {
                        return true;
                    }
                    if (game.prefConfig.multiTouchFingerTaps) {
                        boolean tapBefore = fingerTap.isStarted();
                        if (handleFingerTapGesture(event)) {
                            // The third finger starts a tap: the PC lets go of the two it already has,
                            // and the rest of the sequence stays here.
                            if (!tapBefore) {
                                game.conn.sendTouchEvent(MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL, 0, 0, 0, 0, 0, 0,
                                        MoonBridge.LI_ROT_UNKNOWN);
                            }
                            if (event.getActionMasked() == MotionEvent.ACTION_UP ||
                                    event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
                                nativeTouchGestureActive = false;
                            }
                            return true;
                        }
                    }
                    if (trySendTouchEvent(view, event)) {
                        if (event.getActionMasked() == MotionEvent.ACTION_UP ||
                                event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
                            nativeTouchGestureActive = false;
                        }
                        return true;
                    }
                    nativeTouchGestureActive = false;
                    nativeTouchUnsupported = true;
                    game.overlays.hostNotice(HostNotices.Notice.NATIVE_TOUCH_UNAVAILABLE, R.string.stream_native_touch_fallback);
                }

                // The touchpad gestures right-click on a two-finger tap themselves.
                boolean rightTap = game.prefConfig.touchscreenTrackpad && !usesTrackpadGestures() &&
                        twoFingerTap.onTouch(event, game.getResources().getDisplayMetrics().density);
                if (handleFingerTapGesture(event)) return true;

                // If this is the parent view, we'll offset our coordinates to appear as if they
                // are relative to the StreamView like our StreamView touch events are.
                float xOffset, yOffset;
                if (view != game.streamView && !game.prefConfig.touchscreenTrackpad) {
                    xOffset = getStreamViewOffsetX(view);
                    yOffset = getStreamViewOffsetY(view);
                }
                else {
                    xOffset = 0.f;
                    yOffset = 0.f;
                }

                int actionIndex = event.getActionIndex();

                int eventX = (int)(event.getX(actionIndex) + xOffset);
                int eventY = (int)(event.getY(actionIndex) + yOffset);

                TouchContext context = getTouchContext(actionIndex);
                if (context == null) {
                    return false;
                }

                switch (event.getActionMasked())
                {
                case MotionEvent.ACTION_POINTER_DOWN:
                case MotionEvent.ACTION_DOWN:
                    for (TouchContext touchContext : touchContextMap) {
                        touchContext.setPointerCount(event.getPointerCount());
                    }
                    context.touchDownEvent(eventX, eventY, event.getEventTime(), true);
                    break;
                case MotionEvent.ACTION_POINTER_UP:
                case MotionEvent.ACTION_UP:
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && (event.getFlags() & MotionEvent.FLAG_CANCELED) != 0) {
                        context.cancelTouch();
                    }
                    else {
                        context.touchUpEvent(eventX, eventY, event.getEventTime());
                    }

                    for (TouchContext touchContext : touchContextMap) {
                        touchContext.setPointerCount(event.getPointerCount() - 1);
                    }
                    if (actionIndex == 0 && event.getPointerCount() > 1 && !context.isCancelled()) {
                        // The original secondary touch now becomes primary
                        context.touchDownEvent(
                                (int)(event.getX(1) + xOffset),
                                (int)(event.getY(1) + yOffset),
                                event.getEventTime(), false);
                    }
                    break;
                case MotionEvent.ACTION_MOVE:
                    // ACTION_MOVE is special because it always has actionIndex == 0
                    // We'll call the move handlers for all indexes manually
                    // First process the historical events
                    for (int i = 0; i < event.getHistorySize(); i++) {
                        for (TouchContext aTouchContextMap : touchContextMap) {
                            if (aTouchContextMap.getActionIndex() < event.getPointerCount())
                            {
                                aTouchContextMap.touchMoveEvent(
                                        (int)(event.getHistoricalX(aTouchContextMap.getActionIndex(), i) + xOffset),
                                        (int)(event.getHistoricalY(aTouchContextMap.getActionIndex(), i) + yOffset),
                                        event.getHistoricalEventTime(i));
                            }
                        }
                    }

                    // Now process the current values
                    for (TouchContext aTouchContextMap : touchContextMap) {
                        if (aTouchContextMap.getActionIndex() < event.getPointerCount())
                        {
                            aTouchContextMap.touchMoveEvent(
                                    (int)(event.getX(aTouchContextMap.getActionIndex()) + xOffset),
                                    (int)(event.getY(aTouchContextMap.getActionIndex()) + yOffset),
                                    event.getEventTime());
                        }
                    }
                    break;
                case MotionEvent.ACTION_CANCEL:
                    for (TouchContext aTouchContext : touchContextMap) {
                        aTouchContext.cancelTouch();
                        aTouchContext.setPointerCount(0);
                    }
                    break;
                default:
                    return false;
                }
                if (rightTap) clickRightMouseButton();
            }

            // Handled a known source
            return true;
        }

        // Unknown class
        return false;
    }

    /** Tells the touchpad gestures how many input units are a millimetre, so scrolling does not depend on the device. */
    private void updateTouchpadResolution(MotionEvent event) {
        if (event.getDeviceId() == touchpadResolutionDevice) {
            return;
        }
        touchpadResolutionDevice = event.getDeviceId();
        float unitsPerMm = 0;
        InputDevice device = event.getDevice();
        InputDevice.MotionRange range = device != null ? device.getMotionRange(MotionEvent.AXIS_X, event.getSource()) : null;
        if (range != null) {
            unitsPerMm = range.getResolution();
            if (unitsPerMm <= 0) {
                // Not reported: assume a pad about 100 mm wide.
                unitsPerMm = range.getRange() / 100f;
            }
        }
        for (TrackpadContext context : touchpadContextMap) {
            context.setUnitsPerMm(unitsPerMm);
        }
    }

    /**
     * Whether a physical touchpad's fingers go to the PC this stream (Touchpad gestures on the PC and a
     * PC that takes them), decided at the touchpad's first event; otherwise it stays a mouse, and the
     * PC is named once.
     */
    private boolean nativeTouchpad() {
        if (nativeTouchpadState == 0) {
            if (!game.prefConfig.nativeTouchpad) {
                nativeTouchpadState = 2;
            } else if (game.conn.touchpadTransport() != MoonBridge.TOUCHPAD_NONE) {
                nativeTouchpadState = 1;
            } else {
                nativeTouchpadState = 2;
                LimeLog.warning("Touchpad gestures stay a mouse: this stream takes no touchpad fingers");
                game.overlays.hostNotice(HostNotices.Notice.NATIVE_TOUCHPAD_UNAVAILABLE, R.string.stream_native_touchpad_fallback);
            }
        }
        return nativeTouchpadState == 1;
    }

    /** Lifts the fingers and click the PC holds for the touchpad (capture or focus lost). */
    private void cancelNativeTouchpad() {
        if (nativeTouchpadHeld) {
            nativeTouchpadHeld = false;
            game.conn.cancelTouchpad();
        }
    }

    /** The touchpad's fingers still on it after this event, and its click, as one frame for the PC. */
    private void sendNativeTouchpadFrame(MotionEvent event) {
        int action = event.getActionMasked();
        if (action == MotionEvent.ACTION_CANCEL) {
            cancelNativeTouchpad();
            return;
        }
        int lifted = action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_POINTER_UP ? event.getActionIndex() : -1;
        int pointers = event.getPointerCount();
        // Reused buffers, grown only past their size: this runs for every touchpad event on the main thread.
        if (nativeTouchpadIds.length < pointers) {
            int size = Math.max(pointers, nativeTouchpadIds.length * 2);
            nativeTouchpadIds = new int[size];
            nativeTouchpadXs = new float[size];
            nativeTouchpadYs = new float[size];
            nativeTouchpadPressures = new float[size];
        }
        int[] ids = nativeTouchpadIds;
        float[] xs = nativeTouchpadXs;
        float[] ys = nativeTouchpadYs;
        float[] pressures = nativeTouchpadPressures;
        InputDevice device = event.getDevice();
        InputDevice.MotionRange xRange = device != null ? device.getMotionRange(MotionEvent.AXIS_X, InputDevice.SOURCE_TOUCHPAD) : null;
        InputDevice.MotionRange yRange = device != null ? device.getMotionRange(MotionEvent.AXIS_Y, InputDevice.SOURCE_TOUCHPAD) : null;
        int count = 0;
        for (int i = 0; i < pointers; i++) {
            if (i == lifted) {
                continue;
            }
            ids[count] = event.getPointerId(i);
            xs[count] = TouchpadSurface.unit(event.getX(i), xRange != null ? xRange.getMin() : 0, xRange != null ? xRange.getMax() : 0);
            ys[count] = TouchpadSurface.unit(event.getY(i), yRange != null ? yRange.getMin() : 0, yRange != null ? yRange.getMax() : 0);
            pressures[count] = Math.max(0f, Math.min(1f, event.getPressure(i)));
            count++;
        }
        boolean button = (event.getButtonState() & MotionEvent.BUTTON_PRIMARY) != 0;
        nativeTouchpadHeld = count > 0 || button;
        game.conn.sendTouchpadFrame(event.getDeviceId(), ids, xs, ys, pressures, count, button,
                TouchpadSurface.lengthMm(xRange != null ? xRange.getRange() : 0, xRange != null ? xRange.getResolution() : 0),
                TouchpadSurface.lengthMm(yRange != null ? yRange.getRange() : 0, yRange != null ? yRange.getResolution() : 0));
    }

    private boolean handleTouchpadTouch(MotionEvent event) {
        updateTouchpadResolution(event);
        int actionIndex = event.getActionIndex();
        int pointerCount = event.getPointerCount();
        TouchContext context = actionIndex < touchpadContextMap.length ? touchpadContextMap[actionIndex] : null;
        if (context == null) {
            return false;
        }

        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_POINTER_DOWN:
            case MotionEvent.ACTION_DOWN:
                for (TouchContext touchContext : touchpadContextMap) {
                    touchContext.setPointerCount(pointerCount);
                }
                context.touchDownEvent((int) event.getX(actionIndex), (int) event.getY(actionIndex), event.getEventTime(), true);
                break;
            case MotionEvent.ACTION_POINTER_UP:
            case MotionEvent.ACTION_UP:
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && (event.getFlags() & MotionEvent.FLAG_CANCELED) != 0) {
                    context.cancelTouch();
                }
                else {
                    context.touchUpEvent((int) event.getX(actionIndex), (int) event.getY(actionIndex), event.getEventTime());
                }
                for (TouchContext touchContext : touchpadContextMap) {
                    touchContext.setPointerCount(pointerCount - 1);
                }
                if (actionIndex == 0 && pointerCount > 1 && !context.isCancelled()) {
                    // The original secondary touch now becomes primary
                    context.touchDownEvent((int) event.getX(1), (int) event.getY(1), event.getEventTime(), false);
                }
                break;
            case MotionEvent.ACTION_MOVE:
                // ACTION_MOVE always has actionIndex 0, so call the move handler of every finger
                for (int i = 0; i < event.getHistorySize(); i++) {
                    for (TouchContext touchContext : touchpadContextMap) {
                        if (touchContext.getActionIndex() < pointerCount) {
                            touchContext.touchMoveEvent((int) event.getHistoricalX(touchContext.getActionIndex(), i),
                                    (int) event.getHistoricalY(touchContext.getActionIndex(), i), event.getHistoricalEventTime(i));
                        }
                    }
                }
                for (TouchContext touchContext : touchpadContextMap) {
                    if (touchContext.getActionIndex() < pointerCount) {
                        touchContext.touchMoveEvent((int) event.getX(touchContext.getActionIndex()),
                                (int) event.getY(touchContext.getActionIndex()), event.getEventTime());
                    }
                }
                break;
            case MotionEvent.ACTION_CANCEL:
                for (TouchContext touchContext : touchpadContextMap) {
                    touchContext.cancelTouch();
                    touchContext.setPointerCount(0);
                }
                break;
            default:
                return false;
        }
        return true;
    }

    private void updateMousePosition(View touchedView, MotionEvent event) {
        float eventX = event.getX(0) + getStreamViewOffsetX(touchedView);
        float eventY = event.getY(0) + getStreamViewOffsetY(touchedView);

        if (event.getPointerCount() == 1 && event.getActionIndex() == 0 &&
                (event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER ||
                event.getToolType(0) == MotionEvent.TOOL_TYPE_STYLUS))
        {
            switch (event.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                case MotionEvent.ACTION_HOVER_ENTER:
                case MotionEvent.ACTION_HOVER_EXIT:
                case MotionEvent.ACTION_HOVER_MOVE:
                    if (event.getEventTime() - lastAbsTouchUpTime <= STYLUS_UP_DEAD_ZONE_DELAY &&
                            Math.sqrt(Math.pow(eventX - lastAbsTouchUpX, 2) + Math.pow(eventY - lastAbsTouchUpY, 2)) <= STYLUS_UP_DEAD_ZONE_RADIUS) {
                        // Enforce a small deadzone between touch up and hover or touch down to allow more precise double-clicking
                        return;
                    }
                    break;

                case MotionEvent.ACTION_MOVE:
                case MotionEvent.ACTION_UP:
                    if (event.getEventTime() - lastAbsTouchDownTime <= STYLUS_DOWN_DEAD_ZONE_DELAY &&
                            Math.sqrt(Math.pow(eventX - lastAbsTouchDownX, 2) + Math.pow(eventY - lastAbsTouchDownY, 2)) <= STYLUS_DOWN_DEAD_ZONE_RADIUS) {
                        // Enforce a small deadzone between touch down and move or touch up to allow more precise double-clicking
                        return;
                    }
                    break;
            }
        }

        // We may get values slightly outside our view region on ACTION_HOVER_ENTER and ACTION_HOVER_EXIT.
        // Normalize these to the view size. We can't just drop them because we won't always get an event
        // right at the boundary of the view, so dropping them would result in our cursor never really
        // reaching the sides of the screen.
        eventX = Math.min(Math.max(eventX, 0), game.streamView.getWidth());
        eventY = Math.min(Math.max(eventY, 0), game.streamView.getHeight());

        game.conn.sendMousePosition((short)eventX, (short)eventY, (short)game.streamView.getWidth(), (short)game.streamView.getHeight());
    }

    // Fingers beside the per-game touch controls reach the stream as one gesture of their own.
    boolean onTouchControlsPassthrough(View view, MotionEvent event) {
        if (event.getActionMasked() == MotionEvent.ACTION_DOWN && game.overlays.isDebugOverlay() &&
                game.performanceOverlayScroll.getVisibility() == View.VISIBLE) {
            float x = event.getX() + game.gameTouchControls.getX(), y = event.getY() + game.gameTouchControls.getY();
            if (x >= game.performanceOverlayScroll.getX() && x < game.performanceOverlayScroll.getX() + game.performanceOverlayScroll.getWidth() &&
                    y >= game.performanceOverlayScroll.getY() && y < game.performanceOverlayScroll.getY() + game.performanceOverlayScroll.getHeight()) {
                // The debug overlay below scrolls on its own.
                return false;
            }
        }
        return onOverlayTouch(game.gameTouchControls, event);
    }

    @SuppressLint("ClickableViewAccessibility")
    boolean onOverlayTouch(View view, MotionEvent event) {
        MotionEvent translated = MotionEvent.obtain(event);
        translated.offsetLocation(getStreamViewOffsetX(view), getStreamViewOffsetY(view));
        try {
            return game.onTouch(game.streamView, translated);
        } finally {
            translated.recycle();
        }
    }

    @SuppressLint("ClickableViewAccessibility")
    boolean onTouch(View view, MotionEvent event) {
        noteTouchArrivals(event);
        game.keyboardInput.noteStreamPress(view, event);
        if (game.desktopGestures != null && game.desktopGestures.enabled() && game.foreground && game.connected && game.streamMenu == null) {
            MotionEvent relative = MotionEvent.obtain(event);
            relative.offsetLocation(getStreamViewOffsetX(view), getStreamViewOffsetY(view));
            boolean handled;
            try {
                handled = game.desktopGestures.onTouch(relative, game.grabbedInput &&
                        TouchMode.isRelative(touchMode()) && isInTrackpadArea(view, event));
            }
            finally { relative.recycle(); }
            if (handled) {
                if (game.trackpadPointer != null) game.trackpadPointer.syncCursor();
                return true;
            }
        }
        if (event.getAction() == MotionEvent.ACTION_DOWN) {
            // Tell the OS not to buffer input events for us
            //
            // NB: This is still needed even when we call the newer requestUnbufferedDispatch()!
            view.requestUnbufferedDispatch(event);
        }

        return handleMotionEvent(view, event);
    }

    private void noteTouchArrivals(MotionEvent event) {
        if (event.getActionMasked() != MotionEvent.ACTION_MOVE) return;
        for (int i = 0; i < event.getHistorySize(); i++) touchArrivals.sample(event.getHistoricalEventTime(i));
        touchArrivals.sample(event.getEventTime());
        String line = touchArrivals.delivered(android.os.SystemClock.uptimeMillis());
        if (line != null) {
            android.view.Display display = game.videoOutput.getVideoDisplay();
            LimeLog.info(line + ", display at " + display.getRefreshRate() + " Hz" +
                    (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ? " of " + display.getMode().getRefreshRate() + " Hz" : ""));
        }
    }

    void updateGameTouchProfile() {
        if (game.gameTouchControls == null || game.app == null || game.app.getRole() != NvApp.Role.STREAM ||
                game.app.isControlAction() || game.recoveryAppId == 0) return;
        game.gameTouchControls.setProfile(com.limelight.binding.input.TouchLayout.profileKey(
                game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), game.recoveryAppUuid, game.recoveryAppId));
    }

    void mouseMove(int deltaX, int deltaY) {
        game.conn.sendMouseMove(
                (short)mouseDeltaX.scale(deltaX, game.prefConfig.mouseSpeed / 100f),
                (short)mouseDeltaY.scale(deltaY, game.prefConfig.mouseSpeed / 100f));
    }

    void mouseButtonEvent(int buttonId, boolean down) {
        byte buttonIndex;

        switch (buttonId)
        {
        case EvdevListener.BUTTON_LEFT:
            buttonIndex = MouseButtonPacket.BUTTON_LEFT;
            break;
        case EvdevListener.BUTTON_MIDDLE:
            buttonIndex = MouseButtonPacket.BUTTON_MIDDLE;
            break;
        case EvdevListener.BUTTON_RIGHT:
            buttonIndex = MouseButtonPacket.BUTTON_RIGHT;
            break;
        case EvdevListener.BUTTON_X1:
            buttonIndex = MouseButtonPacket.BUTTON_X1;
            break;
        case EvdevListener.BUTTON_X2:
            buttonIndex = MouseButtonPacket.BUTTON_X2;
            break;
        default:
            LimeLog.warning("Unhandled button: "+buttonId);
            return;
        }

        if (down) {
            game.conn.sendMouseButtonDown(buttonIndex);
        }
        else {
            game.conn.sendMouseButtonUp(buttonIndex);
        }
    }

    void sendCustomMouse(int button, boolean down) {
        game.conn.sendOverlayMouseButton((byte)button, down);
    }

    void releaseMouseButtons() {
        if (game.connected) {
            // Evdev, stylus and synthetic Back clicks do not share the MotionEvent button mask.
            byte[] buttons = {MouseButtonPacket.BUTTON_LEFT, MouseButtonPacket.BUTTON_RIGHT, MouseButtonPacket.BUTTON_MIDDLE,
                    MouseButtonPacket.BUTTON_X1, MouseButtonPacket.BUTTON_X2};
            for (byte button : buttons) {
                game.conn.sendMouseButtonUp(button);
            }
        }
        lastButtonState = 0;
    }
}
