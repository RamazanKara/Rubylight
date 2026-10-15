package com.limelight.demo;

import android.content.Context;
import android.util.Log;
import android.view.MotionEvent;
import android.view.View;

import com.limelight.binding.input.touch.PointerAcceleration;
import com.limelight.binding.input.touch.RelativeTouchContext;
import com.limelight.binding.input.touch.TouchContext;
import com.limelight.binding.input.touch.TouchMomentum;
import com.limelight.binding.input.touch.TrackpadPointer;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.LocalCursorView;
import com.limelight.ui.StreamView;

/**
 * Drives the production trackpad and Local cursor code from the demo stream, with "--es touch
 * trackpad" or "--es touch local_cursor". What would go to a host is written to logcat (tag
 * NightjarDemoInput) so emulator runs can check it.
 */
final class DemoTouchpad implements View.OnTouchListener {
    private static final String TAG = "NightjarDemoInput";
    private static final int WIDTH = 1920, HEIGHT = 1080;

    private final TouchContext[] contexts = new TouchContext[2];
    private final TrackpadPointer pointer;

    DemoTouchpad(Context context, StreamView stream, LocalCursorView cursor, String mode) {
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(context);
        NvConnection connection = new LoggingConnection();
        pointer = new TrackpadPointer(connection, stream, cursor, false,
                config.trackpadAcceleration ? PointerAcceleration.forCurve(config.trackpadAccelerationCurve) : null,
                config.trackpadMomentum ? new TouchMomentum(config.trackpadMomentumGlide) : null);
        pointer.setReferenceSize(WIDTH, HEIGHT);
        pointer.setLocalCursor("local_cursor".equals(mode));
        for (int i = 0; i < contexts.length; i++) {
            contexts[i] = new RelativeTouchContext(connection, i, WIDTH, HEIGHT, stream, config, pointer);
        }
    }

    void stop() {
        pointer.stopMomentum();
        for (TouchContext context : contexts) context.cancelTouch();
    }

    @Override
    public boolean onTouch(View view, MotionEvent event) {
        int index = event.getActionIndex();
        int count = event.getPointerCount();
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN:
                for (TouchContext context : contexts) context.setPointerCount(count);
                if (index < contexts.length) {
                    contexts[index].touchDownEvent((int) event.getX(index), (int) event.getY(index), event.getEventTime(), true);
                }
                break;
            case MotionEvent.ACTION_MOVE:
                for (TouchContext context : contexts) {
                    int i = context.getActionIndex();
                    if (i < count) context.touchMoveEvent((int) event.getX(i), (int) event.getY(i), event.getEventTime());
                }
                break;
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP:
                if (index < contexts.length) {
                    contexts[index].touchUpEvent((int) event.getX(index), (int) event.getY(index), event.getEventTime());
                }
                for (TouchContext context : contexts) context.setPointerCount(count - 1);
                break;
            case MotionEvent.ACTION_CANCEL:
                stop();
                break;
            default:
                return false;
        }
        pointer.syncCursor();
        return true;
    }

    private static final class LoggingConnection extends NvConnection {
        LoggingConnection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "demo",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return true; }
        @Override public void sendMouseMove(short x, short y) { Log.i(TAG, "move " + x + " " + y); }
        @Override public void sendMousePosition(short x, short y, short width, short height) {
            Log.i(TAG, "position " + x + " " + y + " of " + width + "x" + height);
        }
        @Override public void sendMouseButtonDown(byte button) { Log.i(TAG, "button down " + button); }
        @Override public void sendMouseButtonUp(byte button) { Log.i(TAG, "button up " + button); }
        @Override public void sendMouseHighResScroll(short amount) { Log.i(TAG, "scroll " + amount); }
    }
}
