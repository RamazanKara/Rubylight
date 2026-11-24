package com.limelight;

import android.annotation.SuppressLint;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.widget.Toast;

import com.limelight.binding.input.MouseDeltaAccumulator;
import com.limelight.binding.input.TouchArrivalWatch;
import com.limelight.binding.input.touch.FingerTapGesture;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.ui.HostCursorShapes;
import com.limelight.ui.NavigationDialogBuilder;

/** The stream's pointer input: touch modes and contexts, the trackpad, mice and pens, the desktop pointer, on-screen controls and the mic button. */
final class GameTouchInput {
    private final Game game;
    private final TouchArrivalWatch touchArrivals = new TouchArrivalWatch();

    // Undoes the desktop zoom for an event. Set again for every event while the desktop is zoomed and
    // done with before the event is handled; input arrives on the main thread only, so one serves all.
    private final android.graphics.Matrix zoomInverse = new android.graphics.Matrix();
    private final float[] zoomInverseValues = new float[9];
    private final FingerTapGesture fingerTap = new FingerTapGesture();

    private final GameMouseInput mouseInput;
    private final GamePhysicalTouchpad physicalTouchpad;
    private final GameFingerTouchHandler fingerTouch;

    private int desktopHeldButtons;
    private MouseDeltaAccumulator desktopDeltaX = new MouseDeltaAccumulator();
    private MouseDeltaAccumulator desktopDeltaY = new MouseDeltaAccumulator();
    private MouseDeltaAccumulator desktopScrollV = new MouseDeltaAccumulator();
    private MouseDeltaAccumulator desktopScrollH = new MouseDeltaAccumulator();

    GameTouchInput(Game game) {
        this.game = game;
        this.mouseInput = new GameMouseInput(game);
        this.physicalTouchpad = new GamePhysicalTouchpad(game);
        this.fingerTouch = new GameFingerTouchHandler(game, fingerTap);
    }

    // A USB or Bluetooth mouse, not a touchscreen or a virtual device.
    boolean hasExternalMouse() {
        return mouseInput.hasExternalMouse();
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
        fingerTouch.ensureTrackpadPointer(touchMode());
        updateHostCursorShapes();
        physicalTouchpad.initializeContexts();
        fingerTouch.initializeTouchContexts(usesTrackpadGestures());
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

    void clickRightMouseButton() {
        fingerTouch.clickRightMouseButton();
    }

    void cancelTouchInput() {
        if (game.desktopGestures != null) game.desktopGestures.cancelTouch();
        cancelHostTouchInput();
        physicalTouchpad.cancelNativeTouchpad();
    }

    // Desktop pan/zoom takes over the host gesture while retaining its own tap candidate.
    void cancelHostTouchInput() {
        fingerTouch.cancelHostTouchInput();
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
        fingerTouch.resetNativeTouchUnsupported();
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
            if (game.modernController != null) {
                game.modernController.finishEditing(false);
                game.modernController.hide();
            }
        }
        // The microphone button's default place follows the on-screen controller's top row.
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
        // A connected gamepad may have kept the controls off the screen; their layout is still editable.
        game.ensureOnscreenControllers();
        // The controller edits in place: drag, pinch and the panel.
        if (game.modernController != null) game.modernController.startEditing();
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

    private float getStreamViewRelativeNormalizedX(View view, MotionEvent event, int pointerIndex, int sample) {
        return GameNativeTouchSender.getStreamViewRelativeNormalizedX(game, view, event, pointerIndex, sample);
    }

    private float getStreamViewRelativeNormalizedY(View view, MotionEvent event, int pointerIndex, int sample) {
        return GameNativeTouchSender.getStreamViewRelativeNormalizedY(game, view, event, pointerIndex, sample);
    }

    float getStreamViewOffsetY(View view) {
        return GameNativeTouchSender.getStreamViewOffsetY(game, view);
    }

    private void sendNativeTouchpadFrame(MotionEvent event) {
        physicalTouchpad.sendNativeTouchpadFrame(event);
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
            translated.offsetLocation(GameNativeTouchSender.getStreamViewOffsetX(game, view),
                    GameNativeTouchSender.getStreamViewOffsetY(game, view));
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
            if (GameMouseInput.isMouseOrNonFingerEvent(event, eventSource)) {
                mouseInput.handleMouseOrStylusEvent(view, event, eventSource, physicalTouchpad);
                return true;
            }
            if (eventSource == InputDevice.SOURCE_TOUCHPAD) {
                return physicalTouchpad.handleFingerEvent(event);
            }
            return fingerTouch.handleFingerEvent(view, event, touchMode(), usesTrackpadGestures());
        }

        // Unknown class
        return false;
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
        translated.offsetLocation(GameNativeTouchSender.getStreamViewOffsetX(game, view),
                GameNativeTouchSender.getStreamViewOffsetY(game, view));
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
            relative.offsetLocation(GameNativeTouchSender.getStreamViewOffsetX(game, view),
                    GameNativeTouchSender.getStreamViewOffsetY(game, view));
            boolean handled;
            try {
                handled = game.desktopGestures.onTouch(relative, game.grabbedInput &&
                        TouchMode.isRelative(touchMode()) && fingerTouch.isInTrackpadArea(view, event));
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
                    (" of " + display.getMode().getRefreshRate() + " Hz"));
        }
    }

    void updateGameTouchProfile() {
        if (game.gameTouchControls == null || game.app == null || game.app.getRole() != NvApp.Role.STREAM ||
                game.app.isControlAction() || game.recoveryAppId == 0) return;
        game.gameTouchControls.setProfile(com.limelight.binding.input.TouchLayout.profileKey(
                game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), game.recoveryAppUuid, game.recoveryAppId));
    }

    void mouseMove(int deltaX, int deltaY) {
        mouseInput.mouseMove(deltaX, deltaY);
    }

    void mouseButtonEvent(int buttonId, boolean down) {
        mouseInput.mouseButtonEvent(buttonId, down);
    }

    void sendCustomMouse(int button, boolean down) {
        mouseInput.sendCustomMouse(button, down);
    }

    void releaseMouseButtons() {
        mouseInput.releaseMouseButtons();
    }
}
