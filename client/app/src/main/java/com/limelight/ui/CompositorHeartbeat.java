package com.limelight.ui;

import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.PixelFormat;
import android.graphics.PorterDuff;
import android.graphics.drawable.ColorDrawable;
import android.os.Build;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.view.Gravity;
import android.view.Surface;
import android.view.SurfaceControl;
import android.view.SurfaceView;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;

import androidx.annotation.RequiresApi;

import com.limelight.LimeLog;

/**
 * Keeps the display compositor actively compositing an overlay layer alongside the video surface.
 *
 * <ul>
 *   <li>For streams on Android 10+ (API 29+) with a valid {@link SurfaceView} {@link SurfaceControl},
 *   {@link #forStream(ViewGroup, SurfaceView, long)} attaches a {@code 1x1} transparent child
 *   {@link SurfaceControl} and posts frames on a dedicated {@link HandlerThread} (zero visible dot,
 *   zero UI-thread invalidation, zero HWUI {@code RenderThread} work).</li>
 *   <li>On older Android, {@code TextureView}, runtime {@code SurfaceControl} failure, or refresh-drop
 *   recovery ({@link #CompositorHeartbeat(ViewGroup)}, which votes {@code View.setRequestedFrameRate}
 *   on API 35+), it falls back to a tiny {@link View} in the video window.</li>
 * </ul>
 *
 * Ported from the derflacco fork (branch Artemide_Experimental_Async, GPL-3.0),
 * app/src/main/java/com/limelight/ui/CompositorHeartbeat.java, commit 784b4eca. Changed for
 * Nightjar: Added 1x1 transparent child {@link SurfaceControl} mode on API 29+ {@link SurfaceView}
 * streams and imperceptible 1x1 near-zero-alpha fallback {@link View} alongside the 100 ms dot
 * created and removed dynamically so it can sit in an external display's window too.
 */
public final class CompositorHeartbeat {
    static final long INTERVAL_MS = 100L;
    public static final long STREAM_INTERVAL_MS = 1000L;
    static final int COLOR_PHASE_ONE = 0xFF000000;
    static final int COLOR_PHASE_TWO = 0xFF010101;
    // Near-zero premultiplied-safe alpha so fallback View ticks are imperceptible on both black and white.
    static final int FALLBACK_ALPHA_ONE = 0x02010101;
    static final int FALLBACK_ALPHA_TWO = 0x03010101;

    private enum Mode {
        VIEW_BLACK,
        VIEW_ALPHA,
        SURFACE_CONTROL_CLEAR
    }

    private final ViewGroup holder;
    private final SurfaceView targetSurfaceView;
    private Mode mode;
    private boolean surfaceControlFailed;
    private final long intervalMs;

    private View dot;
    private final ColorDrawable color;
    private final int phaseOneColor;
    private final int phaseTwoColor;
    private final Handler uiHandler = new Handler(Looper.getMainLooper());
    private Runnable removeDot;

    private HandlerThread scThread;
    private volatile Handler scHandler;
    private SurfaceControl scLayer;
    private volatile Surface scSurface;

    private volatile boolean running;
    private volatile boolean stopped;
    private volatile boolean surfaceControlFramePosted;
    private boolean alternatePhase;

    private final Runnable viewHeartbeat = new Runnable() {
        @Override
        public void run() {
            if (!running) {
                return;
            }
            color.setColor(alternatePhase ? phaseTwoColor : phaseOneColor);
            alternatePhase = !alternatePhase;
            uiHandler.postDelayed(this, intervalMs);
        }
    };

    /** Refresh-drop recovery heartbeat (View-based so API 35+ View.setRequestedFrameRate applies). Main thread only. */
    public CompositorHeartbeat(ViewGroup holder) {
        this(holder, null, Mode.VIEW_BLACK, INTERVAL_MS);
    }

    /**
     * Stream keepalive for TV / compositor pacing recovery: uses a 1x1 transparent child
     * {@link SurfaceControl} on API 29+ when {@code surfaceView} has a valid {@link SurfaceControl},
     * falling back to an imperceptible 1x1 physical pixel {@link View} otherwise. Main thread only.
     */
    public static CompositorHeartbeat forStream(ViewGroup holder, SurfaceView surfaceView) {
        return forStream(holder, surfaceView, STREAM_INTERVAL_MS);
    }

    /** {@link #forStream(ViewGroup, SurfaceView)} with an explicit tick interval in milliseconds. Main thread only. */
    public static CompositorHeartbeat forStream(ViewGroup holder, SurfaceView surfaceView, long intervalMs) {
        long resolvedInterval = intervalMs > 0 ? intervalMs : STREAM_INTERVAL_MS;
        return new CompositorHeartbeat(holder, surfaceView, resolveStreamMode(surfaceView), resolvedInterval);
    }

    private CompositorHeartbeat(ViewGroup holder, SurfaceView surfaceView, Mode mode, long intervalMs) {
        if (holder == null) {
            throw new IllegalArgumentException("holder must not be null");
        }
        this.holder = holder;
        this.targetSurfaceView = surfaceView;
        this.mode = mode;
        this.intervalMs = intervalMs;

        boolean black = mode == Mode.VIEW_BLACK;
        this.phaseOneColor = black ? COLOR_PHASE_ONE : FALLBACK_ALPHA_ONE;
        this.phaseTwoColor = black ? COLOR_PHASE_TWO : FALLBACK_ALPHA_TWO;
        this.color = new ColorDrawable(phaseOneColor);

        if (!isSurfaceControlMode(mode)) {
            initFallbackDotView();
        }
    }

    private void initFallbackDotView() {
        if (dot != null) {
            return;
        }
        View view = new View(holder.getContext());
        view.setBackground(color);
        // Never takes focus, touch or accessibility focus: touches pass to the views below.
        view.setFocusable(false);
        view.setFocusableInTouchMode(false);
        view.setClickable(false);
        view.setLongClickable(false);
        view.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
        dot = view;
        removeDot = () -> {
            if (!running && view.getParent() == holder) {
                holder.removeView(view);
            }
        };
    }

    private static boolean isSurfaceControlMode(Mode mode) {
        return mode == Mode.SURFACE_CONTROL_CLEAR;
    }

    private static Mode resolveStreamMode(SurfaceView surfaceView) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && hasValidSurfaceControl(surfaceView)) {
            return Mode.SURFACE_CONTROL_CLEAR;
        }
        return Mode.VIEW_ALPHA;
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private static boolean hasValidSurfaceControl(SurfaceView surfaceView) {
        if (surfaceView == null) {
            return false;
        }
        SurfaceControl sc = surfaceView.getSurfaceControl();
        return sc != null && sc.isValid();
    }

    /** The window's view the dot sits in. */
    public ViewGroup holder() {
        return holder;
    }

    /** The fallback dot View, or {@code null} when running on a child {@link SurfaceControl}. */
    public View dot() {
        return dot;
    }

    public boolean running() {
        return running;
    }

    /** True when backed by a zero-dot child {@link SurfaceControl}. */
    public boolean usesSurfaceControl() {
        return isSurfaceControlMode(mode);
    }

    /** True if this heartbeat already targets the same stream {@link SurfaceView} / mode state. */
    public boolean matchesStreamTarget(ViewGroup targetHolder, SurfaceView surfaceView) {
        if (stopped || this.holder != targetHolder || this.targetSurfaceView != surfaceView) {
            return false;
        }
        // Upgrade from fallback View once SurfaceView's SurfaceControl becomes valid after surfaceCreated().
        if (!isSurfaceControlMode(mode) && !surfaceControlFailed && resolveStreamMode(surfaceView) != mode) {
            return false;
        }
        return !isSurfaceControlMode(mode)
                || (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && hasValidSurfaceControl(surfaceView));
    }

    /** Starts the compositor keepalive. Main thread only. */
    public void start() {
        if (running) {
            return;
        }

        stopped = false;
        running = true;
        surfaceControlFramePosted = false;
        alternatePhase = false;
        if (isSurfaceControlMode(mode) && Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            if (startSurfaceControl()) {
                return;
            }
            // If SurfaceControl setup fails at runtime, fall back cleanly to 1x1 near-zero-alpha View.
            mode = Mode.VIEW_ALPHA;
            surfaceControlFailed = true;
            LimeLog.warning("Compositor keepalive falling back to view_alpha after SurfaceControl start failure");
        }
        startViewHeartbeat();
    }

    private void startViewHeartbeat() {
        initFallbackDotView();
        uiHandler.removeCallbacks(removeDot);
        dot.setVisibility(View.VISIBLE);
        if (dot.getParent() != holder) {
            int size = mode == Mode.VIEW_BLACK
                    ? Math.max(1, Math.round(holder.getResources().getDisplayMetrics().density))
                    : 1;
            holder.addView(dot, new FrameLayout.LayoutParams(size, size, Gravity.TOP | Gravity.LEFT));
        }
        viewHeartbeat.run();
    }

    /** Stops the keepalive and releases any child {@link SurfaceControl} or fallback {@link View}. */
    public void stop() {
        stopped = true;
        running = false;
        if (isSurfaceControlMode(mode) && Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            stopSurfaceControl();
            return;
        }
        uiHandler.removeCallbacks(viewHeartbeat);
        if (dot != null) {
            dot.setVisibility(View.GONE);
            if (dot.getParent() == holder) {
                uiHandler.removeCallbacks(removeDot);
                uiHandler.post(removeDot);
            }
        }
    }

    /** Sets the Android 15+ requested frame rate vote on the dot view so UI refresh recovery carries the pinned rate. */
    public void setRequestedFrameRate(float frameRate) {
        if (dot != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.VANILLA_ICE_CREAM) {
            dot.setRequestedFrameRate(frameRate);
        }
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private boolean startSurfaceControl() {
        SurfaceControl parentSc = targetSurfaceView != null ? targetSurfaceView.getSurfaceControl() : null;
        if (parentSc == null || !parentSc.isValid()) {
            return false;
        }
        SurfaceControl child = null;
        try {
            child = new SurfaceControl.Builder()
                    .setName("NightjarCompositorKeepalive")
                    .setParent(parentSc)
                    .setBufferSize(1, 1)
                    .setFormat(PixelFormat.RGBA_8888)
                    .setOpaque(false)
                    .build();
            try (SurfaceControl.Transaction tx = new SurfaceControl.Transaction()) {
                tx.setLayer(child, 1)
                        .setAlpha(child, 1.0f)
                        .setVisibility(child, true)
                        .apply();
            }
            final Surface surface = new Surface(child);
            scLayer = child;
            scSurface = surface;
            scThread = new HandlerThread("CompositorKeepalive");
            scThread.start();
            final Handler handler = new Handler(scThread.getLooper());
            scHandler = handler;
            LimeLog.info("Compositor keepalive started: " + mode.name().toLowerCase(java.util.Locale.ROOT)
                    + " (" + intervalMs + " ms)");
            Runnable tick = new Runnable() {
                @Override
                public void run() {
                    if (!running || scSurface != surface) {
                        return;
                    }
                    if (!postSurfaceControlTick(surface)) {
                        if (!surfaceControlFramePosted && running && scSurface == surface) {
                            uiHandler.post(() -> fallbackToViewAlphaOnMainThread(surface));
                            return;
                        }
                    }
                    if (running && scSurface == surface) {
                        handler.postDelayed(this, intervalMs);
                    }
                }
            };
            handler.post(tick);
            return true;
        } catch (RuntimeException e) {
            LimeLog.warning("Compositor keepalive SurfaceControl start failed: " + e);
            if (scSurface != null) {
                scSurface.release();
            }
            if (child != null) {
                try (SurfaceControl.Transaction tx = new SurfaceControl.Transaction()) {
                    tx.reparent(child, null).apply();
                } catch (RuntimeException ignored) {
                }
                child.release();
            }
            scLayer = null;
            scSurface = null;
            if (scThread != null) {
                scThread.quitSafely();
                scThread = null;
            }
            scHandler = null;
            return false;
        }
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private boolean postSurfaceControlTick(Surface surface) {
        if (surface == null || !surface.isValid()) {
            return false;
        }
        try {
            Canvas canvas = surface.lockCanvas(null);
            if (canvas == null) {
                LimeLog.warning("Compositor keepalive tick failed: lockCanvas returned null");
                return false;
            }
            try {
                canvas.drawColor(Color.TRANSPARENT, PorterDuff.Mode.CLEAR);
            } finally {
                surface.unlockCanvasAndPost(canvas);
            }
            surfaceControlFramePosted = true;
            return true;
        } catch (RuntimeException e) {
            LimeLog.warning("Compositor keepalive tick failed: " + e);
            return false;
        }
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    void fallbackToViewAlphaOnMainThread(Surface failedSurface) {
        if (!running || stopped || !isSurfaceControlMode(mode)
                || scSurface != failedSurface || !hasValidSurfaceControl(targetSurfaceView)) {
            return;
        }
        stopSurfaceControl();
        mode = Mode.VIEW_ALPHA;
        surfaceControlFailed = true;
        LimeLog.warning("Compositor keepalive falling back to view_alpha after initial SurfaceControl tick failure");
        startViewHeartbeat();
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private void stopSurfaceControl() {
        Handler handler = scHandler;
        HandlerThread thread = scThread;
        Surface surface = scSurface;
        SurfaceControl layer = scLayer;
        scHandler = null;
        scThread = null;
        scSurface = null;
        scLayer = null;
        if (handler != null && thread != null) {
            handler.removeCallbacksAndMessages(null);
            handler.post(() -> {
                releaseSurfaceControlResources(surface, layer);
                thread.quitSafely();
            });
        } else {
            releaseSurfaceControlResources(surface, layer);
            if (thread != null) {
                thread.quitSafely();
            }
        }
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private static void releaseSurfaceControlResources(Surface surface, SurfaceControl layer) {
        if (surface != null) {
            try {
                surface.release();
            } catch (RuntimeException ignored) {
            }
        }
        if (layer != null) {
            try {
                if (layer.isValid()) {
                    try (SurfaceControl.Transaction tx = new SurfaceControl.Transaction()) {
                        tx.reparent(layer, null).apply();
                    }
                }
            } catch (RuntimeException ignored) {
            }
            try {
                layer.release();
            } catch (RuntimeException ignored) {
            }
        }
    }
}
