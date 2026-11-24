package com.limelight;

import android.os.Build;
import android.view.InputDevice;
import android.view.MotionEvent;

import com.limelight.binding.input.touch.TouchContext;
import com.limelight.binding.input.touch.TouchpadSurface;
import com.limelight.binding.input.touch.TrackpadContext;
import com.limelight.computers.HostNotices;
import com.limelight.nvstream.jni.MoonBridge;

/**
 * Physical touchpad ({@link InputDevice#SOURCE_TOUCHPAD}) handling: forwards multi-finger touchpad
 * frames to the PC when supported, or translates finger gestures through {@link TrackpadContext}.
 */
final class GamePhysicalTouchpad {
    private final Game game;

    // Finger touches of a physical touchpad: tap-to-click, scroll and drag, with the Trackpad Sensitivity settings.
    private final TrackpadContext[] touchpadContextMap = new TrackpadContext[2];
    private int touchpadResolutionDevice = -2;

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

    GamePhysicalTouchpad(Game game) {
        this.game = game;
    }

    void initializeContexts() {
        for (int i = 0; i < touchpadContextMap.length; i++) {
            TrackpadContext touchpad = new TrackpadContext(game.conn, i, game.prefConfig.trackpadSwapAxis,
                    game.prefConfig.trackpadSensitivityX, game.prefConfig.trackpadSensitivityY,
                    game.prefConfig.mouseSpeed, game.prefConfig.absoluteMouseMode ? game.streamView : null);
            touchpad.setScrollSpeed(game.prefConfig.trackpadScrollSpeed);
            touchpadContextMap[i] = touchpad;
        }
        touchpadResolutionDevice = -2;
    }

    boolean handleFingerEvent(MotionEvent event) {
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
    boolean nativeTouchpad() {
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
    void cancelNativeTouchpad() {
        if (nativeTouchpadHeld) {
            nativeTouchpadHeld = false;
            game.conn.cancelTouchpad();
        }
    }

    /** The touchpad's fingers still on it after this event, and its click, as one frame for the PC. */
    void sendNativeTouchpadFrame(MotionEvent event) {
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
}
