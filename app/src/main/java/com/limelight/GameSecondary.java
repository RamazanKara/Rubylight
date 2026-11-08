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
            new Handler(Looper.getMainLooper()).postDelayed(() -> {
                if (alive == 0) Process.killProcess(Process.myPid());
            }, 5000);
        }
    }
}
