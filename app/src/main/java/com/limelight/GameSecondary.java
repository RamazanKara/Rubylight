package com.limelight;

import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.Process;

import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.jni.NativeApiBridge;

/**
 * The stream on the second screen in dual-screen mode (see {@link DualScreen}). It is the same
 * activity as {@link Game}, declared in the manifest with a process and a task of its own: the
 * streaming engines keep one stream's state per process, so two streams need two processes.
 */
public class GameSecondary extends Game {
    /** Streams alive in this process; only the main thread touches it. */
    private static int alive;
    /** How long the process stays after its last stream, for a stream started again right away. */
    static final long PROCESS_LINGER_MS = 5000;
    private static final long PROCESS_CHECK_MS = 1000;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        alive++;
        // Everything this process asks the host is the second stream's, so the host keeps it apart from
        // the first: its launch, resume, bitrate, metrics, cancel and native API requests alike.
        NvHTTP.setSecondScreen(true);
        if (NativeApiBridge.isAvailable()) {
            NativeApiBridge.setSecondScreen(true);
        }
        super.onCreate(savedInstanceState);
        // Follows the first stream's end and its touch mode.
        DualScreen.link(this);
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        alive--;
        if (isFinishing()) {
            // The settings this process read at its start are old by the next stream, and a settings
            // change in the main process never reaches it, so it goes once nothing is using it.
            endProcessLater(new Handler(Looper.getMainLooper()), connectionStopThread,
                    android.os.SystemClock.uptimeMillis());
        }
    }

    private static void endProcessLater(Handler handler, Thread stopping, long destroyedAt) {
        handler.postDelayed(() -> {
            long since = android.os.SystemClock.uptimeMillis() - destroyedAt;
            switch (processEnd(alive, stopping != null && stopping.isAlive(), since)) {
                case END:
                    Process.killProcess(Process.myPid());
                    break;
                case WAIT:
                    endProcessLater(handler, stopping, destroyedAt);
                    break;
                case STAY:
                    break;
            }
        }, PROCESS_CHECK_MS);
    }

    enum ProcessEnd { END, WAIT, STAY }

    /**
     * What the process does {@code sinceDestroyMs} after its last stream's activity went: it stays for a
     * stream started again, and it waits for that stream's stop to tell the host before it ends. A stop
     * stuck past the time a stuck stop is reported (a decoder hung in MediaCodec) ends with it.
     */
    static ProcessEnd processEnd(int alive, boolean stopRunning, long sinceDestroyMs) {
        if (alive > 0) return ProcessEnd.STAY;
        if (sinceDestroyMs < PROCESS_LINGER_MS) return ProcessEnd.WAIT;
        if (stopRunning && sinceDestroyMs < GameConnection.STOP_REPORT_MS) return ProcessEnd.WAIT;
        return ProcessEnd.END;
    }
}
