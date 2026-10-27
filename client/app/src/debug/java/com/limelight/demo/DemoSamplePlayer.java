package com.limelight.demo;

import android.content.res.AssetFileDescriptor;
import android.media.MediaCodec;
import android.media.MediaExtractor;
import android.media.MediaFormat;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.view.Choreographer;
import android.view.Surface;

import androidx.annotation.RequiresApi;

import java.io.IOException;
import java.nio.ByteBuffer;
import java.util.ArrayDeque;

/**
 * Loops the 60 fps demo sample into the stream surface and shows one sample frame per sixtieth of a second of
 * display refreshes, counted on Choreographer. MediaPlayer follows the media clock instead: on an emulator, whose
 * refreshes drift against that clock, it repeats and skips frames, so screen recordings of the demo judder.
 * The end of the file is followed by its start again without a flush, so the loop has no gap.
 */
final class DemoSamplePlayer implements Choreographer.FrameCallback {
    interface Listener {
        void onSampleFailed();
    }

    private static final double SAMPLE_FPS = 60;
    private final HandlerThread thread = new HandlerThread("Demo sample");
    private final ArrayDeque<Integer> decoded = new ArrayDeque<>();
    private final ArrayDeque<Long> times = new ArrayDeque<>();
    private final Listener listener;
    private final double samplesPerRefresh;
    private final long refreshNanos;
    private MediaExtractor extractor;
    private MediaCodec codec;
    private long loopOffsetUs, durationUs, lastSampleUs, resumeUs, shownUs;
    private double due;
    private boolean running, released;

    DemoSamplePlayer(double refreshRate, Listener listener) {
        this.listener = listener;
        double rate = refreshRate >= 1 ? refreshRate : SAMPLE_FPS;
        samplesPerRefresh = SAMPLE_FPS / rate;
        refreshNanos = (long) (1e9 / rate);
    }

    /** Starts decoding into the surface from the given sample time; frames show once {@link #setRunning} is true. */
    @RequiresApi(23)
    void start(AssetFileDescriptor asset, Surface surface, long positionUs) throws IOException {
        extractor = new MediaExtractor();
        extractor.setDataSource(asset.getFileDescriptor(), asset.getStartOffset(), asset.getLength());
        MediaFormat format = null;
        for (int i = 0; i < extractor.getTrackCount(); i++) {
            MediaFormat track = extractor.getTrackFormat(i);
            String mime = track.getString(MediaFormat.KEY_MIME);
            if (mime != null && mime.startsWith("video/")) {
                extractor.selectTrack(i);
                format = track;
                break;
            }
        }
        if (format == null) throw new IOException("The demo sample has no video track");
        durationUs = format.containsKey(MediaFormat.KEY_DURATION) ? format.getLong(MediaFormat.KEY_DURATION) : 10_000_000L;
        resumeUs = positionUs % Math.max(1, durationUs);
        shownUs = resumeUs;
        extractor.seekTo(resumeUs, MediaExtractor.SEEK_TO_PREVIOUS_SYNC);
        thread.start();
        Handler handler = new Handler(thread.getLooper());
        codec = MediaCodec.createDecoderByType(format.getString(MediaFormat.KEY_MIME));
        codec.setCallback(new MediaCodec.Callback() {
            @Override public void onInputBufferAvailable(MediaCodec mc, int index) {
                queueInput(mc, index);
            }

            @Override public void onOutputBufferAvailable(MediaCodec mc, int index, MediaCodec.BufferInfo info) {
                synchronized (decoded) {
                    if (released) return;
                    if (info.presentationTimeUs < resumeUs) {
                        // Decoded on the way from the sync frame to the resume point: not shown.
                        mc.releaseOutputBuffer(index, false);
                        return;
                    }
                    decoded.add(index);
                    times.add(info.presentationTimeUs);
                }
            }

            @Override public void onError(MediaCodec mc, MediaCodec.CodecException e) {
                fail();
            }

            @Override public void onOutputFormatChanged(MediaCodec mc, MediaFormat changed) { }
        }, handler);
        codec.configure(format, surface, null, 0);
        codec.start();
    }

    private void queueInput(MediaCodec mc, int index) {
        try {
            ByteBuffer buffer = mc.getInputBuffer(index);
            int size = extractor.readSampleData(buffer, 0);
            if (size < 0) {
                // The end: carry on from the start, later in presentation time.
                loopOffsetUs += Math.max(durationUs, lastSampleUs + (long) (1_000_000 / SAMPLE_FPS));
                extractor.seekTo(0, MediaExtractor.SEEK_TO_CLOSEST_SYNC);
                size = extractor.readSampleData(buffer, 0);
                if (size < 0) throw new IllegalStateException("The demo sample is empty");
            }
            lastSampleUs = extractor.getSampleTime();
            mc.queueInputBuffer(index, 0, size, loopOffsetUs + lastSampleUs, 0);
            extractor.advance();
        } catch (IllegalStateException e) {
            fail();
        }
    }

    private void fail() {
        new Handler(Looper.getMainLooper()).post(() -> {
            if (released) return;
            setRunning(false);
            listener.onSampleFailed();
        });
    }

    void setRunning(boolean value) {
        if (running == value || released) return;
        running = value;
        Choreographer choreographer = Choreographer.getInstance();
        if (running) {
            due = 0;
            choreographer.postFrameCallback(this);
        } else {
            choreographer.removeFrameCallback(this);
        }
    }

    @Override public void doFrame(long frameTimeNanos) {
        if (!running || released) return;
        // One sample frame per sixtieth of a second of refreshes; a refresh that finds no decoded frame waits.
        due = Math.min(due + samplesPerRefresh, 1);
        synchronized (decoded) {
            // A 60.000004 Hz panel adds 0.99999994 per refresh: allow for rounding.
            if (due >= 0.999 && !decoded.isEmpty()) {
                due = Math.max(due - 1, 0);
                shownUs = times.remove();
                try {
                    // For the refresh after next: a frame whose decode finishes a little late still gets its own refresh.
                    codec.releaseOutputBuffer(decoded.remove(), frameTimeNanos + 2 * refreshNanos);
                } catch (IllegalStateException e) {
                    fail();
                    return;
                }
            }
        }
        Choreographer.getInstance().postFrameCallback(this);
    }

    /** The sample time last shown, for resuming after the surface is replaced. */
    long positionUs() {
        return shownUs;
    }

    void release() {
        if (released) return;
        setRunning(false);
        synchronized (decoded) {
            released = true;
            decoded.clear();
            times.clear();
        }
        if (codec != null) {
            try {
                codec.stop();
            } catch (IllegalStateException ignored) {
            }
            codec.release();
        }
        thread.quitSafely();
        if (extractor != null) extractor.release();
    }
}
