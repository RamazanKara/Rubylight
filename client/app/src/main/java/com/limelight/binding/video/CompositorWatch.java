package com.limelight.binding.video;

import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.NightjarPolicyJni;

/**
 * Self-detecting display compositor monitor backed by
 * {@code nightjar_policy::video_queue::CompositorWatch}.
 *
 * <p>Healthy streams pay no runtime cost. If {@code OnFrameRendered} callbacks go dead or
 * post-handoff render timestamps show sustained compositor judder despite steady decoder output,
 * this triggers a 500 ms pulse or locks the invisible compositor keepalive layer on.
 */
public final class CompositorWatch implements AutoCloseable {
    public static final int ACTION_NONE = 0;
    public static final int ACTION_PULSE = 1;
    public static final int ACTION_KEEP_ON = 2;
    public static final int ACTION_STOP = 3;

    private final long handle = NightjarPolicyJni.queueSlabCreate(NightjarPolicyJni.QUEUE_SLAB_COMPOSITOR_WATCH);
    private final HandleReaper.Token reaper = HandleReaper.register(
            this,
            handle,
            h -> NightjarPolicyJni.queueSlabDestroy(NightjarPolicyJni.QUEUE_SLAB_COMPOSITOR_WATCH, h));
    private boolean closed;

    @Override
    public synchronized void close() {
        if (!closed) {
            closed = true;
            reaper.close();
        }
    }

    /** Resets warmup grace and current sampling bucket while keeping any locked-on decision. */
    public synchronized void resetWindow() {
        if (closed) return;
        NightjarPolicyJni.compositorWatchOp(0, handle, 0, 0, false);
    }

    /** Records a codec output release to the display surface ({@code render == true}). */
    public synchronized void onReleased(long releaseNs, boolean expectsRenderCallback) {
        if (closed) return;
        NightjarPolicyJni.compositorWatchOp(1, handle, releaseNs, 0, expectsRenderCallback);
    }

    /** Records a delivered {@code OnFrameRendered} timestamp. */
    public synchronized void onRendered(long handoffNs, long renderNs) {
        if (closed) return;
        NightjarPolicyJni.compositorWatchOp(2, handle, handoffNs, renderNs, false);
    }

    /** Polls once per second for keepalive action ({@link #ACTION_NONE}..{@link #ACTION_STOP}). */
    public synchronized int poll(long nowNs, boolean overlayVisible, boolean manualKeepaliveOn) {
        if (closed) return ACTION_NONE;
        return (int) NightjarPolicyJni.compositorWatchOp(
                3, handle, nowNs, manualKeepaliveOn ? 1L : 0L, overlayVisible);
    }

    /** True once automatic keepalive has proven recovery and locked on for this stream. */
    public synchronized boolean isLockedOn() {
        if (closed) return false;
        return NightjarPolicyJni.compositorWatchOp(4, handle, 0, 0, false) != 0;
    }

    /** Drains one human-readable transition message for logging, or {@code null}. */
    public synchronized String takeEvent() {
        if (closed) return null;
        return NightjarPolicyJni.compositorWatchTakeEvent(handle);
    }
}
