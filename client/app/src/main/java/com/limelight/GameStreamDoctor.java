package com.limelight;

import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.View;

import com.limelight.binding.video.StreamDoctorController;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.nativeapi.HostErrorText;
import com.limelight.nativeapi.KnownCapabilities;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeStreamDoctor;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;

/**
 * The stream's side of Stream Doctor 2.0 ({@link StreamDoctorController}): this stream's facts, the host's doctor on the
 * native menu thread while the host lists {@code stream.doctor_v2}, the classic bitrate trial of {@link GameAdaptation},
 * and the in-place mode change of {@link GameVideoReconfigure} (0x5532). The card opens from the Control Center's
 * "Why did my stream stutter?" ({@link com.limelight.ui.StreamDoctorSheet}).
 */
final class GameStreamDoctor implements StreamDoctorController.Host {
    private final Game game;
    private final GameMenuActions menu;
    private final GameHostActions hostActions;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final StreamDoctorController controller = new StreamDoctorController(this);
    // Whether the stream's host lists the feature, read once per connection.
    private NvConnection offeredFor;
    private boolean offered;

    GameStreamDoctor(Game game, GameMenuActions menu, GameHostActions hostActions) {
        this.game = game;
        this.menu = menu;
        this.hostActions = hostActions;
    }

    /** Opens the card over the stream. */
    void show() {
        if (!game.connected || game.isFinishing() || game.conn == null) return;
        com.limelight.ui.StreamDoctorSheet sheet = new com.limelight.ui.StreamDoctorSheet(game, controller);
        menu.showStreamDialog(sheet.dialog());
        sheet.start();
    }

    @Override public boolean offered() {
        NvConnection connection = game.conn;
        if (connection == null || game.app.getRole() != NvApp.Role.STREAM) return false;
        if (connection != offeredFor) {
            offeredFor = connection;
            offered = false;
            ComputerDetails details = connection.getHostDetails();
            String known = KnownCapabilities.of(details.uuid);
            if (known != null) {
                try {
                    offered = details.caps.withNativeCapabilities(known).has(HostCapabilities.Feature.STREAM_DOCTOR_V2);
                } catch (org.json.JSONException e) {
                    LimeLog.warning("Stream doctor: unreadable native capabilities: " + e.getMessage());
                }
            }
        }
        // The capabilities arrive a moment into the stream: until then, ask again on the next look.
        if (!offered && KnownCapabilities.of(connection.getHostDetails().uuid) == null) offeredFor = null;
        return offered;
    }

    private NativeApiClient client() throws NativeApiException {
        NvConnection connection = game.conn;
        NativeApiClient client = connection == null ? null : connection.nativeApiClient();
        if (client == null) throw new NativeApiException(NativeApiException.Kind.UNAVAILABLE, "no native API for this stream");
        return client;
    }

    @Override public NativeStreamDoctor fetch() throws NativeApiException {
        return client().streamDoctor();
    }

    @Override public NativeStreamDoctor apply(boolean undo, String trialId) throws NativeApiException {
        return client().streamDoctorApply(undo, trialId);
    }

    @Override public boolean background(Runnable task) {
        try {
            hostActions.nativeMenuThread().execute(task);
            return true;
        } catch (java.util.concurrent.RejectedExecutionException e) {
            // The stream is over.
            return false;
        }
    }

    @Override public void ui(Runnable task) {
        game.runOnUiThread(() -> {
            if (!game.isFinishing()) task.run();
        });
    }

    @Override public void later(Runnable task, long delayMs) {
        main.postDelayed(task, delayMs);
    }

    @Override public void cancel(Runnable task) {
        main.removeCallbacks(task);
    }

    @Override public long nowMs() {
        return SystemClock.uptimeMillis();
    }

    @Override public StreamDoctorController.Facts facts() {
        if (!game.connected || game.conn == null || game.decoderRenderer == null || game.isFinishing()) return null;
        StreamDoctorController.Facts f = new StreamDoctorController.Facts();
        f.report = game.streamHealth.report(SystemClock.uptimeMillis());
        f.width = game.streamWidth;
        f.height = game.streamHeight;
        f.fps = game.streamFps;
        android.view.Window videoWindow = game.videoOutput != null ? game.videoOutput.getVideoWindow() : null;
        View window = (videoWindow != null ? videoWindow : game.getWindow()).getDecorView();
        f.screenWidth = window.getWidth();
        f.screenHeight = window.getHeight();
        int format = game.decoderRenderer.getActiveVideoFormat();
        f.hdr = game.streamHdr;
        f.tenBit = !game.streamHdr && (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
        f.pyrowave = (format & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0;
        UpscalingPolicy.Mode upscaling = game.prefConfig.upscalingMode;
        f.upscaler = upscaling == UpscalingPolicy.Mode.FSR1 ? "fsr1" : upscaling == UpscalingPolicy.Mode.SGSR1 ? "sgsr1" : "off";
        f.receiveBuffer = MoonBridge.getVideoReceiveBufferInfo();
        f.bitrateManual = Game.healthTrialAllowed(game.adaptation.isAdaptiveBitrateEnabled(),
                game.adaptation.isRateChangeBitrateOn(), game.adaptiveQuality != null);
        f.currentKbps = game.currentBitrate == 0 ? game.prefConfig.bitrate : game.currentBitrate;
        f.classicBitrate = game.adaptation.supportsAdaptiveBitrate() && game.currentBitrate > 500 && !game.hostActionInProgress;
        f.classicUndoKbps = game.healthOriginalBitrate > 0 && game.currentBitrate == game.healthTrialBitrate && f.bitrateManual
                ? game.healthOriginalBitrate : 0;
        f.resize = canResize(f.pyrowave);
        f.technical = Game.healthText(game, f.report, hostActions.wifiText(f.report.stallPeriodMs)).toString();
        return f;
    }

    // The mode changes in place only where the stream's own reconfigure would: a host that takes 0x5532, a decoder
    // that changes size without a rebuild, outside picture-in-picture, without automatic quality (which owns the
    // resolution) and not for PyroWave.
    private boolean canResize(boolean pyrowave) {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && !game.isInPictureInPictureMode() && !pyrowave
                && game.app.getRole() == NvApp.Role.STREAM && game.adaptiveQuality == null
                && MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_RECONFIGURE)
                && game.decoderRenderer.getReconfigureLimits() != null;
    }

    @Override public String diagnose(String request) {
        return NightjarPolicyJni.doctorDiagnose(request);
    }

    @Override public Object stream() {
        return game.conn;
    }

    @Override public void classicTrial(boolean undo) {
        game.adaptation.applyHealthBitrate(undo);
    }

    @Override public boolean resize(int width, int height) {
        if (!game.connected || game.decoderRenderer == null) return false;
        if (!canResize((game.decoderRenderer.getActiveVideoFormat() & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0)) return false;
        int fpsMillihz = game.reconfigureRequester.fpsMillihz() > 0 ? game.reconfigureRequester.fpsMillihz()
                : game.streamFps * 1000;
        return game.videoOutput.requestManualStreamReconfigure(new int[] {width, height, fpsMillihz});
    }

    @Override public void hostBitrateApplied(int kbps) {
        game.currentBitrate = kbps;
        // The host's trial owns the bitrate now: the classic one is over.
        game.healthOriginalBitrate = game.healthTrialBitrate = 0;
    }

    @Override public String describe(NativeApiException error) {
        return HostErrorText.of(game.getResources(), error);
    }
}
