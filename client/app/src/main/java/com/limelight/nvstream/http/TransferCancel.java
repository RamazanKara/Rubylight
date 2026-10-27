package com.limelight.nvstream.http;

import okhttp3.Call;

/**
 * Cancels one upload or download from another thread, at once, even while it waits on the host: its call is
 * cancelled, which closes the socket. One per transfer; the NvHTTP's other requests carry on, unlike
 * {@link NvHTTP#cancelPendingRequests}. A transfer started with a cancelled handle fails before it connects.
 */
public final class TransferCancel {
    private Call call;
    private boolean cancelled;

    public void cancel() {
        Call current;
        synchronized (this) {
            cancelled = true;
            current = call;
        }
        if (current != null) {
            current.cancel();
        }
    }

    public synchronized boolean isCancelled() {
        return cancelled;
    }

    void attach(Call call) {
        boolean cancel;
        synchronized (this) {
            this.call = call;
            cancel = cancelled;
        }
        if (cancel) {
            call.cancel();
        }
    }
}
