package com.limelight;

import com.limelight.binding.input.MouseDeltaAccumulator;
import com.limelight.binding.input.touch.AbsoluteTouchContext;
import com.limelight.binding.input.touch.PointerAcceleration;
import com.limelight.binding.input.touch.RelativeTouchContext;
import com.limelight.binding.input.touch.FingerTapGesture;
import com.limelight.binding.input.touch.TwoFingerTapGesture;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.binding.input.touch.TouchMomentum;
import com.limelight.binding.input.touch.TrackpadArea;
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
import android.content.SharedPreferences;
import android.os.Build;
import android.preference.PreferenceManager;
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

    private int lastButtonState = 0;
    private final MouseDeltaAccumulator mouseDeltaX = new MouseDeltaAccumulator();
    private final MouseDeltaAccumulator mouseDeltaY = new MouseDeltaAccumulator();

    // Mouse emulation uses two touches; native touch forwards all pointers.
    private final TouchContext[] touchContextMap = new TouchContext[2];
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
        for (int i = 0; i < touchContextMap.length; i++) {
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
        return game.prefConfig.touchMode == TouchMode.LOCAL_CURSOR || game.prefConfig.touchMode == TouchMode.OFF ?
                game.prefConfig.touchMode : TouchMode.TRACKPAD;
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

    void showTouchModeDialog() {
        if (game.externalPresentation != null) {
            game.messages.alert(R.string.desktop_touchpad_help, Toast.LENGTH_LONG);
            return;
        }
        String[] modes = game.getResources().getStringArray(R.array.stream_touch_modes);
        game.showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.stream_touch_mode)
                .setSingleChoiceItems(modes, touchMode(), (dialog, which) -> {
                    cancelTouchInput();
                    game.prefConfig.touchMode = which;
                    game.prefConfig.touchscreenTrackpad = TouchMode.legacyTrackpad(which);
                    game.nativeTouchEnabled = which == TouchMode.MULTI_TOUCH;
                    nativeTouchUnsupported = false;
                    initializeTouchContexts();
                    SharedPreferences.Editor editor = PreferenceManager.getDefaultSharedPreferences(game).edit();
                    TouchMode.save(editor, which);
                    editor.apply();
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
        PreferenceManager.getDefaultSharedPreferences(game).edit()
                .putBoolean("checkbox_show_onscreen_controls", enabled).apply();
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
        PreferenceManager.getDefaultSharedPreferences(game).edit()
                .putBoolean(com.limelight.preferences.PreferenceConfiguration.MIC_BUTTON_PREF_STRING, shown).apply();
        updateMicButton();
        game.performanceOverlayScroll.requestLayout();
    }

    void showControllerLayoutDialog() {
        if (game.modernController != null) {
            // The modern controller edits in place: drag, pinch and the panel instead of modes.
            game.modernController.startEditing();
            return;
        }
        game.showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.stream_controls_layout)
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
        game.setMetaKeyCaptureState(grab && game.prefConfig.captureSystemKeys);

        game.grabbedInput = grab;
        if (game.controllerHandler != null) {
            game.controllerHandler.setInputEnabled(grab && game.hasWindowFocus());
        }
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

    private float[] getStreamViewRelativeNormalizedXY(View view, MotionEvent event, int pointerIndex) {
        float normalizedX = event.getX(pointerIndex) + getStreamViewOffsetX(view);
        float normalizedY = event.getY(pointerIndex) + getStreamViewOffsetY(view);

        normalizedX = Math.max(normalizedX, 0.0f);
        normalizedY = Math.max(normalizedY, 0.0f);

        normalizedX = Math.min(normalizedX, game.streamView.getWidth());
        normalizedY = Math.min(normalizedY, game.streamView.getHeight());

        normalizedX /= game.streamView.getWidth();
        normalizedY /= game.streamView.getHeight();

        return new float[] { normalizedX, normalizedY };
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

    private static float[] polarToCartesian(float r, float theta) {
        return new float[] { (float)(r * Math.cos(theta)), (float)(r * Math.sin(theta)) };
    }

    private static float cartesianToR(float[] point) {
        return (float)Math.sqrt(Math.pow(point[0], 2) + Math.pow(point[1], 2));
    }

    private float[] getStreamViewNormalizedContactArea(MotionEvent event, int pointerIndex) {
        float orientation;

        // If the orientation is unknown, we'll just assume it's at a 45 degree angle and scale it by
        // X and Y scaling factors evenly.
        if (event.getDevice() == null || event.getDevice().getMotionRange(MotionEvent.AXIS_ORIENTATION, event.getSource()) == null) {
            orientation = (float)(Math.PI / 4);
        }
        else {
            orientation = event.getOrientation(pointerIndex);
        }

        float contactAreaMajor, contactAreaMinor;
        switch (event.getActionMasked()) {
            // Hover events report the tool size
            case MotionEvent.ACTION_HOVER_ENTER:
            case MotionEvent.ACTION_HOVER_MOVE:
            case MotionEvent.ACTION_HOVER_EXIT:
                contactAreaMajor = event.getToolMajor(pointerIndex);
                contactAreaMinor = event.getToolMinor(pointerIndex);
                break;

            // Other events report contact area
            default:
                contactAreaMajor = event.getTouchMajor(pointerIndex);
                contactAreaMinor = event.getTouchMinor(pointerIndex);
                break;
        }

        // The contact area major axis is parallel to the orientation, so we simply convert
        // polar to cartesian coordinates using the orientation as theta.
        float[] contactAreaMajorCartesian = polarToCartesian(contactAreaMajor, orientation);

        // The contact area minor axis is perpendicular to the contact area major axis (and thus
        // the orientation), so rotate the orientation angle by 90 degrees.
        float[] contactAreaMinorCartesian = polarToCartesian(contactAreaMinor, (float)(orientation + (Math.PI / 2)));

        // Normalize the contact area to the stream view size
        contactAreaMajorCartesian[0] = Math.min(Math.abs(contactAreaMajorCartesian[0]), game.streamView.getWidth()) / game.streamView.getWidth();
        contactAreaMinorCartesian[0] = Math.min(Math.abs(contactAreaMinorCartesian[0]), game.streamView.getWidth()) / game.streamView.getWidth();
        contactAreaMajorCartesian[1] = Math.min(Math.abs(contactAreaMajorCartesian[1]), game.streamView.getHeight()) / game.streamView.getHeight();
        contactAreaMinorCartesian[1] = Math.min(Math.abs(contactAreaMinorCartesian[1]), game.streamView.getHeight()) / game.streamView.getHeight();

        // Convert the normalized values back into polar coordinates
        return new float[] { cartesianToR(contactAreaMajorCartesian), cartesianToR(contactAreaMinorCartesian) };
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

        float[] normalizedCoords = getStreamViewRelativeNormalizedXY(view, event, pointerIndex);
        float[] normalizedContactArea = getStreamViewNormalizedContactArea(event, pointerIndex);
        return game.conn.sendPenEvent(eventType, toolType, penButtons,
                normalizedCoords[0], normalizedCoords[1],
                getPressureOrDistance(event, pointerIndex),
                normalizedContactArea[0], normalizedContactArea[1],
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
        float[] normalizedCoords = getStreamViewRelativeNormalizedXY(view, event, pointerIndex);
        float[] normalizedContactArea = getStreamViewNormalizedContactArea(event, pointerIndex);
        return game.conn.sendTouchEvent(eventType, event.getPointerId(pointerIndex),
                normalizedCoords[0], normalizedCoords[1],
                getPressureOrDistance(event, pointerIndex),
                normalizedContactArea[0], normalizedContactArea[1],
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
            android.graphics.Matrix inverse = new android.graphics.Matrix();
            inverse.setValues(new float[] {1 / viewport.scale(), 0, -viewport.x() / viewport.scale(),
                    0, 1 / viewport.scale(), -viewport.y() / viewport.scale(), 0, 0, 1});
            translated.transform(inverse);
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
            // This case is for mice and non-finger touch devices
            if (eventSource == InputDevice.SOURCE_MOUSE ||
                    (eventSource & InputDevice.SOURCE_CLASS_POSITION) != 0 || // SOURCE_TOUCHPAD
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

                boolean rightTap = game.prefConfig.touchscreenTrackpad &&
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
