package com.limelight.binding.video;

import java.io.IOException;
import java.util.List;

import android.media.MediaCodec;
import android.media.MediaCodec.CodecException;
import android.os.SystemClock;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.preferences.PreferenceConfiguration;

/**
 * Manages MediaCodec thread quiescing, flush/restart/reset recovery, back-off,
 * exception handling, hang-reset watchdogs, and vendor low-latency key hang
 * persistence for {@link MediaCodecDecoderRenderer}.
 */
final class MediaCodecRecovery {
    static final long HANG_RESET_TIMEOUT_MS = 10_000;
    private static final long RECORD_INPUT_WAIT_MS = 500;
    static final int EXCEPTION_REPORT_DELAY_MS = 3000;

    private final MediaCodecDecoderRenderer r;

    // Frames out and uptime when the last recovery ran: CR_REFILL_FRAMES good frames and
    // CR_REFILL_MS since refill the attempts, so only a crash loop uses them up (one every few
    // seconds at 240 FPS included), not ten recoveries hours apart
    private int framesOutAtLastRecovery;
    private long uptimeAtLastRecoveryMs;
    volatile long uptimeAtHangResetMs;
    boolean hangResetWithoutKeys;
    // Whether the running recovery went past its first step (CodecRecoverySteps.Codec.escalated).
    private boolean recoveryEscalated;

    private volatile MediaCodecDecoderRenderer.RendererException initialException;
    volatile long initialExceptionTimestamp;
    // Frames out when initialException was taken; once more come out, it is forgiven
    private int initialExceptionFramesOut;

    MediaCodecRecovery(MediaCodecDecoderRenderer renderer) {
        this.r = renderer;
    }

    // All threads that interact with the MediaCodec instance must call this function regularly!
    boolean doCodecRecoveryIfRequired(int quiescenceFlag) {
        // NB: We cannot check 'stopping' here because we could end up bailing in a partially
        // quiesced state that will cause the quiesced threads to never wake up.
        if (r.codecRecoveryType.get() == MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE) {
            // Common case
            return false;
        }

        // We need some sort of recovery, so quiesce all threads before starting that
        synchronized (r.codecRecoveryMonitor) {
            if (r.vsyncPacer.choreographerHandlerThread == null ||
                    r.prefs.framePacing != PreferenceConfiguration.FRAME_PACING_BALANCED) {
                // Only balanced pacing touches codec buffers on the Choreographer thread.
                r.codecRecoveryThreadQuiescedFlags |= MediaCodecDecoderRenderer.CR_FLAG_CHOREOGRAPHER;
            }

            r.codecRecoveryThreadQuiescedFlags |= quiescenceFlag;

            // This is the final thread to quiesce, so let's perform the codec recovery now.
            if (r.codecRecoveryThreadQuiescedFlags == MediaCodecDecoderRenderer.CR_FLAG_ALL) {
                // Any recovery leaves the decoder unrecorded (recordVendorKeyHang) unless it ends as one
                // of two that set the flag again below: the setup without vendor keys succeeding on the
                // decoder that hung, or a planned restart done at its first step on the same decoder.
                boolean unrecordedBefore = r.session.vendorKeyHangUnrecorded.getAndSet(false);
                if (r.upscaler != null) {
                    // A flush can strand frames in the texture queue. Restart onto direct output.
                    if (r.packing == null) r.upscalingPolicy.fail(UpscalingPolicy.Reason.GPU_ERROR);
                    if (r.codecRecoveryType.compareAndSet(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_FLUSH,
                            MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART)) {
                        r.restartReason = "flush with the upscaler";
                    }
                }
                // Input and output buffers are invalidated by stop() and reset().
                r.decodeLoop.clearInputBuffersOnRecovery();
                synchronized (r.outputBufferQueue) {
                    r.outputBufferQueue.clear();
                }
                r.vsyncPacer.resetPacingOnRecovery();
                r.outputBufferQueueLimit = 1;
                r.frameLatencyStats.discardPending("codec_reset");
                // Frames inside the codec are gone; the scheduler counts from here.
                r.decoderDepthBase = r.numFramesIn - r.numFramesOut;
                r.submitWork.reset();
                r.debt.reset();
                if (r.hostFrameTimes) {
                    r.hostFrameClock.reset();
                    r.presentScheduler.reset();
                    r.residency.reset();
                    r.inFlight.reset();
                }

                // If we just need a flush, do so now with all threads quiesced.
                if (r.codecRecoveryType.get() == MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_FLUSH) {
                    LimeLog.warning("Flushing decoder");
                    try {
                        r.videoDecoder.flush();
                        r.codecRecoveryType.set(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE);
                    } catch (IllegalStateException e) {
                        e.printStackTrace();

                        // Something went wrong during the restart, let's use a bigger hammer
                        // and try a reset instead.
                        r.restartReason = "failed flush";
                        r.recoveryAfterFailure = true;
                        r.codecRecoveryType.set(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART);
                    }
                }

                // Recoveries in a row that bring no frame back off, then give the decoder up.
                if (r.codecRecoveryType.get() != MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE &&
                        r.recoveryAfterFailure && !backOffBeforeRecovery()) {
                    return true;
                }

                // We don't count flushes as codec recovery attempts
                if (r.codecRecoveryType.get() != MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE) {
                    r.codecRecoveryAttempts++;
                    framesOutAtLastRecovery = r.numFramesOut;
                    uptimeAtLastRecoveryMs = SystemClock.uptimeMillis();
                    LimeLog.info("Codec recovery attempt: " + r.codecRecoveryAttempts);

                    // The codec is configured again, with the current metadata.
                    r.uncheckedHdrStaticInfo = null;
                    // A decoder that hung with its vendor keys is set up anew without them (resetHungDecoder).
                    boolean withoutKeys = r.restartWithoutVendorKeys;
                    boolean toVulkan = r.rebuildForVulkan;
                    String decoderBefore = r.session.lowLatencyKeysDecoder;
                    // Planned (size, HDR, input buffer growth): a restart no failure asked for.
                    boolean planned = !r.recoveryAfterFailure &&
                            r.codecRecoveryType.get() == MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART;
                    recoveryEscalated = false;
                    try {
                        if (toVulkan && r.upscaler != null) {
                            // Disconnect EGL from renderTarget before Vulkan attaches to it.
                            r.upscalingPolicy.fail(UpscalingPolicy.Reason.DISABLED);
                            r.vulkanHelper.closeUpscaler(true);
                        }
                        boolean recovered = withoutKeys ?
                                CodecRecoverySteps.rebuild("it took no input with its vendor low-latency keys", r.recoveryCodec) :
                                r.dolbyVisionActive && r.dolbyVisionFailed ?
                                        CodecRecoverySteps.rebuild("the stream lost its Dolby Vision metadata", r.recoveryCodec) :
                                toVulkan ? CodecRecoverySteps.rebuild("full color range needs the Vulkan renderer", r.recoveryCodec) :
                                CodecRecoverySteps.run(r.codecRecoveryType.get() == MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART,
                                        r.restartRecreates, r.restartReason, r.recoveryCodec);
                        if (toVulkan) r.rebuildForVulkan = false;
                        if (!recovered) {
                            if (!r.stopping && r.renderTarget != null && r.renderTarget.isValid()) {
                                // The surface is fine: the decoder refused its configuration.
                                r.telemetry.failDecoder(r.telemetry.decoderName() + " refused its configuration after a recovery", null);
                                return true;
                            }
                            // Our Surface is gone, or Vulkan handover failed and already stopped the stream.
                            r.stopping = true;
                        }
                        r.codecRecoveryType.set(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE);
                        r.recoveryAfterFailure = false;
                        r.hangResetPending = false;
                        boolean sameDecoder = decoderBefore != null && decoderBefore.equals(r.session.lowLatencyKeysDecoder);
                        if (withoutKeys) {
                            r.restartWithoutVendorKeys = false;
                            // Only the decoder and MIME type that hung: a refused setup may have handed
                            // the stream to another (a Dolby Vision decoder to HEVC or AV1).
                            r.session.vendorKeyHangUnrecorded.set(sameDecoder);
                        } else if (planned && recovered && !recoveryEscalated && sameDecoder) {
                            // Done at its first step (stop or a new codec, then configure and start):
                            // nothing failed, so the flag is what it was before the restart.
                            r.session.vendorKeyHangUnrecorded.set(unrecordedBefore);
                        }
                    } catch (IllegalStateException e) {
                        // Not even a decoder set up from scratch works: end the stream.
                        e.printStackTrace();
                        r.telemetry.failDecoder(r.telemetry.decoderName() + " could not be set up again after it failed (" + e + ")", e);
                        return true;
                    }
                }

                // Wake all quiesced threads and allow them to begin work again
                r.codecRecoveryThreadQuiescedFlags = 0;
                r.codecRecoveryMonitor.notifyAll();
            } else {
                // If we haven't quiesced all threads yet, wait to be signalled after recovery.
                // The final thread to be quiesced will handle the codec recovery.
                // Every end of a recovery clears the flags: a failure right after it asks for the
                // next one before this thread looks again, and that one needs this thread to
                // quiesce anew. Waiting on would hang both decoder threads for good.
                while (r.codecRecoveryType.get() != MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE &&
                        (r.codecRecoveryThreadQuiescedFlags & quiescenceFlag) != 0) {
                    try {
                        LimeLog.info("Waiting to quiesce decoder threads: " + r.codecRecoveryThreadQuiescedFlags);
                        r.codecRecoveryMonitor.wait(1000);
                    } catch (InterruptedException e) {
                        e.printStackTrace();

                        // InterruptedException clears the thread's interrupt status. Since we can't
                        // handle that here, we will re-interrupt the thread to set the interrupt
                        // status back to true.
                        Thread.currentThread().interrupt();

                        break;
                    }
                }
            }
        }

        return true;
    }

    // With every thread quiesced: waits before a recovery that follows others without a frame, or
    // gives the decoder up after too many (a decoder out of memory fails each reset at once).
    // False when the stream is ending instead; the quiesced threads have been woken then.
    private boolean backOffBeforeRecovery() {
        long delayMs = r.recoveryBackoff.beforeRecovery(r.numFramesOut);
        if (delayMs < 0) {
            r.telemetry.failDecoder(r.telemetry.decoderName() + " failed again after " + (r.recoveryBackoff.recoveriesWithoutFrame() - 1) +
                    " recoveries in a row without a frame at " + r.initialWidth + "x" + r.initialHeight +
                    " (out of memory, or no free decoder)", null);
            return false;
        }
        if (delayMs > 0) {
            LimeLog.warning("Decoder failed again without a frame; waiting " + delayMs + " ms before recovery " +
                    r.recoveryBackoff.recoveriesWithoutFrame());
            long deadlineNs = System.nanoTime() + delayMs * 1_000_000L;
            try {
                // wait() lets prepareForStop() in; it ends the recovery.
                for (long left = delayMs; left > 0 && !r.stopping; left = (deadlineNs - System.nanoTime()) / 1_000_000L) {
                    r.codecRecoveryMonitor.wait(left);
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
            if (r.stopping || Thread.currentThread().isInterrupted()) {
                r.codecRecoveryType.set(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE);
                r.codecRecoveryThreadQuiescedFlags = 0;
                r.codecRecoveryMonitor.notifyAll();
                return false;
            }
        }
        return true;
    }

    /**
     * A frame came without its Dolby Vision RPU (a fallback to SDR sends none): it never reaches the
     * Dolby Vision decoder. The stream keeps its base layer's decoder until the next ANNOUNCE, also
     * when RPUs come back: changing decoders twice more is not worth a device restart. Called on the
     * input thread.
     */
    int leaveDolbyVision() {
        synchronized (r.codecRecoveryMonitor) {
            if (r.stopping) return MoonBridge.DR_OK;
            r.dolbyVisionFailed = true;
            r.codecRecoveryAttempts = 0;
            LimeLog.warning("The PC sent a frame without Dolby Vision metadata; decoding the stream as " +
                    r.codecSelector.dolbyVisionBaseCodec() + " with its " +
                    HdrFormatPolicy.baseLayerName(r.announcedHdrFormat) + " base layer until the next stream");
            r.session.requestRestart("a frame without Dolby Vision metadata");
        }
        doCodecRecoveryIfRequired(MediaCodecDecoderRenderer.CR_FLAG_INPUT_THREAD);
        return r.stopping ? MoonBridge.DR_OK : MoonBridge.DR_NEED_IDR;
    }

    // The recovery steps on videoDecoder, with every thread quiesced under codecRecoveryMonitor.
    CodecRecoverySteps.Codec createRecoveryCodec() {
        return new CodecRecoverySteps.Codec() {
            @Override public void stop() {
                r.videoDecoder.stop();
            }

            @Override public boolean recreate() {
                // release() returns once the codec is gone; then a new one of the same name.
                String name = r.videoDecoder.getName();
                r.videoDecoder.release();
                try {
                    r.videoDecoder = MediaCodec.createByCodecName(name);
                    return true;
                } catch (IOException | IllegalArgumentException | IllegalStateException e) {
                    // IllegalStateException: a MediaCodec.CodecException, as when no decoder is free yet.
                    e.printStackTrace();
                    LimeLog.warning("Decoder " + name + " could not be created again after its release (" + e +
                            "); setting the decoder up from scratch");
                    return false;
                }
            }

            @Override public void reset() {
                r.videoDecoder.reset();
            }

            @Override public void configure() {
                if (r.packing != null && r.upscalingPolicy.getReason() == UpscalingPolicy.Reason.NONE) {
                    r.vulkanHelper.closeUpscaler();
                    r.vulkanHelper.setupUpscaling();
                }
                if (!r.session.configureAndStartDecoder(r.configuredFormat)) {
                    throw new IllegalArgumentException("Vulkan handover stopped before decoder configure");
                }
            }

            @Override public void rebuild(boolean codecFailed) {
                // It may move to another decoder: no size change is offered until that one is configured.
                r.reconfigure.startRebuild();
                if (codecFailed) {
                    r.videoDecoder.release();
                    // Without its vendor keys the same decoder is set up again: it is what is tried.
                    if (r.dolbyVisionActive && !r.dolbyVisionFailed && !r.restartWithoutVendorKeys) {
                        // The same stream decodes as HEVC or AV1; the Dolby Vision decoder is not tried again.
                        r.dolbyVisionFailed = true;
                        LimeLog.warning("Dolby Vision decoder failed while streaming; recreating the decoder for " +
                                r.codecSelector.dolbyVisionBaseCodec());
                    }
                }
                int err = r.initializeDecoder(true);
                if (err != 0) {
                    throw new IllegalStateException("Decoder reset failed: " + err);
                }
            }

            @Override public void escalated() {
                // A planned restart that went past its first step failed: it does not set the flag back.
                recoveryEscalated = true;
            }
        };
    }

    // Returns true if the exception is transient
    boolean handleDecoderException(IllegalStateException e) {
        // Eat decoder exceptions if we're in the process of stopping
        if (r.stopping) {
            return false;
        }

        if (e instanceof CodecException) {
            CodecException codecExc = (CodecException) e;

            if (codecExc.isTransient()) {
                // We'll let transient exceptions go
                LimeLog.warning(codecExc.getDiagnosticInfo());
                return true;
            }

            LimeLog.severe(codecExc.getDiagnosticInfo());
            // A decoder that fails after its setup without vendor keys did not run well, whether a
            // recovery follows or the exception is swallowed (no attempt left, a recovery pending).
            r.session.vendorKeyHangUnrecorded.set(false);

            // We can attempt a recovery or reset at this stage to try to start decoding again
            if (recoveryAttemptsLeft()) {
                // If the exception is non-recoverable or we already require a reset, perform a reset.
                // If we have no prior unrecoverable failure, we will try a restart instead.
                if (codecExc.isRecoverable()) {
                    if (r.codecRecoveryType.compareAndSet(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE,
                            MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART)) {
                        r.restartReason = "CodecException";
                        LimeLog.info("Decoder requires restart for recoverable CodecException");
                        e.printStackTrace();
                    } else if (r.codecRecoveryType.compareAndSet(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_FLUSH,
                            MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART)) {
                        r.restartReason = "CodecException";
                        LimeLog.info("Decoder flush promoted to restart for recoverable CodecException");
                        e.printStackTrace();
                    } else if (!checkExpectedRecoveryType(true)) {
                        return false;
                    }
                } else {
                    if (r.codecRecoveryType.compareAndSet(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE,
                            MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESET)) {
                        LimeLog.info("Decoder requires reset for non-recoverable CodecException");
                        e.printStackTrace();
                    } else if (r.codecRecoveryType.compareAndSet(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_FLUSH,
                            MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESET)) {
                        LimeLog.info("Decoder flush promoted to reset for non-recoverable CodecException");
                        e.printStackTrace();
                    } else if (r.codecRecoveryType.compareAndSet(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART,
                            MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESET)) {
                        LimeLog.info("Decoder restart promoted to reset for non-recoverable CodecException");
                        e.printStackTrace();
                    } else if (!checkExpectedRecoveryType(false)) {
                        return false;
                    }
                }

                // The recovery will take place when all threads reach doCodecRecoveryIfRequired().
                r.recoveryAfterFailure = true;
                return false;
            }
        } else {
            // IllegalStateException was primarily used prior to the introduction of CodecException.
            // Recovery from this requires a full decoder reset.
            //
            // NB: CodecException is an IllegalStateException, so we must check for it first.
            // As above: not running well after the setup without vendor keys, recovered or not.
            r.session.vendorKeyHangUnrecorded.set(false);
            if (recoveryAttemptsLeft()) {
                if (r.codecRecoveryType.compareAndSet(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE,
                        MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESET)) {
                    LimeLog.info("Decoder requires reset for IllegalStateException");
                    e.printStackTrace();
                } else if (r.codecRecoveryType.compareAndSet(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_FLUSH,
                        MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESET)) {
                    LimeLog.info("Decoder flush promoted to reset for IllegalStateException");
                    e.printStackTrace();
                } else if (r.codecRecoveryType.compareAndSet(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART,
                        MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESET)) {
                    LimeLog.info("Decoder restart promoted to reset for IllegalStateException");
                    e.printStackTrace();
                } else if (!checkExpectedRecoveryType(false)) {
                    return false;
                }

                r.recoveryAfterFailure = true;
                return false;
            }
        }

        // Only throw if we're not in the middle of codec recovery
        if (r.codecRecoveryType.get() == MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_NONE) {
            //
            // There seems to be a race condition with decoder/surface teardown causing some
            // decoders to throw IllegalStateExceptions even before 'stopping' is set.
            // To workaround this while allowing real exceptions to propagate, we will eat the
            // first exception. If we are still receiving exceptions 3 seconds later, we will
            // throw the original exception again.
            //
            // Read once: frames coming out on another thread may forgive it meanwhile.
            MediaCodecDecoderRenderer.RendererException pending = unresolvedInitialException();
            if (pending != null) {
                // This isn't the first time we've had an exception processing video
                if (SystemClock.uptimeMillis() - initialExceptionTimestamp >= EXCEPTION_REPORT_DELAY_MS) {
                    // It's been over 3 seconds and we're still getting exceptions: end the stream.
                    // Throwing crashed the app here, as the codec may be released by now.
                    Exception first = (Exception) pending.getCause();
                    r.telemetry.failDecoder(r.telemetry.decoderName() + " kept failing after " + r.codecRecoveryAttempts +
                            " recoveries (" + first + ")", first);
                }
            } else {
                // This is the first exception we've hit
                // The timestamp and frame count first: whoever sees the exception sees them too.
                initialExceptionTimestamp = SystemClock.uptimeMillis();
                initialExceptionFramesOut = r.numFramesOut;
                initialException = new MediaCodecDecoderRenderer.RendererException(r, e);
            }
        }

        // Not transient
        return false;
    }

    private boolean checkExpectedRecoveryType(boolean allowRestart) {
        int type = r.codecRecoveryType.get();
        if (type == MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESET ||
                (allowRestart && type == MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART)) {
            return true;
        }
        if (r.stopping) {
            return false;
        }
        throw new IllegalStateException("Unexpected codec recovery type: " + type);
    }

    // The first exception, until the decoder puts out a frame after it: then the stream recovered,
    // and neither the hang check nor a later exception's 3 s rule should go by it.
    MediaCodecDecoderRenderer.RendererException unresolvedInitialException() {
        MediaCodecDecoderRenderer.RendererException pending = initialException;
        if (pending != null && r.numFramesOut != initialExceptionFramesOut) {
            initialException = null;
            return null;
        }
        return pending;
    }

    boolean resetHungDecoder(int deltaMs) {
        // Hung without its keys before it ran well: the keys were not all of it, nothing is recorded.
        // Cleared before the reset's frames and uptime change, which recordVendorKeyHang reads first.
        r.session.vendorKeyHangUnrecorded.set(false);
        List<String> vendorKeys = r.lowLatencyKeysVendor;
        String decoder = r.session.lowLatencyKeysDecoder;
        final int framesOut = r.numFramesOut;
        int decision = NightjarPolicyJni.decoderRecoveryOp(2, !vendorKeys.isEmpty(), decoder != null,
                decoder != null && r.hungKeys.containsKey(decoder), r.codecRecoveryAttempts, framesOut, framesOutAtLastRecovery,
                r.framesOutAtHangReset, SystemClock.uptimeMillis(), uptimeAtLastRecoveryMs, uptimeAtHangResetMs);
        if ((decision & 4) != 0) {
            LimeLog.info("Codec recovery attempts refilled after " + (framesOut - framesOutAtLastRecovery) + " frames since the last");
            r.codecRecoveryAttempts = 0;
        }
        if ((decision & 3) == 0) return false;
        boolean withoutKeys = (decision & 3) == 2;
        boolean afterReset = r.framesOutAtHangReset >= 0;
        synchronized (r.codecRecoveryMonitor) {
            if (r.stopping) return true;
            r.framesOutAtHangReset = r.numFramesOut;
            uptimeAtHangResetMs = SystemClock.uptimeMillis();
            hangResetWithoutKeys = withoutKeys;
            if (withoutKeys) {
                r.hungKeys.put(decoder, new MediaCodecDecoderRenderer.HungKeys(vendorKeys, false));
                r.restartWithoutVendorKeys = true;
            }
            r.recoveryAfterFailure = true;
            r.hangResetPending = true;
            r.codecRecoveryType.set(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESET);
        }
        if (withoutKeys) {
            LimeLog.warning(r.telemetry.decoderName() + " took no input for " + deltaMs + " ms with its vendor low-latency keys" +
                    (afterReset ? ", again after a reset" : "") + "; setting it up again without them: " +
                    MediaCodecTelemetry.describeKeys(vendorKeys));
        } else {
            LimeLog.warning(r.telemetry.decoderName() + " took no input for " + deltaMs + " ms; resetting it");
        }
        long timeoutMs = r.hangResetTimeoutMs;
        String recovery = withoutKeys ? "its setup without vendor low-latency keys" : "its reset";
        Thread watchdog = new Thread(() -> {
            long deadlineNs = System.nanoTime() + timeoutMs * 1_000_000L;
            try {
                for (long left = timeoutMs; left > 0 && r.hangResetPending && !r.stopping;
                     left = (deadlineNs - System.nanoTime()) / 1_000_000L) {
                    Thread.sleep(Math.min(left, 100));
                }
            } catch (InterruptedException e) {
                return;
            }
            if (!r.hangResetPending || r.stopping) return;
            // The reset may hang inside MediaCodec holding codecRecoveryMonitor: end the stream without it.
            String reason = r.telemetry.markDecoderFailed(r.telemetry.decoderName() + " took no input for " + deltaMs +
                    " ms, and " + recovery + " did not finish within " + timeoutMs + " ms");
            if (reason != null) {
                r.telemetry.reportDecoderFailure(reason, new MediaCodecDecoderRenderer.DecoderHungException(deltaMs));
            }
        }, "Decoder reset watchdog");
        watchdog.setDaemon(true);
        watchdog.start();
        return true;
    }

    // Whether the decoder ran well since its last hang reset: CR_REFILL_FRAMES frames over CR_REFILL_MS,
    // the measure that refills the recovery attempts.
    private boolean ranWellSinceHangReset() {
        return NightjarPolicyJni.decoderRecoveryOp(1, false, false, false, 0,
                r.numFramesOut, 0, r.framesOutAtHangReset, SystemClock.uptimeMillis(), 0, uptimeAtHangResetMs) != 0;
    }

    void recordVendorKeyHang() {
        String decoder = r.session.lowLatencyKeysDecoder;
        MediaCodecDecoderRenderer.HungKeys hung = decoder == null ? null : r.hungKeys.get(decoder);
        long waitSinceMs = r.inputWaitSinceMs;
        if (!r.session.vendorKeyHangUnrecorded.get() || hung == null || hung.earlierStream || r.stopping || r.hangResetPending ||
                (waitSinceMs >= 0 && SystemClock.uptimeMillis() - waitSinceMs >= RECORD_INPUT_WAIT_MS) ||
                !ranWellSinceHangReset() || !r.session.vendorKeyHangUnrecorded.compareAndSet(true, false)) {
            return;
        }
        VendorKeyHangs.record(r.context, decoder, hung.keys);
        LimeLog.warning(r.telemetry.decoderName() + " ran well without the vendor low-latency keys it took no input with (" +
                MediaCodecTelemetry.describeKeys(hung.keys) + "); the next streams start it without them until an app or system update");
    }

    // The recovery attempts are refilled once the decoder put out CR_REFILL_FRAMES frames over at
    // least CR_REFILL_MS since the last recovery: only failures close together use them up.
    boolean recoveryAttemptsLeft() {
        int res = NightjarPolicyJni.decoderRecoveryOp(0, false, false, false, r.codecRecoveryAttempts,
                r.numFramesOut, framesOutAtLastRecovery, 0, SystemClock.uptimeMillis(), uptimeAtLastRecoveryMs, 0);
        if ((res & 2) != 0) {
            LimeLog.info("Codec recovery attempts refilled after " + (r.numFramesOut - framesOutAtLastRecovery) + " frames since the last");
            r.codecRecoveryAttempts = 0;
        }
        return (res & 1) != 0;
    }
}
