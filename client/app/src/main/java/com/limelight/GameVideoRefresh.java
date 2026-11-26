package com.limelight;

import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.DisplayModeChoice;
import com.limelight.binding.video.DisplayRefreshDiagnostics;
import com.limelight.binding.video.HdrFormatPolicy;
import com.limelight.binding.video.HdrPolicy;
import com.limelight.binding.video.HdrRecomposition;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.binding.video.RefreshSwitchDiagnosis;
import com.limelight.binding.video.RefreshSwitchMonitor;
import com.limelight.binding.video.Sdr10Policy;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.computers.HostNotices;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.CompositorHeartbeat;
import com.limelight.ui.StreamGlass;
import android.os.Build;
import android.os.Looper;
import android.os.SystemClock;
import android.view.Display;
import android.view.Surface;
import android.view.View;
import android.view.ViewGroup;
import android.view.ViewParent;
import android.view.Window;
import android.view.WindowManager;

/** Display refresh-rate selection, surface and content-rate votes, refresh-switch/ARR/HDR/Vulkan status polling, and Vulkan HDR recomposition. */
final class GameVideoRefresh {
    private final Game game;
    private final GameVideoOutput output;

    private final DisplayRefreshDiagnostics displayRefreshDiagnostics = new DisplayRefreshDiagnostics();
    // Follows the content rate measured from the host's frame times (checkbox_content_rate_frame_rate).
    private final DisplayFrameRatePolicy.ContentRateVote contentRateVote =
            new DisplayFrameRatePolicy.ContentRateVote();
    // Tells why the screen did not follow the vote (see RefreshSwitchDiagnosis); created once connected.
    private RefreshSwitchMonitor refreshSwitchMonitor;
    // Fallback recovery for pinned phone displays (e.g. OriginOS/ColorOS AMOLED LTPO) that drop
    // back to 60 Hz after UI views go idle (#168): stage 1 re-applies the pinned mode and frame
    // rate vote; stage 2 starts a UI compositor heartbeat if the display stays dropped or drops again.
    private int refreshRecoveryStage;
    private float refreshRecoveryVoteHz;
    private long refreshRecoveryAtMs;
    private boolean refreshRecoveredAfterReapply;
    CompositorHeartbeat refreshHeartbeat;
    // One GPU composition of the Vulkan renderer's HDR picture each time it starts or the stream comes back to the front,
    // as opening the stream menu gives it: some screens show it as SDR until then (HdrRecomposition).
    private final HdrRecomposition hdrRecomposition = new HdrRecomposition();
    private boolean hdrRecompositionUnavailableLogged;
    private Boolean sdr10LastHostHdr;
    private HdrPolicy.Status lastHdrStatus = HdrPolicy.Status.QUIET;

    GameVideoRefresh(Game game, GameVideoOutput output) {
        this.game = game;
        this.output = output;
    }

    float prepareDisplayForRendering() {
        Display display = output.getVideoDisplay();
        boolean television = output.isTelevision();
        float displayRefreshRate;
        boolean arrSupported = Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA && display.hasArrSupport();
        game.useArr = DisplayFrameRatePolicy.useAdaptiveRefreshRate(
                Build.VERSION.SDK_INT, game.prefConfig.vrr, arrSupported);
        if (game.prefConfig.vrr) {
            DisplayFrameRatePolicy.ArrState arr =
                    DisplayFrameRatePolicy.arrState(Build.VERSION.SDK_INT, true, arrSupported);
            LimeLog.info(game.useArr ? "VRR: ARR (Android automatic policy)" : "VRR: max Hz (ARR unavailable: " + arr + ")");
        }

        if (game.useArr && Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA) {
            // Leave refresh-rate selection and touch boost to Android's automatic ARR policy.
            DisplayFrameRatePolicy.applyWindowDisplayMode(output.getVideoWindow(), 0, 0f, true);
            displayRefreshRate = game.panelMaxRefreshRate;
        } else {
            Display.Mode currentMode = display.getMode();
            Display.Mode bestMode = currentMode;
            boolean isNativeResolutionStream = PreferenceConfiguration.isNativeResolution(game.prefConfig.width, game.prefConfig.height);
            LimeLog.info("Current display mode: " + bestMode.getPhysicalWidth() + "x" +
                    bestMode.getPhysicalHeight() + "x" + bestMode.getRefreshRate());
            Display.Mode[] supported = display.getSupportedModes();
            DisplayModeChoice.Mode[] candidates = new DisplayModeChoice.Mode[supported.length];
            for (int i = 0; i < supported.length; i++) {
                LimeLog.info("Examining display mode: " + supported[i].getPhysicalWidth() + "x" +
                        supported[i].getPhysicalHeight() + "x" + supported[i].getRefreshRate());
                candidates[i] = new DisplayModeChoice.Mode(
                        supported[i].getPhysicalWidth(), supported[i].getPhysicalHeight(), supported[i].getRefreshRate());
            }
            int chosen = DisplayModeChoice.choose(
                    new DisplayModeChoice.Mode(bestMode.getPhysicalWidth(),
                            bestMode.getPhysicalHeight(), bestMode.getRefreshRate()),
                    candidates, game.prefConfig.width, game.prefConfig.height, game.prefConfig.fps,
                    game.prefConfig.vrr, isNativeResolutionStream, output.mayReduceRefreshRate());
            if (chosen >= 0) bestMode = supported[chosen];
            LimeLog.info("Best display mode: " + bestMode.getPhysicalWidth() + "x" +
                    bestMode.getPhysicalHeight() + "x" + bestMode.getRefreshRate());
            DisplayFrameRatePolicy.configureWindowDisplayMode(
                    output.getVideoWindow(), currentMode, bestMode, game.prefConfig.useTextureView,
                    television, game.externalPresentation != null);
            displayRefreshRate = bestMode.getRefreshRate();
        }
        applyRequestedFrameRate(game.useArr ? View.REQUESTED_FRAME_RATE_CATEGORY_DEFAULT : displayRefreshRate);
        if (game.useArr) {
            stopRefreshHeartbeat();
            resetRefreshRecovery();
        }
        logDisplayRefreshPolicy(display, arrSupported);

        if (!game.prefConfig.useTextureView && game.prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF) {
            // Keep decoder buffers at stream resolution; the compositor scales them to the view.
            game.renderView.getHolder().setFixedSize(game.prefConfig.width, game.prefConfig.height);
        }
        output.applyVideoScaling();
        game.desiredRefreshRate = displayRefreshRate;
        return television ? displayRefreshRate : Math.min(output.getVideoDisplay().getRefreshRate(), displayRefreshRate);
    }

    void logDisplayRefreshPolicy(Display display) {
        logDisplayRefreshPolicy(display,
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA && display.hasArrSupport());
    }

    void logDisplayRefreshPolicy(Display display, boolean arrSupported) {
        WindowManager.LayoutParams attributes = output.getVideoWindow().getAttributes();
        String changed = displayRefreshDiagnostics.update(Build.VERSION.SDK_INT, display.getDisplayId(),
                display.getMode().getModeId(),
                arrSupported, game.prefConfig.vrr, game.useArr,
                attributes.preferredDisplayModeId,
                attributes.preferredRefreshRate, game.appSurfaceFrameRateVote);
        if (changed != null) LimeLog.info(changed);
    }

    void refreshSurfaceFrameRateVote() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && game.surfaceCreated && game.videoSurface != null &&
                game.videoSurface.isValid() && !game.prefConfig.useTextureView) {
            voteSurfaceFrameRate(game.videoSurface);
        }
    }

    void voteSurfaceFrameRate(Surface surface) {
        if (game.useArr && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // The stream rate, seamless only: ARR's steps are seamless, and a 0 vote would leave
            // the game at Android's 60 Hz default for games (see arrVote). arrHalfRate watches for it.
            int exactRate = game.decoderRenderer == null ? 0 : game.decoderRenderer.getStreamFrameRateMilliHz();
            float vote = DisplayFrameRatePolicy.arrVote(
                    exactRate > 0 ? exactRate / 1000f : game.prefConfig.fps, game.panelMaxRefreshRate);
            DisplayFrameRatePolicy.applyWindowDisplayMode(output.getVideoWindow(), 0, 0f, true);
            stopRefreshHeartbeat();
            resetRefreshRecovery();
            applyRequestedFrameRate(View.REQUESTED_FRAME_RATE_CATEGORY_DEFAULT);
            applySurfaceFrameRate(surface, vote, Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS);
            output.arrHalfRate.reset();
            LimeLog.info("VRR: ARR vote " + vote + " Hz (stream rate, seamless only)");
            logDisplayRefreshPolicy(output.getVideoDisplay());
            return;
        }

        applySurfaceFrameRate(surface, baseSurfaceFrameRate(), Surface.CHANGE_FRAME_RATE_ALWAYS);
        // The base vote replaced any content rate vote; make it again.
        contentRateVote.invalidate();
        logDisplayRefreshPolicy(output.getVideoDisplay());
    }

    // The vote for the stream before the content rate is known, or when it is not followed.
    private float baseSurfaceFrameRate() {
        float desiredFrameRate;

        // Android will pick the lowest matching refresh rate for a given frame rate value, so we want
        // to report the true FPS value if refresh rate reduction is enabled. We also report the true
        // FPS value if there's no suitable matching refresh rate. In that case, Android could try to
        // select a lower refresh rate that avoids uneven pull-down (ex: 30 Hz for a 60 FPS stream on
        // a display that maxes out at 50 Hz).
        if (game.prefConfig.vrr) {
            desiredFrameRate = game.panelMaxRefreshRate;
        }
        else if (output.mayReduceRefreshRate() || game.desiredRefreshRate < game.prefConfig.fps) {
            int exactRate = game.decoderRenderer == null ? 0 : game.decoderRenderer.getStreamFrameRateMilliHz();
            desiredFrameRate = exactRate > 0 ? exactRate / 1000f : game.prefConfig.fps;
        }
        else {
            // Otherwise, we will pretend that our frame rate matches the refresh rate we picked in
            // prepareDisplayForRendering(). This will usually be the highest refresh rate that our
            // frame rate evenly divides into, which ensures the lowest possible display latency.
            desiredFrameRate = game.desiredRefreshRate;
        }
        return desiredFrameRate;
    }

    private void applySurfaceFrameRate(Surface surface, float frameRate, int changeStrategy) {
        // Tell the OS about our frame rate to allow it to adapt the display refresh rate appropriately
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // The stream's own vote changes the rate even if it's not seamless, since on TVs and
            // external outputs prepareDisplayForRendering() will not set the display mode on S+ if
            // it only differs by the refresh rate. It depends on us to trigger the switch here.
            surface.setFrameRate(frameRate,
                    Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE,
                    changeStrategy);
            game.appSurfaceFrameRateVote = frameRate;
            output.appSurfaceSeamlessOnly = changeStrategy == Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS;
        }
        else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            // No strategy on API 30: the two-argument vote only ever switches seamlessly.
            surface.setFrameRate(frameRate,
                    Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE);
            game.appSurfaceFrameRateVote = frameRate;
            output.appSurfaceSeamlessOnly = true;
        }
        if (!game.useArr) {
            applyRequestedFrameRate(frameRate);
            syncWindowDisplayMode(frameRate, output.appSurfaceSeamlessOnly);
        }
    }

    private void applyRequestedFrameRate(float frameRate) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.VANILLA_ICE_CREAM) return;
        if (game.renderView != null) {
            game.renderView.setVideoRequestedFrameRate(frameRate);
        }
        Window window = output.getVideoWindow();
        View decor = window == null ? null : window.getDecorView();
        if (decor != null) {
            decor.setRequestedFrameRate(frameRate);
        }
        if (refreshHeartbeat != null) {
            refreshHeartbeat.setRequestedFrameRate(frameRate);
        }
    }

    void syncWindowDisplayMode(float targetHz, boolean seamlessOnly) {
        DisplayFrameRatePolicy.syncWindowDisplayMode(output.getVideoWindow(), output.getVideoDisplay(),
                targetHz, output.mayReduceRefreshRate(), seamlessOnly, game.useArr, game.prefConfig.useTextureView,
                output.isTelevision(), game.externalPresentation != null,
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && game.isInPictureInPictureMode());
    }

    void startRefreshSwitchPolling() {
        if (refreshSwitchMonitor == null) {
            refreshSwitchMonitor = new RefreshSwitchMonitor(game);
        }
        game.bitrateHandler.removeCallbacks(output.pollRefreshSwitch);
        game.bitrateHandler.postDelayed(output.pollRefreshSwitch, 1000);
    }

    void stopRefreshSwitchPolling() {
        game.bitrateHandler.removeCallbacks(output.pollRefreshSwitch);
        output.clearAutoCompositorKeepalive();
        stopRefreshHeartbeat();
        resetRefreshRecovery();
        if (refreshSwitchMonitor != null) refreshSwitchMonitor.poll(0, null, null, false);
        output.arrHalfRate.reset();
        game.messages.status("refresh", null);
        game.messages.status("arr", null);
        game.messages.status("hdr", null);
    }

    void pollRefreshSwitchTick() {
        checkRefreshSwitch();
        checkArrRate();
        checkHdrStatus();
        checkHdrRecomposition();
        checkVulkanPace();
        checkCompositorWatch();
    }

    // When the screen has stayed off the rate voted for, says why on the overlay's status line
    // and, on pinned non-ARR non-TV displays, attempts pin re-application and UI heartbeat recovery.
    private void checkRefreshSwitch() {
        if (refreshHeartbeat != null && !canKeepRefreshHeartbeat()) {
            stopRefreshHeartbeat();
        }
        if (refreshSwitchMonitor == null) return;
        long nowMs = SystemClock.uptimeMillis();
        Display display = output.getVideoDisplay();
        Float voteHz = game.surfaceCreated && !game.useArr ? game.appSurfaceFrameRateVote : null;
        String text = refreshSwitchMonitor.poll(nowMs, display, voteHz, output.appSurfaceSeamlessOnly);
        if (game.messages.status("refresh", text)) game.overlays.updatePerformanceOverlay();
        recoverPinnedRefresh(nowMs, display, voteHz, refreshSwitchMonitor.lastSettled());
    }

    boolean canKeepRefreshHeartbeat() {
        return game.connected && game.foreground && !game.isFinishing() && !game.isDestroyed()
                && !game.useArr
                && (game.app == null || game.app.getRole() != NvApp.Role.INPUT_ONLY)
                && (Build.VERSION.SDK_INT < Build.VERSION_CODES.O || !game.isInPictureInPictureMode());
    }

    private boolean canRecoverPinnedRefresh(RefreshSwitchDiagnosis.Result settled) {
        if (settled == null || output.appSurfaceSeamlessOnly || !canKeepRefreshHeartbeat()) return false;
        if (settled.activeHz >= settled.voteHz - 0.5f) return false;
        if (settled.cause != RefreshSwitchDiagnosis.Cause.UNEXPLAINED &&
                settled.cause != RefreshSwitchDiagnosis.Cause.NEEDS_MATCH_SETTING) {
            return false;
        }
        return DisplayFrameRatePolicy.syncsWindowDisplayModeWithVote(
                game.useArr, game.prefConfig.useTextureView, output.isTelevision(), game.externalPresentation != null);
    }

    private void recoverPinnedRefresh(long nowMs, Display display, Float voteHz, RefreshSwitchDiagnosis.Result settled) {
        if (voteHz == null || voteHz <= 0f) {
            stopRefreshHeartbeat();
            resetRefreshRecovery();
            return;
        }
        if (refreshRecoveryVoteHz > 0f && Math.abs(voteHz - refreshRecoveryVoteHz) > 0.01f) {
            stopRefreshHeartbeat();
            resetRefreshRecovery();
        }
        if (settled == null) {
            if (refreshRecoveryStage == 1 && display != null &&
                    RefreshSwitchDiagnosis.holds(voteHz, display.getMode().getRefreshRate())) {
                refreshRecoveredAfterReapply = true;
            }
            return;
        }
        if (!canRecoverPinnedRefresh(settled)) return;
        if (refreshRecoveryStage == 0) {
            reapplyPinnedDisplayMode(voteHz);
            refreshRecoveryStage = 1;
            refreshRecoveryVoteHz = voteHz;
            refreshRecoveryAtMs = nowMs;
            refreshRecoveredAfterReapply = false;
            LimeLog.info("Refresh drop recovery: re-applying pinned display mode for " + voteHz + " Hz");
        } else if ((refreshRecoveryStage == 1 &&
                (refreshRecoveredAfterReapply || nowMs - refreshRecoveryAtMs >= RefreshSwitchDiagnosis.Watcher.SETTLE_MS)) ||
                (refreshRecoveryStage == 2 && refreshHeartbeat == null)) {
            startRefreshHeartbeat(voteHz, nowMs);
        }
    }

    private void reapplyPinnedDisplayMode(float voteHz) {
        syncWindowDisplayMode(voteHz, false);
        Window window = output.getVideoWindow();
        if (window != null) {
            // Re-commit attributes even when preferredDisplayModeId is unchanged so the window manager
            // re-evaluates the pinned mode after an OEM idle refresh drop.
            window.setAttributes(window.getAttributes());
        }
        if (game.videoSurface != null && game.videoSurface.isValid()) {
            applySurfaceFrameRate(game.videoSurface, voteHz, Surface.CHANGE_FRAME_RATE_ALWAYS);
        } else {
            applyRequestedFrameRate(voteHz);
        }
    }

    private void startRefreshHeartbeat(float voteHz, long nowMs) {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            game.runOnUiThread(() -> startRefreshHeartbeat(voteHz, nowMs));
            return;
        }
        if (!canKeepRefreshHeartbeat()) return;
        ViewParent parent = game.renderView == null ? null : game.renderView.getParent();
        ViewGroup holder = parent instanceof ViewGroup ? (ViewGroup) parent : null;
        if (holder == null) return;
        if (refreshHeartbeat != null && refreshHeartbeat.holder() != holder) {
            refreshHeartbeat.stop();
            refreshHeartbeat = null;
        }
        if (refreshHeartbeat == null) {
            output.updateCompositorHeartbeat();
            refreshHeartbeat = new CompositorHeartbeat(holder);
            refreshHeartbeat.setRequestedFrameRate(voteHz);
            refreshHeartbeat.start();
            output.updateCompositorHeartbeat();
            LimeLog.info("Refresh drop recovery: starting UI compositor heartbeat to hold " + voteHz + " Hz");
        }
        refreshRecoveryStage = 2;
        refreshRecoveryAtMs = nowMs;
        reapplyPinnedDisplayMode(voteHz);
    }

    void stopRefreshHeartbeat() {
        if (refreshHeartbeat == null) return;
        if (Looper.myLooper() != Looper.getMainLooper()) {
            game.runOnUiThread(this::stopRefreshHeartbeat);
            return;
        }
        refreshHeartbeat.stop();
        refreshHeartbeat = null;
        output.updateCompositorHeartbeat();
    }

    private void resetRefreshRecovery() {
        refreshRecoveryStage = 0;
        refreshRecoveryVoteHz = 0f;
        refreshRecoveryAtMs = 0L;
        refreshRecoveredAfterReapply = false;
    }

    // Under ARR the refresh switch check above cannot tell what the screen runs at, so watch what it
    // shows: about half the rendered frames for 3 s, with the display at about half the stream rate,
    // means Android holds the stream at half its rate.
    private void checkArrRate() {
        if (!game.useArr || game.decoderRenderer == null) return;
        float[] rates = game.decoderRenderer.getFrameRates();
        float rendered = PerformanceOverlay.renderedFps(rates);
        float presented = PerformanceOverlay.presentedFps(rates);
        Display display = output.getVideoDisplay();
        float displayHz = display == null ? 0 : display.getRefreshRate();
        int exactRate = game.decoderRenderer.getStreamFrameRateMilliHz();
        float streamHz = exactRate > 0 ? exactRate / 1000f : game.prefConfig.fps;
        boolean was = output.arrHalfRate.flagged();
        boolean flagged = output.arrHalfRate.update(SystemClock.uptimeMillis(), rendered, presented, displayHz, streamHz);
        if (flagged && !was) {
            LimeLog.warning("VRR: ARR shows " + presented + " of " + rendered + " fps rendered for " +
                    DisplayFrameRatePolicy.ArrHalfRateCheck.HOLD_MS / 1000 +
                    " s, display at " + displayHz + " Hz: the screen is held at half the stream rate (vote " +
                    game.appSurfaceFrameRateVote + " Hz)");
        } else if (!flagged && was) {
            LimeLog.info("VRR: ARR shows the rendered frames again (" + presented + " of " + rendered + " fps)");
        }
        String text = flagged ? game.getString(R.string.stream_vrr_arr_half_rate, Math.round(presented), Math.round(rendered)) : null;
        if (game.messages.status("arr", text)) game.overlays.updatePerformanceOverlay();
    }

    // Render through Vulkan can't keep up on this display: say so once per PC and on the status line.
    // The renderer stays on and the setting stays; turning it off is the user's choice.
    private void checkVulkanPace() {
        if (game.decoderRenderer != null && game.decoderRenderer.takeVulkanBehind()) {
            game.overlays.hostNotice(HostNotices.Notice.VULKAN_BEHIND, R.string.stream_vulkan_behind);
        }
    }

    private void checkCompositorWatch() {
        if (game.decoderRenderer == null || !game.connected || game.isDestroyed()
                || (game.app != null && game.app.getRole() == NvApp.Role.INPUT_ONLY)) {
            return;
        }
        android.view.SurfaceView surfaceView = game.renderView == null ? null : game.renderView.getSurfaceView();
        boolean hasVideoView = surfaceView != null
                || (game.renderView != null && game.renderView.getTextureView() != null);
        if (!game.foreground || game.isInPictureInPictureMode() || (hasVideoView && !game.surfaceCreated)) {
            game.decoderRenderer.resetCompositorWatchWindow();
            if (!game.decoderRenderer.isCompositorKeepaliveLockedOn()) {
                output.clearAutoCompositorKeepalive();
            }
            return;
        }
        boolean overlayVisible = (game.overlays != null && game.overlays.isPerformanceOverlayVisible())
                || refreshHeartbeat != null;
        boolean manualOn = game.prefConfig != null && game.prefConfig.tvDisplayCompatibility;
        int action = game.decoderRenderer.pollCompositorKeepaliveAction(overlayVisible, manualOn);
        String event;
        while ((event = game.decoderRenderer.takeCompositorWatchEvent()) != null) {
            LimeLog.info(event);
        }
        if (action == com.limelight.binding.video.CompositorWatch.ACTION_PULSE) {
            output.pulseCompositorKeepalive(500L);
        } else if (action == com.limelight.binding.video.CompositorWatch.ACTION_KEEP_ON) {
            output.setAutoCompositorKeepalive(true);
        } else if (action == com.limelight.binding.video.CompositorWatch.ACTION_STOP) {
            output.setAutoCompositorKeepalive(false);
        }
    }

    /** The stream window is in front again (onResume). */
    void armHdrRecomposition() {
        hdrRecomposition.arm();
    }

    private void checkHdrRecomposition() {
        if (game.decoderRenderer == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return;
        boolean inFront = game.foreground && game.hasWindowFocus() && game.streamMenu == null &&
                game.externalPresentation == null && !game.isInPictureInPictureMode();
        if (!hdrRecomposition.take(game.decoderRenderer.isVulkanHdrOutput(), inFront)) return;
        if (StreamGlass.recompose(game, HdrRecomposition.DURATION_MS)) {
            LimeLog.info("Vulkan HDR output: one GPU composition asked of the system");
        } else if (!hdrRecompositionUnavailableLogged) {
            hdrRecompositionUnavailableLogged = true;
            LimeLog.info("Vulkan HDR output: no GPU composition asked, the system's window blur is off");
        }
    }

    // HDR was granted and then not delivered: say so on the status line for as long as it lasts,
    // and log it. A refusal before the stream is already noted by refuseHdr, not repeated here.
    void checkHdrStatus() {
        String text = null;
        if (game.streamHdr && game.decoderRenderer != null) {
            int format = game.decoderRenderer.getActiveVideoFormat();
            HdrPolicy.Status status = format == 0 ? HdrPolicy.Status.WAITING : HdrPolicy.drawnStatus(HdrPolicy.status(true,
                    (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0, game.decoderRenderer.getHostHdrReport(),
                    game.decoderRenderer.getHdrMetadataState(), game.decoderRenderer.getDecodedTransfer(),
                    game.decoderRenderer.getStreamTransfer(), HdrPolicy.hlgOnly(HdrPolicy.displayHdrTypes(output.getVideoDisplay()))),
                    game.decoderRenderer.isVulkanHdrToneMapped());
            int res = Game.hdrStatusText(status, game.decoderRenderer.isPyroWaveHlgNeedingHostUpdate(),
                    game.decoderRenderer.isPyroWaveHlgRefusedByDisplay());
            if (status == HdrPolicy.Status.SHOWN_AS_SDR) {
                text = game.getString(res, HdrFormatPolicy.baseLayerName(game.decoderRenderer.getAnnouncedHdrFormat()));
            } else if (res != 0) {
                text = game.getString(res);
            }
            // The window took HLG before the stream but does not show it as HLG (the renderer and
            // MediaCodecDecoderRenderer log why).
            if (game.decoderRenderer.isPyroWaveHlgLost()) text = game.getString(R.string.stream_hdr_pyrowave_hlg_lost);
            // Plain words, once per PC, for what the Vulkan renderer cannot show of this screen's HDR.
            boolean forFrameWarp = game.decoderRenderer.vulkanForFrameWarp();
            int limit = Game.vulkanHdrLimitText(game.decoderRenderer.takeVulkanHdrLimit(), forFrameWarp);
            if (limit != 0) {
                game.overlays.hostNotice(forFrameWarp ? HostNotices.Notice.FRAME_WARP_HDR_LIMIT :
                        HostNotices.Notice.VULKAN_HDR_LIMIT, limit);
            }
            // Once per stream on the status line (and once per PC as a message): the output window
            // refused HDR10+, so the picture keeps HDR10's static metadata alone.
            if (game.decoderRenderer.takeVulkanHdr10PlusRefused()) {
                LimeLog.warning("HDR10+ refused by the Vulkan renderer's window");
                game.overlays.hostNotice(HostNotices.Notice.VULKAN_HDR10_PLUS_REFUSED, R.string.stream_hdr_vulkan_no_hdr10_plus);
            }
            // And for what PyroWave cannot carry; Dolby Vision names what the stream is instead.
            int pyroWaveLimit = Game.pyroWaveHdrLimitText(game.decoderRenderer.takePyroWaveHdrLimit());
            if (pyroWaveLimit != 0) {
                game.messages.notice(game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), HostNotices.Notice.PYROWAVE_HDR_LIMIT,
                        game.getString(pyroWaveLimit, HdrFormatPolicy.baseLayerName(game.decoderRenderer.getAnnouncedHdrFormat())));
            }
            if (status != lastHdrStatus) {
                lastHdrStatus = status;
                if (res != 0) LimeLog.warning("HDR requested but " + status);
            }
        }
        if (game.streamSdr10 && game.decoderRenderer != null) {
            int format = game.decoderRenderer.getActiveVideoFormat();
            boolean tenBit = (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
            Boolean report = game.decoderRenderer.getHostHdrReport();
            if (Sdr10Policy.status(tenBit, report) == Sdr10Policy.Status.HOST_HDR) {
                text = game.getString(R.string.stream_sdr10_host_hdr);
            }
            Sdr10Policy.HostHdrStep step = Sdr10Policy.hostHdrStep(tenBit, report, sdr10LastHostHdr);
            if (step == Sdr10Policy.HostHdrStep.SAY) {
                LimeLog.warning("10-bit SDR requested but the PC sends HDR");
                game.overlays.hostNotice(HostNotices.Notice.SDR10_HOST_HDR, R.string.stream_sdr10_host_hdr);
            } else if (step == Sdr10Policy.HostHdrStep.REARM) {
                // The PC's display is in SDR now; if it goes to HDR again, say so again.
                HostNotices.reset(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), HostNotices.Notice.SDR10_HOST_HDR);
            }
            if (tenBit && report != null) sdr10LastHostHdr = report;
        }
        if (game.messages.status("hdr", text)) game.overlays.updatePerformanceOverlay();
    }

    private boolean followsContentRate() {
        return DisplayFrameRatePolicy.followsContentRate(Build.VERSION.SDK_INT,
                game.prefConfig.contentRateFrameRate,
                MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_CLOCK_SYNC),
                game.useArr, game.prefConfig.vrr, game.prefConfig.useTextureView);
    }

    // Moves the surface's frame rate vote to the rate the game presents at once that held steady,
    // seamless modes only, so a vote never blanks the screen during the stream.
    void voteContentRate() {
        if (!game.surfaceCreated || game.videoSurface == null || !game.videoSurface.isValid() || game.decoderRenderer == null ||
                !followsContentRate()) {
            return;
        }
        long now = SystemClock.uptimeMillis();
        int contentMilliHz = game.decoderRenderer.getContentRateMilliHz();
        if (contentRateVote.withdraw(now, contentMilliHz)) {
            // The game no longer holds a rate: back to the vote the stream would make.
            if (keepsVrrVote(baseSurfaceFrameRate())) return;
            applySurfaceFrameRate(game.videoSurface, baseSurfaceFrameRate(), Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS);
            LimeLog.info("Content frame rate vote withdrawn");
            logDisplayRefreshPolicy(output.getVideoDisplay());
            return;
        }
        float contentHz = contentRateVote.next(now, contentMilliHz);
        if (contentHz <= 0) return;
        // With VRR (no ARR here) the base vote is the panel maximum. With a lower refresh rate allowed
        // the vote is as without VRR. Otherwise it goes lower only when the game's rate would judder at
        // the maximum, to a seamless multiple of it: even frame timing for a little more wait for the
        // next refresh (about 1.4 ms on average for 90 Hz instead of 120, not measured).
        float vote = game.prefConfig.vrr ?
                DisplayFrameRatePolicy.vrrContentVote(contentHz, game.panelMaxRefreshRate,
                        output.mayReduceRefreshRate(),
                        DisplayFrameRatePolicy.seamlessRates(output.getVideoDisplay())) :
                DisplayFrameRatePolicy.contentVote(contentHz,
                        baseSurfaceFrameRate(), output.mayReduceRefreshRate(), game.panelMaxRefreshRate);
        if (keepsVrrVote(vote)) return;
        applySurfaceFrameRate(game.videoSurface, vote, Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS);
        LimeLog.info("Content frame rate vote: " + vote + " Hz (game at " + contentHz + " Hz)");
        logDisplayRefreshPolicy(output.getVideoDisplay());
    }

    // With VRR a content vote that equals the vote in force (the panel maximum the stream started
    // with) is not made again: re-voting it seamless-only would change the reason the status line
    // gives when the screen never reached that rate (a TV that needs Match content frame rate).
    private boolean keepsVrrVote(float vote) {
        return game.prefConfig.vrr && game.appSurfaceFrameRateVote != null && Math.abs(vote - game.appSurfaceFrameRateVote) < 0.01f;
    }
}
