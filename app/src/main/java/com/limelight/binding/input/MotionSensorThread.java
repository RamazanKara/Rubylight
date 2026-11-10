package com.limelight.binding.input;

import android.hardware.Sensor;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Process;

/**
 * The thread a stream's motion sensor events arrive on. A listener registered without a handler is
 * called on the main thread, and a gyroscope and an accelerometer at the rate a host asks for make
 * several hundred calls a second there, between the touch, key and frame callbacks of the stream.
 * One thread serves every sensor of a stream, so the events of a sensor keep their order. It starts
 * with the first listener, since most streams never use a motion sensor, and ends with the stream.
 */
final class MotionSensorThread {
    static final String NAME = "Controller motion";

    private HandlerThread thread;
    private Handler handler;
    private boolean quit;

    /**
     * Registers the listener at the sampling period, to be called on this thread. False when the
     * sensor took no listener, or when the stream has ended and nothing may be registered anymore.
     */
    synchronized boolean register(SensorManager sensors, Sensor sensor, SensorEventListener listener,
                                  int samplingPeriodUs) {
        if (quit) {
            return false;
        }
        if (thread == null) {
            // Motion is input: it runs ahead of background work, as it did on the main thread.
            thread = new HandlerThread(NAME, Process.THREAD_PRIORITY_DISPLAY);
            thread.start();
            handler = new Handler(thread.getLooper());
        }
        return sensors.registerListener(listener, sensor, samplingPeriodUs, handler);
    }

    /** Ends the thread; events it had not delivered yet are dropped. */
    synchronized void quit() {
        quit = true;
        if (thread != null) {
            thread.quit();
            thread = null;
            handler = null;
        }
    }

    /** The handler listeners are called on, or null while no listener was registered. */
    synchronized Handler handler() {
        return handler;
    }
}
