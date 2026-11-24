package com.limelight;

import android.os.Build;
import android.util.DisplayMetrics;
import android.view.MotionEvent;
import android.view.View;

import com.limelight.binding.input.touch.AbsoluteTouchContext;
import com.limelight.binding.input.touch.FingerTapGesture;
import com.limelight.binding.input.touch.PointerAcceleration;
import com.limelight.binding.input.touch.RelativeTouchContext;
import com.limelight.binding.input.touch.TouchContext;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.binding.input.touch.TouchMomentum;
import com.limelight.binding.input.touch.TrackpadArea;
import com.limelight.binding.input.touch.TrackpadContext;
import com.limelight.binding.input.touch.TrackpadPointer;
import com.limelight.binding.input.touch.TwoFingerTapGesture;
import com.limelight.computers.HostNotices;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.nvstream.jni.MoonBridge;

/**
 * Touchscreen finger input for a running stream: touch contexts, trackpad area filtering, native
 * multi-touch fallback, and two-/three-/four-finger tap gestures.
 */
final class GameFingerTouchHandler {
    // Travel allowed for a three- or four-finger tap, in dp.
    private static final int FINGER_TAP_SLOP_DP = 40;

    private final Game game;
    private final FingerTapGesture fingerTap;
    // Mouse emulation uses two touches; native touch forwards all pointers.
    private final TouchContext[] touchContextMap = new TouchContext[2];
    private final TwoFingerTapGesture twoFingerTap = new TwoFingerTapGesture();

    private TrackpadArea trackpadArea;

    // The current finger gesture started outside the trackpad area or while touch is off.
    private boolean touchGestureIgnored;
    private boolean middleButtonUpPending;
    private final Runnable middleButtonUp = () -> {
        middleButtonUpPending = false;
        GameFingerTouchHandler.this.game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_MIDDLE);
    };
    private boolean rightButtonUpPending;
    private final Runnable rightButtonUp = () -> {
        rightButtonUpPending = false;
        GameFingerTouchHandler.this.game.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
    };

    private boolean nativeTouchUnsupported;
    private boolean nativeTouchGestureActive;

    GameFingerTouchHandler(Game game, FingerTapGesture fingerTap) {
        this.game = game;
        this.fingerTap = fingerTap;
    }

    void ensureTrackpadPointer(int mode) {
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
        game.trackpadPointer.setLocalCursor(mode == TouchMode.LOCAL_CURSOR);
    }

    void initializeTouchContexts(boolean usesTrackpadGestures) {
        for (int i = 0; i < touchContextMap.length; i++) {
            if (usesTrackpadGestures) {
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

    boolean isInTrackpadArea(View view, MotionEvent event) {
        if (trackpadArea == null || trackpadArea.isFull()) return true;
        View parent = (View) game.streamView.getParent();
        if (parent == null) return true;
        float x = event.getX(0) + (view != null ? view.getX() : 0);
        float y = event.getY(0) + (view != null ? view.getY() : 0);
        return trackpadArea.contains(x, y, parent.getWidth(), parent.getHeight());
    }

    void resetNativeTouchUnsupported() {
        nativeTouchUnsupported = false;
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
            // Some devices (seen on ColorOS) cancel the touches ~40 ms after the third finger lands, even
            // with their own three-finger gestures off, and then never deliver the lifts. A cancel that
            // arrives inside the tap window, with the fingers still, is the tap itself. From Android 13 only
            // a cancel the system flags as its own counts; older versions have no flag to check.
            boolean systemCancel = Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU
                    || (event.getFlags() & MotionEvent.FLAG_CANCELED) != 0;
            if (game.prefConfig.fingerTapOnCancel && systemCancel) {
                int fingers = fingerTap.finish(event.getEventTime(),
                        FINGER_TAP_SLOP_DP * game.getResources().getDisplayMetrics().density);
                if (fingers == 3) onFingerTap(game.prefConfig.threeFingerTap);
                else if (fingers == 4) onFingerTap(game.prefConfig.fourFingerTap);
            } else {
                fingerTap.reset();
            }
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

    private TouchContext getTouchContext(int actionIndex) {
        if (actionIndex < touchContextMap.length) {
            return touchContextMap[actionIndex];
        }
        else {
            return null;
        }
    }

    boolean handleFingerEvent(View view, MotionEvent event, int mode, boolean usesTrackpadGestures) {
        if (game.modernController != null && game.modernController.isEditing()) {
            // Ignore presses while the on-screen controller's layout is being edited
            return true;
        }

        if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
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
            if (GameNativeTouchSender.trySendTouchEvent(game, view, event)) {
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
        boolean rightTap = game.prefConfig.touchscreenTrackpad && !usesTrackpadGestures &&
                twoFingerTap.onTouch(event, game.getResources().getDisplayMetrics().density);
        if (handleFingerTapGesture(event)) return true;

        // If this is the parent view, we'll offset our coordinates to appear as if they
        // are relative to the StreamView like our StreamView touch events are.
        float xOffset, yOffset;
        if (view != game.streamView && !game.prefConfig.touchscreenTrackpad) {
            xOffset = GameNativeTouchSender.getStreamViewOffsetX(game, view);
            yOffset = GameNativeTouchSender.getStreamViewOffsetY(game, view);
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
        return true;
    }
}
