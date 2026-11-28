package com.limelight;

import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.StreamResizePolicy;
import com.limelight.nvstream.AdaptiveQualityController;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamOrientation;
import android.annotation.TargetApi;
import android.graphics.Point;
import android.os.Build;
import android.os.SystemClock;
import android.view.Display;
import android.view.View;
import android.widget.Toast;

/** Mid-stream reconfiguration: decoder rebuild action queue, offered stream geometry, display-mode and pane-size tracking. */
final class GameVideoReconfigure {
    private final Game game;
    private final GameVideoOutput output;

    private boolean streamStartedLandscape = true;
    private int lastPaneWidth, lastPaneHeight;

    GameVideoReconfigure(Game game, GameVideoOutput output) {
        this.game = game;
        this.output = output;
    }

    // Mid-stream reconfigure. A Rubylight 2.2 host that lists control message 0x5532 changes the
    // stream's size or frame rate in place and answers with an IDR. A native-resolution stream asks
    // for the panel's size when a foldable opens or closes, computed like the stream's start size,
    // and for its window's size in split screen or a desktop window. A stream at the panel's top
    // refresh rate follows a panel whose top rate changes. Fixed presets, PyroWave and decoders
    // without adaptive playback never ask.
    void scheduleReconfigureCheck() {
        scheduleReconfigureCheck(StreamReconfigureRequester.SETTLE_MS);
    }

    void scheduleReconfigureCheck(long settleMs) {
        if (!game.connected || game.isFinishing()) {
            return;
        }
        game.bitrateHandler.removeCallbacks(game.checkReconfigure);
        game.bitrateHandler.postDelayed(game.checkReconfigure, game.reconfigureRequester.onChange(SystemClock.uptimeMillis(), settleMs));
    }

    // Rotate on a host that cannot turn this stream: say once that the picture is scaled instead.
    void noteRotationWithoutReconfigure() {
        if (game.streamOrientation != StreamOrientation.ROTATE || output.orientationNoticeShown || !game.connected ||
                game.externalPresentation != null || game.app == null || game.app.getRole() == com.limelight.nvstream.http.NvApp.Role.INPUT_ONLY ||
                (game.hostShapesStream && MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_RECONFIGURE))) {
            return;
        }
        output.orientationNoticeShown = true;
        game.messages.alert(R.string.stream_orientation_scaled, Toast.LENGTH_LONG);
    }

    void startReconfigureTracking() {
        output.displaySignature = displaySignature();
        if (game.decoderRenderer == null) {
            return;
        }
        listenForRebuilds();
        whenDecoderReady("track", this::trackReconfigure);
    }

    // A rebuild's decoder is configured (told on the decoder thread): what waited for it runs on this
    // one, in order, then the usual check, asked for once.
    void listenForRebuilds() {
        game.decoderRenderer.setReconfigureLimitsListener(() -> game.bitrateHandler.post(() -> {
            game.bitrateHandler.removeCallbacks(game.checkReconfigure);
            output.decoderActions.put("check", this::checkStreamReconfigure);
            // The new decoder may answer differently: what a menu opened during the next rebuild shows.
            output.decoderActions.put("answer", game.adaptation::supportsAdaptiveQuality);
            runWaitingDecoderActions();
        }));
    }

    // A UI action that reads the decoder's limits or asks for a size, on the main looper: it runs now,
    // or, while a rebuild sets the decoder up again, once that is done, after those asked for before it.
    // Never nested: code inside an action calls the plain methods.
    void whenDecoderReady(String key, Runnable action) {
        if (game.decoderRenderer == null) {
            action.run();
            return;
        }
        if (runWaitingDecoderActions() && game.decoderRenderer.runUnlessRebuilding(action)) return;
        // Once the stream stopped nothing is kept for later.
        if (game.connected && !game.isFinishing()) output.decoderActions.put(key, action);
    }

    void dropDecoderAction(String key) {
        output.decoderActions.remove(key);
    }

    // The stream stopped: nothing that waited runs.
    void clearDecoderActions() {
        output.decoderActions.clear();
    }

    // True when nothing waits any more; once the stream stopped, nothing that waited runs.
    private boolean runWaitingDecoderActions() {
        while (!output.decoderActions.isEmpty()) {
            if (!game.connected || game.isFinishing() || game.decoderRenderer == null) {
                output.decoderActions.clear();
                return true;
            }
            java.util.Map.Entry<String, Runnable> first = output.decoderActions.entrySet().iterator().next();
            String key = first.getKey();
            Runnable action = first.getValue();
            if (!game.decoderRenderer.runUnlessRebuilding(action)) return false;
            if (output.decoderActions.get(key) == action) output.decoderActions.remove(key);
        }
        return true;
    }

    private void trackReconfigure() {
        // Every stream: what a menu opened during a rebuild shows, also with automatic quality off.
        game.adaptation.supportsAdaptiveQuality();
        int[] size = game.decoderRenderer.getStreamSize();
        streamStartedLandscape = size[0] >= size[1];
        int fpsMillihz = game.decoderRenderer.getStreamFrameRateMilliHz();
        if (fpsMillihz <= 0) fpsMillihz = game.streamFps * 1000;
        // What the phone offered when the stream was asked for, in the window shape and on the panel
        // it had then: a turn, fold or unfold while the stream connected was not seen, so the settled
        // check below still asks for it.
        boolean launchLandscape = output.launchWidth > 0 && output.launchHeight > 0 ?
                output.launchWidth >= output.launchHeight : windowLandscape();
        int[] offered = offeredStream(size[0], size[1], fpsMillihz, launchLandscape,
                output.launchPanel != null ? output.launchPanel : PreferenceConfiguration.nativeResolution(game));
        if (offered == null) {
            offered = new int[] {size[0], size[1], fpsMillihz};
        }
        game.reconfigureRequester.start(size[0], size[1], fpsMillihz, offered[0], offered[1], offered[2]);
        game.qualityMaxWidth = size[0];
        game.qualityMaxHeight = size[1];
        game.qualityMaxFps = game.streamFps;
        game.qualityMaxFrameRateMilliHz = fpsMillihz;
        // The surface was created before negotiation supplied the exact stream cadence.
        output.refresh.refreshSurfaceFrameRateVote();
        scheduleReconfigureCheck();
    }

    void checkStreamReconfigure() {
        if (!game.connected || game.isFinishing() || game.decoderRenderer == null || (!game.nativeResolutionStream && !game.fpsFollowsDisplay &&
                !StreamOrientation.followsRotation(game.streamOrientation, game.hostShapesStream))) {
            return;
        }
        long now = SystemClock.uptimeMillis();
        if (!game.reconfigureRequester.settled(now)) {
            game.bitrateHandler.postDelayed(game.checkReconfigure, game.reconfigureRequester.remainingMs(now));
            return;
        }
        if ((Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && game.isInPictureInPictureMode()) ||
                !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_RECONFIGURE)) {
            return;
        }
        if (game.adaptiveQuality != null && game.pendingQualityTarget != null) {
            game.bitrateHandler.postDelayed(game.checkReconfigure, 1000);
            return;
        }
        int[] offered = offeredStream(game.adaptiveQuality == null ? game.reconfigureRequester.width() : game.qualityMaxWidth,
                game.adaptiveQuality == null ? game.reconfigureRequester.height() : game.qualityMaxHeight,
                game.adaptiveQuality == null ? game.reconfigureRequester.fpsMillihz() : game.adaptation.qualityCeilingFrameRateMilliHz());
        if (game.adaptiveQuality != null && offered != null) {
            AdaptiveQualityController.Target target = game.adaptiveQuality.resizeCeiling(offered[0], offered[1],
                    Math.round(offered[2] / 1000f), now);
            // The quality ladder uses whole FPS, but a fractional-only display change still
            // needs to reach the host when the current level uses the ceiling's rate.
            if (target == null && game.adaptiveQuality.current().fps == Math.round(offered[2] / 1000f) &&
                    game.reconfigureRequester.fpsMillihz() != offered[2]) target = game.adaptiveQuality.current();
            if (target == null || game.adaptation.applyAdaptiveQuality(target, now, offered[2])) {
                game.qualityMaxWidth = offered[0]; game.qualityMaxHeight = offered[1]; game.qualityMaxFps = Math.round(offered[2] / 1000f);
                game.qualityMaxFrameRateMilliHz = offered[2];
                game.streamPanelHz = game.panelMaxRefreshRate;
            }
            return;
        }
        int[] request = offered == null ? null : game.reconfigureRequester.decide(offered[0], offered[1], offered[2]);
        if (request == null) {
            return;
        }
        if (requestStreamReconfigure(request, offered)) {
            // This is the new ceiling even if automatic quality is enabled later.
            game.qualityMaxWidth = offered[0]; game.qualityMaxHeight = offered[1]; game.qualityMaxFps = Math.round(offered[2] / 1000f);
            game.qualityMaxFrameRateMilliHz = offered[2];
            game.streamPanelHz = game.panelMaxRefreshRate;
        }
    }

    boolean requestStreamReconfigure(int[] request) {
        return requestStreamReconfigure(request, request);
    }

    boolean requestManualStreamReconfigure(int[] request) {
        return requestStreamReconfigure(request, null);
    }

    private boolean requestStreamReconfigure(int[] request, int[] offeredBaseline) {
        LimeLog.info("Reconfigure: asking for " + request[0] + "x" + request[1] + " at " + request[2] / 1000f +
                " fps (was " + game.reconfigureRequester.width() + "x" + game.reconfigureRequester.height() + " at " +
                game.reconfigureRequester.fpsMillihz() / 1000f + " fps)");
        if (!game.decoderRenderer.prepareForReconfigure(request[0], request[1])) {
            if (game.decoderRenderer.reconfigureLimitsPending()) {
                // Actions run through whenDecoderReady, which no rebuild starts or ends during.
                LimeLog.warning("Reconfigure: asked while the decoder is being set up again");
            }
            LimeLog.warning("Reconfigure: the decoder cannot switch to " + request[0] + "x" + request[1] +
                    "; the stream stays as it is");
            if (StreamOrientation.followsRotation(game.streamOrientation, game.hostShapesStream) && !output.orientationNoticeShown) {
                output.orientationNoticeShown = true;
                game.messages.alert(R.string.stream_orientation_decoder_limit, Toast.LENGTH_LONG);
            }
            return false;
        }
        if (!MoonBridge.requestReconfigure(request[0], request[1], request[2])) {
            LimeLog.warning("Reconfigure: request not sent");
            // No IDR at that size comes: the decoder keeps its size and does not restart for it.
            game.decoderRenderer.cancelReconfigure();
            return false;
        }
        boolean fpsChanged = request[2] != game.reconfigureRequester.fpsMillihz();
        game.reconfigureRequester.sent(request, offeredBaseline);
        game.decoderRenderer.resetDecodeTiming();
        if (fpsChanged) setStreamFrameRate(request[2]);
        return true;
    }

    // The stream runs at this rate now: a reconfigure asked for it, or a 0x553C rate change ran.
    void setStreamFrameRate(int fpsMillihz) {
        int fps = Math.round(fpsMillihz / 1000f);
        game.streamFps = fps;
        game.prefConfig.fps = fps;
        game.decoderRenderer.setStreamFrameRateMilliHz(fpsMillihz);
        if (game.adaptiveBitrate != null) game.adaptiveBitrate.setFrameRate(fps);
        output.refresh.refreshSurfaceFrameRateVote();
        game.menuActions.refreshStreamMenu();
    }

    // What this phone offers a stream that follows it, {width, height, fps in millihertz}, given
    // the stream's current values; null when this stream cannot change in place.
    private int[] offeredStream(int width, int height, int fpsMillihz) {
        return offeredStream(width, height, fpsMillihz, windowLandscape(), PreferenceConfiguration.nativeResolution(game));
    }

    // As above, for a window of that orientation on a screen whose own resolution is nativeResolution.
    private int[] offeredStream(int width, int height, int fpsMillihz, boolean windowLandscape, String nativeResolution) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return null;
        }
        int[] limits = game.decoderRenderer.getReconfigureLimits();
        if (limits == null) {
            return null;
        }
        int[] size = game.nativeResolutionStream ? nativeStreamSize(limits, windowLandscape, nativeResolution) :
                StreamOrientation.followsRotation(game.streamOrientation, game.hostShapesStream) && !game.isInMultiWindowMode() ?
                StreamOrientation.turnedSize(output.launchWidth, output.launchHeight, windowLandscape, limits[0], limits[1],
                        limits[2], limits[3]) : null;
        int fps = StreamResizePolicy.targetFpsMillihz(game.fpsFollowsDisplay, fpsMillihz, game.streamPanelHz, game.panelMaxRefreshRate);
        return size == null ? new int[] {width, height, fps} : new int[] {size[0], size[1], fps};
    }

    @TargetApi(Build.VERSION_CODES.O)
    private int[] nativeStreamSize(int[] limits, boolean windowLandscape, String nativeResolution) {
        if (game.isInMultiWindowMode()) {
            // Split screen or a desktop window: the stream fills the window's pane, in panel pixels.
            View pane = game.findViewById(android.R.id.content);
            int paneWidth = pane.getWidth() - pane.getPaddingLeft() - pane.getPaddingRight();
            int paneHeight = pane.getHeight() - pane.getPaddingTop() - pane.getPaddingBottom();
            Display display = game.getWindowManager().getDefaultDisplay();
            Point real = new Point();
            display.getRealSize(real);
            // The screen in use's own long side: a foldable can list the other panel's modes too.
            String panel = PreferenceConfiguration.nativeResolution(game);
            int modeLong = panel == null ? 0 : Integer.parseInt(panel.substring(0, panel.indexOf('x')));
            float scale = modeLong > 0 && real.x > 0 && real.y > 0 ? (float) modeLong / Math.max(real.x, real.y) : 1;
            return StreamResizePolicy.nativeSize(paneWidth, paneHeight, scale, StreamResizePolicy.ORIENTATION_OF_AREA,
                    limits[0], limits[1], limits[2], limits[3]);
        }
        // Full screen: the same native resolution the stream starts with, in the orientation it
        // started in, even after a split-screen window turned it; with Rotate on a host that
        // draws the stream at its shape, in the window's orientation.
        if (nativeResolution == null) {
            return null;
        }
        String[] parts = nativeResolution.split("x");
        return StreamResizePolicy.nativeSize(Integer.parseInt(parts[0]), Integer.parseInt(parts[1]), 1,
                StreamOrientation.streamOrientation(game.streamOrientation, game.hostShapesStream, streamStartedLandscape,
                        windowLandscape),
                limits[0], limits[1], limits[2], limits[3]);
    }

    private boolean windowLandscape() {
        Point real = new Point();
        game.getWindowManager().getDefaultDisplay().getRealSize(real);
        return real.x >= real.y;
    }

    String displaySignature() {
        Display display = output.getVideoDisplay();
        // The input pane must not hide a refresh-capability change on the video output.
        // Local output still tracks the phone's native size across folds and mode changes.
        String resolution = game.externalPresentation == null ? PreferenceConfiguration.nativeResolution(game) : "";
        // No "@": the problem report's redactor reads "1344x2992@120.0" as an e-mail address.
        return display.getDisplayId() + ": " + (resolution == null || resolution.isEmpty() ? "" : resolution + ", ") + "up to " +
                DisplayFrameRatePolicy.maxRefreshRate(display) + " Hz";
    }

    // Display changes also fire for refresh rate switches; only a new panel or a new top refresh
    // rate matters here.
    void onDisplayModesChanged() {
        // ARR support can change with the active mode even when size and maximum Hz do not.
        // Observe it independently of the selected policy; changing votes here could oscillate modes.
        output.refresh.logDisplayRefreshPolicy(output.getVideoDisplay());
        String signature = displaySignature();
        if (signature.equals(output.displaySignature)) {
            return;
        }
        LimeLog.info("Display modes changed: " + output.displaySignature + " -> " + signature);
        output.displaySignature = signature;
        output.configureStreamWindow();
        game.panelMaxRefreshRate = DisplayFrameRatePolicy.maxRefreshRate(
                output.getVideoDisplay(), game.prefConfig.width, game.prefConfig.height);
        if (!game.useArr && game.appSurfaceFrameRateVote != null) {
            output.refresh.syncWindowDisplayMode(game.appSurfaceFrameRateVote, output.appSurfaceSeamlessOnly);
        }
        scheduleReconfigureCheck();
    }

    void onPaneLaidOut(View pane) {
        if (game.externalPresentation != null) return;
        int width = pane.getWidth() - pane.getPaddingLeft() - pane.getPaddingRight();
        int height = pane.getHeight() - pane.getPaddingTop() - pane.getPaddingBottom();
        if (width == lastPaneWidth && height == lastPaneHeight) {
            return;
        }
        lastPaneWidth = width;
        lastPaneHeight = height;
        // Full-screen sizes come from the display; only a window follows its own pane.
        if (game.isInMultiWindowMode()) {
            scheduleReconfigureCheck();
        }
    }
}
