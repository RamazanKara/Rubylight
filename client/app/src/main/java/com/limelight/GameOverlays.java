package com.limelight;

import com.limelight.binding.audio.MicrophoneStatus;
import com.limelight.binding.video.HdrPolicy;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.computers.HostNotices;
import com.limelight.nvstream.AdaptiveQualityController;
import com.limelight.nvstream.http.HardwareMetrics;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import android.content.Context;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.graphics.Rect;
import android.os.Build;
import android.os.SystemClock;
import android.preference.PreferenceManager;
import android.view.Display;
import android.view.View;
import android.widget.Toast;

/** The stream's on-screen overlays: the performance overlay and its hardware metrics, the stream menu hint and the notices of the stream. */
final class GameOverlays {
    private final Game game;

    GameOverlays(Game game) {
        this.game = game;
    }

    private CharSequence debugPerformanceText = "";
    private CharSequence statisticsPerformanceText = "";
    private CharSequence compactPerformanceText = "";

    private final Runnable expireStreamMenuHint = () -> dismissStreamMenuHint(true);
    private HardwareMetrics hardwareMetrics;
    private boolean hardwareMetricsPolling;
    private int hardwareMetricsGeneration;
    private long hardwareMetricsSampleTimestamp = Long.MIN_VALUE;
    private long hardwareMetricsFreshUntil;
    private final Runnable expireHardwareMetrics = () -> {
        hardwareMetrics = null;
        updatePerformanceOverlay();
    };

    void showPerformanceOverlay(String mode) {
        setPerformanceOverlayMode(mode);
        if (!game.prefConfig.enablePerfOverlay) togglePerformanceOverlay();
    }

    // Off from the menu: the same switch the settings' Off writes, the mode kept for the next time.
    void hidePerformanceOverlay() {
        if (game.prefConfig.enablePerfOverlay) togglePerformanceOverlay();
    }

    // Once, on the first stream: how to open the stream menu with this device's own inputs.
    void showStreamMenuHint() {
        if (!game.foreground || !game.connected || game.isFinishing() || game.streamMenu != null || game.streamMenuHint == null ||
                game.isHidingOverlays || com.limelight.ui.StreamMenuHint.seen(game)) {
            return;
        }
        boolean controller = com.limelight.ui.StreamMenuHint.hasGamepad() ||
                (game.controllerHandler != null && game.controllerHandler.hasUsbControllers());
        String text = com.limelight.ui.StreamMenuHint.text(game, controller);
        game.streamMenuHint.setText(text);
        game.streamMenuHint.setContentDescription(text + " " + game.getString(R.string.stream_menu_hint_close));
        game.streamMenuHint.setOnClickListener(view -> dismissStreamMenuHint(true));
        game.streamMenuHint.setVisibility(View.VISIBLE);
        com.limelight.ui.StreamMenuHint.place(game.streamMenuHint, game.performanceOverlayScroll);
        game.bitrateHandler.removeCallbacks(expireStreamMenuHint);
        game.bitrateHandler.postDelayed(expireStreamMenuHint, com.limelight.ui.StreamMenuHint.SHOW_MS);
    }

    void dismissStreamMenuHint(boolean read) {
        game.bitrateHandler.removeCallbacks(game.showStreamMenuHint);
        game.bitrateHandler.removeCallbacks(expireStreamMenuHint);
        if (game.streamMenuHint != null) game.streamMenuHint.setVisibility(View.GONE);
        if (read) com.limelight.ui.StreamMenuHint.markSeen(game);
    }

    /**
     * Says which profile and own values this stream started with, and loudly what could not be
     * used. A profile picked for the network is announced; the overlay always lists the layers.
     */
    void showStreamLayers(PreferenceConfiguration config, boolean firstStart) {
        com.limelight.preferences.StreamLayers.Result layers = config.layers;
        if (layers == null) return;
        for (String problem : com.limelight.preferences.StreamLayersText.problems(game, layers)) {
            game.messages.alert(problem, Toast.LENGTH_LONG);
        }
        config.layersText = com.limelight.preferences.StreamLayersText.applied(game, layers);
        if (config.layersText == null) return;
        if (firstStart && layers.reason == com.limelight.preferences.StreamLayers.Reason.NETWORK) {
            game.messages.countedNotice(config.layersText);
        } else {
            game.messages.note(config.layersText);
        }
    }

    void togglePerformanceOverlay() {
        if (game.app.getRole() == NvApp.Role.INPUT_ONLY) {
            return;
        }
        game.prefConfig.enablePerfOverlay = !game.prefConfig.enablePerfOverlay;
        game.performanceOverlayScroll.setVisibility(game.prefConfig.enablePerfOverlay && !game.isHidingOverlays ? View.VISIBLE : View.GONE);
        PreferenceManager.getDefaultSharedPreferences(game).edit()
                .putBoolean("checkbox_enable_perf_overlay", game.prefConfig.enablePerfOverlay).apply();
        updateHardwareMetricsPolling();
    }

    boolean isDebugOverlay() {
        return PerformanceOverlay.MODE_DEBUG.equals(game.overlayMode);
    }

    private void setPerformanceOverlayMode(String mode) {
        game.overlayMode = PerformanceOverlay.normalizeMode(mode);
        PreferenceManager.getDefaultSharedPreferences(game).edit()
                .putString("performance_overlay_mode", game.overlayMode).apply();
        game.performanceOverlayScroll.scrollTo(0, 0);
        updatePerformanceOverlay();
        // The compact line leaves the host's hardware readings out; the other modes poll them.
        updateHardwareMetricsPolling();
    }

    void setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner corner) {
        game.overlayCorner = corner;
        game.prefConfig.overlayPosition = corner.key;
        PreferenceManager.getDefaultSharedPreferences(game).edit()
                .putString("performance_overlay_position", corner.key).apply();
        // Move it now; a hidden overlay is placed by the next layout once it shows.
        android.view.ViewGroup parent = (android.view.ViewGroup) game.performanceOverlayScroll.getParent();
        int margin = Math.round(8 * game.getResources().getDisplayMetrics().density);
        com.limelight.ui.OverlayPlacement.Spot spot = com.limelight.ui.OverlayPlacement.place(
                game.performanceOverlayScroll, parent, this::collectOverlayObstacles, margin, corner);
        if (spot != null) alignOverlayText(spot);
    }

    void attachOverlayPlacement(android.view.ViewGroup parent) {
        com.limelight.ui.OverlayPlacement.attach(game.performanceOverlayScroll, parent,
                this::collectOverlayObstacles, () -> game.overlayCorner, this::alignOverlayText);
    }

    // Lines of different widths hug the side the overlay sits on. Set only when the side changes.
    private void alignOverlayText(com.limelight.ui.OverlayPlacement.Spot spot) {
        int side = spot == com.limelight.ui.OverlayPlacement.Spot.TOP_END ||
                spot == com.limelight.ui.OverlayPlacement.Spot.BOTTOM_END ? android.view.Gravity.END : android.view.Gravity.START;
        if ((game.performanceOverlayView.getGravity() & android.view.Gravity.RELATIVE_HORIZONTAL_GRAVITY_MASK) != side) {
            game.performanceOverlayView.setGravity(android.view.Gravity.TOP | side);
        }
    }

    void updatePerformanceOverlay() {
        boolean compact = PerformanceOverlay.MODE_COMPACT.equals(game.overlayMode);
        game.performanceOverlayView.setTextSize(compact ? 13 : isDebugOverlay() ? 11 : 12);
        CharSequence text = compact ? compactPerformanceText : hardwarePerformanceText(isDebugOverlay());
        if (PerformanceOverlay.MODE_STATISTICS.equals(game.overlayMode) && text.length() != 0) {
            text = PerformanceOverlay.withStatusLine(text, game.messages.notes());
        }
        if (text.length() == 0) {
            text = compact ? "● " + game.getString(R.string.overlay_compact_waiting) : game.getString(R.string.stats_waiting);
        }
        text = PerformanceOverlay.withLinkNotice(game, text, game.linkNotice);
        game.performanceOverlayView.setText(text);
        game.performanceOverlayView.setContentDescription(compact ?
                PerformanceOverlay.describeCompact(game, compactPerformanceText, game.linkNotice) : null);
    }

    // The on-screen gamepad, its layout button, per-game buttons and the key toolbar stay uncovered.
    private void collectOverlayObstacles(java.util.List<Rect> out) {
        View parent = (View) game.performanceOverlayScroll.getParent();
        if (game.virtualController != null && game.prefConfig.onscreenController) game.virtualController.collectVisibleBounds(out);
        if (game.modernController != null && game.prefConfig.onscreenController) game.modernController.collectVisibleBounds(out);
        if (game.micButton != null) com.limelight.ui.OverlayPlacement.addVisible(game.micButton, parent, out);
        if (game.gameTouchControls != null) game.gameTouchControls.collectVisibleBounds(parent, out);
        if (game.keyboardInput.isKeyboardOverlayVisible()) com.limelight.ui.OverlayPlacement.addVisible(game.keyboardOverlay, parent, out);
        if (game.keyboardInput.isPcKeyboardVisible()) com.limelight.ui.OverlayPlacement.addVisible(game.pcKeyboard, parent, out);
        // The poor-connection warning sits top right: the overlay keeps out of its way.
        com.limelight.ui.OverlayPlacement.addVisible(game.notificationOverlayView, parent, out);
    }

    private CharSequence hardwarePerformanceText(boolean debug) {
        HardwareMetrics fresh = SystemClock.uptimeMillis() < hardwareMetricsFreshUntil ? hardwareMetrics : null;
        return PerformanceOverlay.withHardwareMetrics(game, debug ? debugPerformanceText : statisticsPerformanceText, fresh, debug);
    }

    void updateHardwareMetricsPolling() {
        if (!game.foreground || !game.connected || game.conn == null || game.isFinishing() || game.isDestroyed() || game.isHidingOverlays ||
                game.performanceOverlayScroll == null || game.performanceOverlayScroll.getVisibility() != View.VISIBLE ||
                PerformanceOverlay.MODE_COMPACT.equals(game.overlayMode)) {
            stopHardwareMetricsPolling();
            return;
        }
        if (hardwareMetricsPolling) return;
        int generation = ++hardwareMetricsGeneration;
        hardwareMetricsPolling = game.conn.startHardwareMetricsPolling(metrics -> game.bitrateHandler.post(() ->
                receiveHardwareMetrics(generation, metrics)));
    }

    private void receiveHardwareMetrics(int generation, HardwareMetrics metrics) {
        if (!hardwareMetricsPolling || generation != hardwareMetricsGeneration) return;
        long now = SystemClock.uptimeMillis();
        if (metrics != null) {
            if (metrics.sampledAtUnixMs != hardwareMetricsSampleTimestamp) {
                hardwareMetricsSampleTimestamp = metrics.sampledAtUnixMs;
                hardwareMetricsFreshUntil = now + Math.max(5000, metrics.intervalMs * 2L);
            }
            if (now >= hardwareMetricsFreshUntil) metrics = null;
        }
        hardwareMetrics = metrics;
        game.bitrateHandler.removeCallbacks(expireHardwareMetrics);
        if (metrics != null) game.bitrateHandler.postAtTime(expireHardwareMetrics, hardwareMetricsFreshUntil);
        updatePerformanceOverlay();
    }

    void stopHardwareMetricsPolling() {
        hardwareMetricsGeneration++;
        if (hardwareMetricsPolling && game.conn != null) game.conn.stopHardwareMetricsPolling();
        hardwareMetricsPolling = false;
        game.bitrateHandler.removeCallbacks(expireHardwareMetrics);
        if (hardwareMetrics != null) {
            hardwareMetrics = null;
            updatePerformanceOverlay();
        }
    }

    void copyPerformanceStats() {
        CharSequence text = hardwarePerformanceText(true);
        if (text.length() == 0) {
            game.messages.alert(R.string.overlay_empty, Toast.LENGTH_LONG);
            return;
        }
        ClipboardManager clipboard = (ClipboardManager) game.getSystemService(Context.CLIPBOARD_SERVICE);
        clipboard.setPrimaryClip(ClipData.newPlainText(game.getString(R.string.overlay_copy),
                text));
        game.messages.alert(R.string.overlay_copied, Toast.LENGTH_SHORT);
    }

    // The reason HDR was left off although the setting is on: the status line each stream
    // and a message once per PC (notice does both).
    void refuseHdr(HdrPolicy.Refusal refusal) {
        int text;
        switch (refusal) {
            case ANDROID_TOO_OLD: text = R.string.stream_hdr_android_unsupported; break;
            case MODE_WITHOUT_HDR10: text = R.string.stream_hdr_mode_unsupported; break;
            case DECODER_WITHOUT_HDR10: text = R.string.stream_hdr_decoder_unsupported; break;
            case DISPLAY_WITHOUT_HDR10: text = R.string.stream_hdr_display_unsupported; break;
            case HLG_NEEDS_DIRECT_OUTPUT: text = R.string.stream_hdr_hlg_needs_direct_output; break;
            default: return;
        }
        LimeLog.warning("HDR requested but not used: " + refusal);
        hostNotice(HostNotices.Notice.HDR_UNAVAILABLE, text);
    }

    void hostNotice(HostNotices.Notice notice, int text) {
        game.messages.notice(game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), notice, game.getString(text));
    }

    // PyroWave was chosen and the stream runs another codec: say once for the PC why, and which.
    void notePyroWaveFallback() {
        if (game.decoderRenderer == null) return;
        boolean chosen = game.prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE;
        int format = game.decoderRenderer.getActiveVideoFormat();
        String pc = game.getIntent().getStringExtra(Game.EXTRA_PC_UUID);
        if (!PyroWaveFallback.fellBack(chosen, format)) {
            if (chosen && format != 0) HostNotices.reset(game, pc, HostNotices.Notice.PYROWAVE_FALLBACK);
            return;
        }
        String codec = PyroWaveFallback.codecName(format);
        int reason = game.decoderRenderer.getPyroWaveDropReason();
        boolean hdrLossSaid = (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0 &&
                game.decoderRenderer.pyroWaveHdrFailureReason() != 0;
        PyroWaveFallback.Notice notice = PyroWaveFallback.notice(reason, hdrLossSaid);
        String text;
        switch (notice) {
            case PC: text = game.getString(R.string.host_pyrowave_unavailable, codec); break;
            case PC_WITHOUT_HDR: text = game.getString(R.string.stream_pyrowave_no_hdr_on_pc, codec); break;
            case FULL_CHROMA:
                text = game.getString(R.string.stream_pyrowave_full_chroma, codec, game.getString(R.string.title_packed_444));
                break;
            case DEVICE: text = game.getString(R.string.stream_pyrowave_device_unavailable, codec, game.getString(reason)); break;
            default: text = null;
        }
        LimeLog.warning("PyroWave was chosen, but the stream uses " + codec + ": " + (text != null ? text :
                hdrLossSaid ? "PyroWave HDR failed on this device (announced)" : "no known reason"));
        if (text != null) game.messages.notice(pc, HostNotices.Notice.PYROWAVE_FALLBACK, text);
    }

    void onPerfUpdate(final String video, final String network, final String decode, final CharSequence statisticsText,
                      final CharSequence compactText) {
        game.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (!game.connected || game.isFinishing() || game.isDestroyed()) {
                    return;
                }
                String displayLine = displayRefreshOverlayLine();
                String networkText = !game.adaptation.isAdaptiveBitrateEnabled() ? network : network + "\n" +
                        game.getString(R.string.stream_auto_bitrate_status, game.currentBitrate / 1000.0);
                if (game.adaptiveQuality != null) {
                    AdaptiveQualityController.Target quality = game.adaptiveQuality.current();
                    networkText += "\n" + game.getString(R.string.quality_status,
                            game.getResources().getStringArray(R.array.adaptive_quality_names)[game.adaptation.qualityMode().ordinal()],
                            quality.width, quality.height, quality.fps);
                } else if (game.qualityStatus != null) {
                    networkText += "\n" + game.qualityStatus;
                }
                MicrophoneStatus micStatus = game.conn.getMicrophoneStatus();
                if (micStatus.label != 0) {
                    networkText += "\n" + game.getString(micStatus.label);
                }
                for (String note : game.messages.notes()) {
                    if (!note.equals(game.qualityStatus)) networkText += "\n" + note;
                }
                networkText += "\n" + game.menuActions.wifiText(game.streamHealth.stallPeriodMs());
                debugPerformanceText = PerformanceOverlay.debugText(game, video + displayLine, networkText, decode);
                statisticsPerformanceText = statisticsText;
                compactPerformanceText = compactText;
                updatePerformanceOverlay();
            }
        });
    }

    private String displayRefreshOverlayLine() {
        if (!game.prefConfig.vrr) return "";
        Display display = game.getVideoDisplay();
        float modeRefreshRate = Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ?
                display.getMode().getRefreshRate() : display.getRefreshRate();
        return "\n" + game.getString(R.string.perf_overlay_vrr, modeRefreshRate, display.getRefreshRate(),
                game.useArr ? "ARR" : game.getString(R.string.perf_overlay_vrr_max_hz));
    }
}
