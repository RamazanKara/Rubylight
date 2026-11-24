package com.limelight;

import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;

import com.limelight.nvstream.jni.MoonBridge;

/**
 * Encodes and sends native multi-touch and stylus/pen MotionEvents normalized relative to the
 * stream view, without allocating per-pointer coordinate or contact-area arrays.
 */
final class GameNativeTouchSender {
    // The sample of an event the methods below read: NEWEST_SAMPLE, or an index of its batched
    // history (oldest first), which a touchscreen fills when it samples faster than the display.
    static final int NEWEST_SAMPLE = -1;

    private GameNativeTouchSender() {
    }

    static byte getLiTouchTypeFromEvent(MotionEvent event) {
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
    static float getStreamViewRelativeNormalizedX(Game game, View view, MotionEvent event, int pointerIndex, int sample) {
        float normalizedX = (sample < 0 ? event.getX(pointerIndex) : event.getHistoricalX(pointerIndex, sample)) +
                getStreamViewOffsetX(game, view);
        normalizedX = Math.max(normalizedX, 0.0f);
        normalizedX = Math.min(normalizedX, game.streamView.getWidth());
        return normalizedX / game.streamView.getWidth();
    }

    static float getStreamViewRelativeNormalizedY(Game game, View view, MotionEvent event, int pointerIndex, int sample) {
        float normalizedY = (sample < 0 ? event.getY(pointerIndex) : event.getHistoricalY(pointerIndex, sample)) +
                getStreamViewOffsetY(game, view);
        normalizedY = Math.max(normalizedY, 0.0f);
        normalizedY = Math.min(normalizedY, game.streamView.getHeight());
        return normalizedY / game.streamView.getHeight();
    }

    static float getStreamViewOffsetX(Game game, View view) {
        // Sibling views share the pane padding, including fold and display-cutout insets.
        return (view != null ? view.getX() : 0) - game.streamView.getX();
    }

    static float getStreamViewOffsetY(Game game, View view) {
        return (view != null ? view.getY() : 0) - game.streamView.getY();
    }

    private static float normalizeValueInRange(float value, InputDevice.MotionRange range) {
        return (value - range.getMin()) / range.getRange();
    }

    private static float getPressureOrDistance(MotionEvent event, int pointerIndex, int sample) {
        InputDevice dev = event.getDevice();
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_HOVER_ENTER:
            case MotionEvent.ACTION_HOVER_MOVE:
            case MotionEvent.ACTION_HOVER_EXIT:
                // Hover events report distance
                if (dev != null) {
                    InputDevice.MotionRange distanceRange = dev.getMotionRange(MotionEvent.AXIS_DISTANCE, event.getSource());
                    if (distanceRange != null) {
                        return normalizeValueInRange(sample < 0 ? event.getAxisValue(MotionEvent.AXIS_DISTANCE, pointerIndex) :
                                event.getHistoricalAxisValue(MotionEvent.AXIS_DISTANCE, pointerIndex, sample), distanceRange);
                    }
                }
                return 0.0f;

            default:
                // Other events report pressure
                return sample < 0 ? event.getPressure(pointerIndex) : event.getHistoricalPressure(pointerIndex, sample);
        }
    }

    private static short getRotationDegrees(MotionEvent event, int pointerIndex, int sample) {
        InputDevice dev = event.getDevice();
        if (dev != null) {
            if (dev.getMotionRange(MotionEvent.AXIS_ORIENTATION, event.getSource()) != null) {
                short rotationDegrees = (short) Math.toDegrees(sample < 0 ? event.getOrientation(pointerIndex) :
                        event.getHistoricalOrientation(pointerIndex, sample));
                if (rotationDegrees < 0) {
                    rotationDegrees += 360;
                }
                return rotationDegrees;
            }
        }
        return MoonBridge.LI_ROT_UNKNOWN;
    }

    private static float getContactAreaOrientation(MotionEvent event, int pointerIndex, int sample) {
        // If the orientation is unknown, we'll just assume it's at a 45 degree angle and scale it by
        // X and Y scaling factors evenly.
        if (event.getDevice() == null || event.getDevice().getMotionRange(MotionEvent.AXIS_ORIENTATION, event.getSource()) == null) {
            return (float)(Math.PI / 4);
        }
        else {
            return sample < 0 ? event.getOrientation(pointerIndex) : event.getHistoricalOrientation(pointerIndex, sample);
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
    private static float getStreamViewNormalizedContactAxis(Game game, float r, float theta) {
        // Convert polar to cartesian coordinates, and normalize each to the stream view size
        float x = Math.min(Math.abs((float)(r * Math.cos(theta))), game.streamView.getWidth()) / game.streamView.getWidth();
        float y = Math.min(Math.abs((float)(r * Math.sin(theta))), game.streamView.getHeight()) / game.streamView.getHeight();

        // Convert the normalized values back into polar coordinates
        return (float)Math.sqrt(Math.pow(x, 2) + Math.pow(y, 2));
    }

    private static float getStreamViewNormalizedContactAreaMajor(Game game, MotionEvent event, int pointerIndex, int sample, float orientation) {
        // The contact area major axis is parallel to the orientation, so we simply convert
        // polar to cartesian coordinates using the orientation as theta.
        float major = sample < 0 ?
                (reportsToolSize(event) ? event.getToolMajor(pointerIndex) : event.getTouchMajor(pointerIndex)) :
                (reportsToolSize(event) ? event.getHistoricalToolMajor(pointerIndex, sample) : event.getHistoricalTouchMajor(pointerIndex, sample));
        return getStreamViewNormalizedContactAxis(game, major, orientation);
    }

    private static float getStreamViewNormalizedContactAreaMinor(Game game, MotionEvent event, int pointerIndex, int sample, float orientation) {
        // The contact area minor axis is perpendicular to the contact area major axis (and thus
        // the orientation), so rotate the orientation angle by 90 degrees.
        float minor = sample < 0 ?
                (reportsToolSize(event) ? event.getToolMinor(pointerIndex) : event.getTouchMinor(pointerIndex)) :
                (reportsToolSize(event) ? event.getHistoricalToolMinor(pointerIndex, sample) : event.getHistoricalTouchMinor(pointerIndex, sample));
        return getStreamViewNormalizedContactAxis(game, minor, (float)(orientation + (Math.PI / 2)));
    }

    private static boolean sendPenEventForPointer(Game game, View view, MotionEvent event, byte eventType, byte toolType, int pointerIndex, int sample) {
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
                tiltDegrees = (byte)Math.toDegrees(sample < 0 ? event.getAxisValue(MotionEvent.AXIS_TILT, pointerIndex) :
                        event.getHistoricalAxisValue(MotionEvent.AXIS_TILT, pointerIndex, sample));
            }
        }

        float orientation = getContactAreaOrientation(event, pointerIndex, sample);
        return game.conn.sendPenEvent(eventType, toolType, penButtons,
                getStreamViewRelativeNormalizedX(game, view, event, pointerIndex, sample),
                getStreamViewRelativeNormalizedY(game, view, event, pointerIndex, sample),
                getPressureOrDistance(event, pointerIndex, sample),
                getStreamViewNormalizedContactAreaMajor(game, event, pointerIndex, sample, orientation),
                getStreamViewNormalizedContactAreaMinor(game, event, pointerIndex, sample, orientation),
                getRotationDegrees(event, pointerIndex, sample), tiltDegrees) != MoonBridge.LI_ERR_UNSUPPORTED;
    }

    /**
     * The samples a finger move carries: its batched history (a touchscreen sampling faster than the
     * display draws), oldest first, and its own newest last. Pen moves and hovers send only their
     * newest: both engines merge queued pen moves and hovers into the newest anyway
     * (moonlight-common-c's input send thread, nightjar-core's InputQueue), while touch packets
     * each reach the host.
     */
    private static int sampleCount(MotionEvent event) {
        return event.getActionMasked() == MotionEvent.ACTION_MOVE ? event.getHistorySize() + 1 : 1;
    }

    /** The sample index of the i-th of sampleCount: a history index, or NEWEST_SAMPLE for the last. */
    private static int sampleAt(MotionEvent event, int i) {
        return i < sampleCount(event) - 1 ? i : NEWEST_SAMPLE;
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

    static boolean trySendPenEvent(Game game, View view, MotionEvent event) {
        byte eventType = getLiTouchTypeFromEvent(event);
        if (eventType < 0) {
            return false;
        }

        if (event.getActionMasked() == MotionEvent.ACTION_MOVE) {
            // Move events may impact all active pointers; only the newest sample (see sampleCount)
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

                if (!sendPenEventForPointer(game, view, event, eventType, toolType, i, NEWEST_SAMPLE)) {
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
            return sendPenEventForPointer(game, view, event, eventType, toolType, event.getActionIndex(), NEWEST_SAMPLE);
        }
    }

    private static boolean sendTouchEventForPointer(Game game, View view, MotionEvent event, byte eventType, int pointerIndex, int sample) {
        float orientation = getContactAreaOrientation(event, pointerIndex, sample);
        return game.conn.sendTouchEvent(eventType, event.getPointerId(pointerIndex),
                getStreamViewRelativeNormalizedX(game, view, event, pointerIndex, sample),
                getStreamViewRelativeNormalizedY(game, view, event, pointerIndex, sample),
                getPressureOrDistance(event, pointerIndex, sample),
                getStreamViewNormalizedContactAreaMajor(game, event, pointerIndex, sample, orientation),
                getStreamViewNormalizedContactAreaMinor(game, event, pointerIndex, sample, orientation),
                getRotationDegrees(event, pointerIndex, sample)) != MoonBridge.LI_ERR_UNSUPPORTED;
    }

    static boolean trySendTouchEvent(Game game, View view, MotionEvent event) {
        byte eventType = getLiTouchTypeFromEvent(event);
        if (eventType < 0) {
            return false;
        }

        if (event.getActionMasked() == MotionEvent.ACTION_MOVE) {
            // Move events may impact all active pointers, each batched sample in order
            for (int s = 0, samples = sampleCount(event); s < samples; s++) {
                for (int i = 0; i < event.getPointerCount(); i++) {
                    if (!sendTouchEventForPointer(game, view, event, eventType, i, sampleAt(event, s))) {
                        return false;
                    }
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
            return sendTouchEventForPointer(game, view, event, eventType, event.getActionIndex(), NEWEST_SAMPLE);
        }
    }
}
