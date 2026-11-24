package com.limelight;

import android.os.Build;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;

import com.limelight.binding.input.MouseDeltaAccumulator;
import com.limelight.binding.input.evdev.EvdevListener;
import com.limelight.nvstream.input.MouseButtonPacket;

/**
 * Handles mice, DeX desktop mice, stylus/eraser pointer fallback, and evdev mouse events for a
 * running stream.
 */
final class GameMouseInput {
    private static final int STYLUS_DOWN_DEAD_ZONE_DELAY = 100;
    private static final int STYLUS_DOWN_DEAD_ZONE_RADIUS = 20;

    private static final int STYLUS_UP_DEAD_ZONE_DELAY = 150;
    private static final int STYLUS_UP_DEAD_ZONE_RADIUS = 50;

    private final Game game;
    private int lastButtonState = 0;
    private final MouseDeltaAccumulator mouseDeltaX = new MouseDeltaAccumulator();
    private final MouseDeltaAccumulator mouseDeltaY = new MouseDeltaAccumulator();

    private long lastAbsTouchUpTime = 0;
    private long lastAbsTouchDownTime = 0;
    private float lastAbsTouchUpX, lastAbsTouchUpY;
    private float lastAbsTouchDownX, lastAbsTouchDownY;

    GameMouseInput(Game game) {
        this.game = game;
    }

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

    static boolean isMouseOrNonFingerEvent(MotionEvent event, int eventSource) {
        // A touchpad's finger touches carry no action button; only its clicks do. Clicks are handled
        // below like a mouse, and the touches go to the trackpad gesture handling.
        boolean hasActionButton = event.getActionButton() != 0;

        return eventSource == InputDevice.SOURCE_MOUSE ||
                ((eventSource & InputDevice.SOURCE_CLASS_POSITION) != 0 && hasActionButton) || // SOURCE_TOUCHPAD
                eventSource == InputDevice.SOURCE_MOUSE_RELATIVE ||
                (event.getPointerCount() >= 1 &&
                        (event.getToolType(0) == MotionEvent.TOOL_TYPE_MOUSE ||
                                event.getToolType(0) == MotionEvent.TOOL_TYPE_STYLUS ||
                                event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER)) ||
                eventSource == 12290; // 12290 = Samsung DeX mode desktop mouse
    }

    void handleMouseOrStylusEvent(View view, MotionEvent event, int eventSource, GamePhysicalTouchpad touchpad) {
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
            return;
        }

        // The touchpad's click rides on its frames, with the fingers on it.
        if (eventSource == InputDevice.SOURCE_TOUCHPAD && touchpad.nativeTouchpad()) {
            touchpad.sendNativeTouchpadFrame(event);
            return;
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
        else if (view != null && GameNativeTouchSender.trySendPenEvent(game, view, event)) {
            // If our host supports pen events, send it directly
            return;
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
                    lastAbsTouchDownX = event.getX(0) + GameNativeTouchSender.getStreamViewOffsetX(game, view);
                    lastAbsTouchDownY = event.getY(0) + GameNativeTouchSender.getStreamViewOffsetY(game, view);

                    // Stylus is left click
                    game.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_LEFT);
                } else if (event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER) {
                    lastAbsTouchDownTime = event.getEventTime();
                    lastAbsTouchDownX = event.getX(0) + GameNativeTouchSender.getStreamViewOffsetX(game, view);
                    lastAbsTouchDownY = event.getY(0) + GameNativeTouchSender.getStreamViewOffsetY(game, view);

                    // Eraser is right click
                    game.conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_RIGHT);
                }
            }
            else if (event.getActionMasked() == MotionEvent.ACTION_UP || event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
                if (event.getToolType(0) == MotionEvent.TOOL_TYPE_STYLUS) {
                    lastAbsTouchUpTime = event.getEventTime();
                    lastAbsTouchUpX = event.getX(0) + GameNativeTouchSender.getStreamViewOffsetX(game, view);
                    lastAbsTouchUpY = event.getY(0) + GameNativeTouchSender.getStreamViewOffsetY(game, view);

                    // Stylus is left click
                    game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_LEFT);
                } else if (event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER) {
                    lastAbsTouchUpTime = event.getEventTime();
                    lastAbsTouchUpX = event.getX(0) + GameNativeTouchSender.getStreamViewOffsetX(game, view);
                    lastAbsTouchUpY = event.getY(0) + GameNativeTouchSender.getStreamViewOffsetY(game, view);

                    // Eraser is right click
                    game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
                }
            }
        }

        lastButtonState = buttonState;
    }

    private void updateMousePosition(View touchedView, MotionEvent event) {
        float eventX = event.getX(0) + GameNativeTouchSender.getStreamViewOffsetX(game, touchedView);
        float eventY = event.getY(0) + GameNativeTouchSender.getStreamViewOffsetY(game, touchedView);

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
