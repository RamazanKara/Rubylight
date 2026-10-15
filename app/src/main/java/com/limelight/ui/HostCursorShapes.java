package com.limelight.ui;

import android.content.res.Resources;
import android.graphics.Bitmap;
import android.graphics.drawable.BitmapDrawable;
import android.graphics.drawable.Drawable;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.os.Process;

import com.limelight.LimeLog;
import com.limelight.binding.input.touch.CursorShapeCache;
import com.limelight.binding.input.touch.CursorShapeMessage;
import com.limelight.binding.input.touch.TrackpadPointer;

import java.util.concurrent.atomic.AtomicInteger;

/**
 * Draws the host's cursor shapes (docs/protocol/cursor-shape.md) on the Local cursor. Messages
 * arrive on the control stream's receive thread and are only queued there; checking, decoding and
 * scaling run on a thread of their own, and the UI thread just swaps the drawable. Anything
 * malformed is logged, shows Nightjar's own arrow and is reported once per session.
 */
public final class HostCursorShapes {
    /** More queued messages than this means the host floods the channel; the excess is dropped. */
    private static final int MAX_PENDING = 64;

    private static final class Shape {
        final Drawable drawable;
        final float hotspotX, hotspotY;

        Shape(Drawable drawable, float hotspotX, float hotspotY) {
            this.drawable = drawable;
            this.hotspotX = hotspotX;
            this.hotspotY = hotspotY;
        }
    }

    private final Resources resources;
    private final LocalCursorView cursorView;
    private final TrackpadPointer pointer;
    private final Runnable rejected;
    private final HandlerThread thread = new HandlerThread("Cursor shapes", Process.THREAD_PRIORITY_BACKGROUND);
    private final Handler decoder;
    private final Handler ui = new Handler(Looper.getMainLooper());
    private final AtomicInteger pending = new AtomicInteger();
    // Decoder thread only.
    private final CursorShapeCache<Shape> cache = new CursorShapeCache<>();
    private boolean rejectionReported;
    private volatile boolean closed;
    // What the UI thread shows next. Every message updates the cache in order, but a burst only
    // reaches the UI as its newest state: one post, one swap.
    private final Object displayLock = new Object();
    private boolean displayPosted, displayShape, displayHidden;
    private Shape displayed;
    private final Runnable display = this::display;

    /** rejected runs on the UI thread, at most once, after the first message that could not be drawn. */
    public HostCursorShapes(Resources resources, LocalCursorView cursorView, TrackpadPointer pointer, Runnable rejected) {
        this.resources = resources;
        this.cursorView = cursorView;
        this.pointer = pointer;
        this.rejected = rejected;
        thread.start();
        decoder = new Handler(thread.getLooper());
    }

    /** Any thread; the message is not copied, so the caller must not reuse it. */
    public void accept(byte[] message) {
        if (closed) return;
        if (pending.incrementAndGet() > MAX_PENDING) {
            pending.decrementAndGet();
            LimeLog.warning("Cursor shape dropped: more than " + MAX_PENDING + " waiting to be decoded");
            return;
        }
        decoder.post(() -> {
            pending.decrementAndGet();
            if (!closed) handle(message);
        });
    }

    /** UI thread: stops decoding and puts back Nightjar's arrow. */
    public void close() {
        closed = true;
        thread.quitSafely();
        ui.removeCallbacksAndMessages(null);
        cursorView.setDefaultCursor();
        pointer.setHostCursorHidden(false);
    }

    private void handle(byte[] bytes) {
        CursorShapeMessage message;
        try {
            message = CursorShapeMessage.parse(bytes);
        } catch (CursorShapeMessage.MalformedException e) {
            reject("Cursor shape rejected: " + e.getMessage());
            return;
        }
        switch (message.kind) {
            case HIDE:
                show(null, true);
                break;
            case DEFAULT:
                show(null, false);
                break;
            case SHOW: {
                Shape shape = cache.get(message.id);
                if (shape == null) {
                    reject("Cursor shape " + (message.id & 0xFFFFFFFFL) + " was never sent or has been dropped");
                } else {
                    show(shape, false);
                }
                break;
            }
            case IMAGE: {
                Shape shape;
                try {
                    shape = decode(message);
                } catch (RuntimeException | OutOfMemoryError e) {
                    cache.remove(message.id);
                    reject("Cursor shape " + (message.id & 0xFFFFFFFFL) + " could not be decoded: " + e);
                    return;
                }
                cache.put(message.id, shape);
                show(shape, false);
                break;
            }
        }
    }

    /**
     * One host pixel at the host's 100% scale becomes one dp here, so a 32-pixel Windows cursor
     * sent from a 200% display (64 pixels) is as large as one from a 100% display.
     */
    private Shape decode(CursorShapeMessage message) {
        Bitmap bitmap = Bitmap.createBitmap(message.argbPixels(), message.width, message.height, Bitmap.Config.ARGB_8888);
        float scale = resources.getDisplayMetrics().density * 100f / message.scalePercent;
        int width = Math.max(1, Math.round(message.width * scale));
        int height = Math.max(1, Math.round(message.height * scale));
        if (width != message.width || height != message.height) {
            Bitmap scaled = Bitmap.createScaledBitmap(bitmap, width, height, true);
            if (scaled != bitmap) bitmap.recycle();
            bitmap = scaled;
        }
        bitmap.setDensity(Bitmap.DENSITY_NONE);
        return new Shape(new BitmapDrawable(resources, bitmap),
                message.hotspotX * (float) width / message.width, message.hotspotY * (float) height / message.height);
    }

    private void reject(String reason) {
        LimeLog.warning(reason);
        boolean report = !rejectionReported;
        rejectionReported = true;
        show(null, false);
        if (report) ui.post(() -> {
            if (!closed) rejected.run();
        });
    }

    /** Hidden draws nothing and keeps the current image; otherwise a null shape is Nightjar's arrow. */
    private void show(Shape shape, boolean hidden) {
        synchronized (displayLock) {
            if (!hidden) {
                displayShape = true;
                displayed = shape;
            }
            displayHidden = hidden;
            if (displayPosted) return;
            displayPosted = true;
        }
        ui.post(display);
    }

    private void display() {
        boolean setShape, hidden;
        Shape shape;
        synchronized (displayLock) {
            displayPosted = false;
            setShape = displayShape;
            displayShape = false;
            shape = displayed;
            displayed = null;
            hidden = displayHidden;
        }
        if (closed) return;
        if (setShape && shape == null) {
            cursorView.setDefaultCursor();
        } else if (setShape) {
            cursorView.setCursor(shape.drawable, shape.hotspotX, shape.hotspotY);
        }
        pointer.setHostCursorHidden(hidden);
    }
}
