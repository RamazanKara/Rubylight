package com.limelight;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.computers.HostNotices;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.AdaptiveBitrateController;
import com.limelight.nvstream.AdaptiveQualityController;
import com.limelight.nvstream.PyroWaveBitrateController;
import com.limelight.nvstream.RateChangeResult;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.AutoBitrateChoice;
import com.limelight.ui.BitrateSlider;
import androidx.appcompat.app.AlertDialog;
import com.limelight.ui.NavigationDialogBuilder;
import android.content.Context;
import android.content.SharedPreferences;
import android.os.SystemClock;
import android.text.InputType;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.Toast;
import org.xmlpull.v1.XmlPullParserException;
import java.io.IOException;
import java.math.BigDecimal;

/** Adapting the stream to the link: auto bitrate and Foundation ABR, adaptive quality, stream health sampling, the bitrate and rate change dialogs. */
final class GameAdaptation {
    private final Game game;

    GameAdaptation(Game game) {
        this.game = game;
    }

    // Claims of the host's encoder bitrate that failed in a row. Some hosts report 0 for good.
    private static final int MAX_BITRATE_CLAIM_FAILURES = 6;
    private int adaptiveBitrateClaimFailures;
    // A Foundation Sunshine PC's own bitrate controller, on in place of this device's.
    private com.limelight.nvstream.http.FoundationAbr foundationAbr;

    final Runnable sampleStreamHealth = new Runnable() {
        @Override public void run() {
            if (!game.connected || game.decoderRenderer == null || game.app.getRole() == NvApp.Role.INPUT_ONLY) return;
            if (game.foreground && !game.hostActionInProgress) {
                // A hanging decoder holds the locks this sample reads for seconds: never on the main thread.
                MediaCodecDecoderRenderer renderer = game.decoderRenderer;
                int fps = game.streamFps;
                if (!streamHealthSampler().sample(() -> {
                    long rtt = MoonBridge.getEstimatedRttInfo();
                    return renderer.healthSample(SystemClock.uptimeMillis(), fps,
                            rtt == -1 ? -1 : (float) (rtt & 0xffffffffL));
                }, game.streamHealth::add)) {
                    renderer.skipHealthSample();
                    LimeLog.info("Stream health: the last sample is still running; this one is skipped");
                }
            } else {
                game.decoderRenderer.skipHealthSample();
            }
            game.bitrateHandler.postDelayed(this, 1000);
        }
    };

    private com.limelight.utils.StreamHealthSampler streamHealthSampler;

    private com.limelight.utils.StreamHealthSampler streamHealthSampler() {
        if (streamHealthSampler == null) {
            game.streamHealthThread = java.util.concurrent.Executors.newSingleThreadExecutor(task -> {
                Thread thread = new Thread(task, "Stream health");
                thread.setDaemon(true);
                return thread;
            });
            streamHealthSampler = new com.limelight.utils.StreamHealthSampler(game.streamHealthThread, game.bitrateHandler::post);
        }
        return streamHealthSampler;
    }

    // Once a second: this second's loss and frame rate for a Foundation PC's controller, and the
    // report every three seconds (FoundationAbr).
    private final Runnable foundationAbrTick = new Runnable() {
        @Override
        public void run() {
            com.limelight.nvstream.http.FoundationAbr abr = foundationAbr;
            if (!game.connected || abr == null) return;
            float[] rates = game.decoderRenderer.getFrameRates();
            abr.add(game.decoderRenderer.getNetworkFrameLossPercent(), rates.length > 1 ? Math.max(0, rates[1]) : 0);
            long rttInfo = MoonBridge.getEstimatedRttInfo();
            org.json.JSONObject feedback = abr.due(SystemClock.uptimeMillis(), rttInfo == -1 ? 0 : rttInfo >>> 32);
            if (feedback != null) {
                NvConnection connection = game.conn;
                new Thread(() -> {
                    org.json.JSONObject action = null;
                    try {
                        action = connection.sendFoundationAbrFeedback(feedback);
                    } catch (IOException e) {
                        LimeLog.warning("The PC's automatic bitrate report failed: " + e.getMessage());
                    }
                    org.json.JSONObject answer = action;
                    game.runOnUiThread(() -> {
                        if (foundationAbr != abr) return;
                        int kbps = abr.answered(answer);
                        if (kbps > 0) {
                            LimeLog.info("The PC set " + kbps + " kbps: " + answer.optString("reason"));
                            game.currentBitrate = kbps;
                        }
                    });
                }, "FoundationAbr").start();
            }
            game.bitrateHandler.postDelayed(this, 1000);
        }
    };
    /** The heat the bitrate and quality controllers react to: none unless Thermal protection is on. */
    int adaptationThermalLevel() {
        return com.limelight.binding.video.ThermalMonitor.forAdaptation(game.prefConfig.thermalProtection,
                game.decoderRenderer.getThermalLevel());
    }

    void toggleAdaptiveBitrateFromMenu() {
        // Automatic bitrate on this device turns on and off for this stream only; the setting stays.
        if (isRateChangeBitrateOn() || (!isAdaptiveBitrateEnabled() && game.rateChanges != null && game.prefConfig.rateChangeBitrate)) {
            boolean enable = !isRateChangeBitrateOn();
            setRateChangeBitrateEnabled(enable);
            // The setting's own explanation, as the settings screen shows it.
            if (enable) game.messages.alert(R.string.summary_rate_change_bitrate, Toast.LENGTH_LONG);
            return;
        }
        boolean enable = !isAdaptiveBitrateEnabled();
        if (!enable || !game.hostActionInProgress) {
            setAdaptiveBitrateEnabled(enable, true);
            if (enable) {
                game.messages.alert(game.pyroWaveBitrate != null ? R.string.stream_pyrowave_bitrate_help :
                        R.string.stream_auto_bitrate_help, Toast.LENGTH_LONG);
            }
        } else {
            game.messages.alert(R.string.stream_bitrate_busy, Toast.LENGTH_SHORT);
        }
    }

    void applyHealthBitrate(boolean undo) {
        if (!game.foreground || !game.connected || game.hostActionInProgress || !supportsAdaptiveBitrate() ||
                !Game.healthTrialAllowed(isAdaptiveBitrateEnabled(), isRateChangeBitrateOn(), game.adaptiveQuality != null)) return;
        if (undo && (game.healthOriginalBitrate <= 0 || game.currentBitrate != game.healthTrialBitrate)) return;
        final int original = game.healthOriginalBitrate;
        final int expectedTrial = game.healthTrialBitrate;
        final int generation = game.foregroundGeneration;
        final NvConnection connection = game.conn;
        game.hostActionInProgress = true;
        new Thread(() -> {
            try {
                // The saved preference/current display value may be a transport budget including
                // FEC/audio, or may predate host adaptation. Only the live encoder rate is a baseline.
                int previous = connection.refreshHostDetails().encoderBitrateKbps;
                if (previous <= 0) throw new IOException(game.getString(R.string.health_bitrate_unavailable));
                if ((undo && previous != expectedTrial) || (!undo && previous <= 500)) {
                    game.runOnUiThread(() -> {
                        if (!game.connected || game.conn != connection || game.isFinishing() || isAdaptiveBitrateEnabled() || game.adaptiveQuality != null) return;
                        game.currentBitrate = previous;
                        game.healthOriginalBitrate = game.healthTrialBitrate = 0;
                        if (game.foreground && generation == game.foregroundGeneration) {
                            game.messages.alert(undo ? R.string.health_trial_replaced : R.string.health_bitrate_minimum, Toast.LENGTH_LONG);
                        }
                    });
                    return;
                }
                if (!game.connected || game.conn != connection) return;
                int target = undo ? original : Math.max(500, (int) ((long) previous * 4 / 5));
                int applied = connection.setBitrate(target);
                game.runOnUiThread(() -> {
                    if (!game.connected || game.conn != connection || game.isFinishing() || isAdaptiveBitrateEnabled() || game.adaptiveQuality != null) return;
                    game.currentBitrate = applied;
                    game.healthOriginalBitrate = undo ? 0 : previous;
                    game.healthTrialBitrate = undo ? 0 : applied;
                    if (game.foreground && generation == game.foregroundGeneration) {
                        game.messages.alert(game.getString(R.string.stream_bitrate_applied, applied / 1000.0), Toast.LENGTH_LONG);
                    }
                });
            } catch (IOException | XmlPullParserException e) {
                showHostActionError(e, generation);
            } finally {
                game.runOnUiThread(() -> game.hostActionInProgress = false);
            }
        }, "Stream doctor bitrate trial").start();
    }

    void showBitrateDialog() {
        com.google.android.material.textfield.TextInputLayout field = new com.google.android.material.textfield.TextInputLayout(game);
        field.setBoxBackgroundMode(com.google.android.material.textfield.TextInputLayout.BOX_BACKGROUND_OUTLINE);
        field.setHint(game.getString(R.string.stream_bitrate));
        int padding = Math.round(24 * game.getResources().getDisplayMetrics().density);
        LinearLayout fields = new LinearLayout(game);
        fields.setOrientation(LinearLayout.VERTICAL);
        fields.setPadding(padding, 0, padding, 0);
        EditText input = new com.google.android.material.textfield.TextInputEditText(game);
        input.setMinHeight(Math.round(48 * game.getResources().getDisplayMetrics().density));
        input.setSingleLine(true);
        field.addView(input);
        input.setInputType(InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL);
        int bitrateKbps = game.currentBitrate == 0 ? game.prefConfig.bitrate : game.currentBitrate;
        input.setText(BigDecimal.valueOf(bitrateKbps, 3).stripTrailingZeros().toPlainString());
        input.selectAll();
        AlertDialog bitrateDialog = new NavigationDialogBuilder(game).setTitle(R.string.stream_bitrate)
                .setMessage(R.string.stream_bitrate_help).setView(fields)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(android.R.string.ok, null).create();
        fields.addView(BitrateSlider.create(game, input, bitrateKbps, () -> applyFromSlider(bitrateDialog)));
        fields.addView(field);
        game.menuActions.showStreamDialog(bitrateDialog);
        bitrateDialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            final int kbps;
            try {
                kbps = new BigDecimal(input.getText().toString().trim().replace(',', '.'))
                        .movePointRight(3).intValueExact();
                if (kbps < 500 || kbps > StreamConfiguration.MAX_BITRATE_KBPS) {
                    throw new NumberFormatException();
                }
            } catch (NumberFormatException | ArithmeticException e) {
                field.setError(game.getString(R.string.stream_bitrate_invalid));
                return;
            }
            if (game.hostActionInProgress) {
                game.messages.alert(R.string.stream_bitrate_busy, Toast.LENGTH_SHORT);
                return;
            }
            if (!game.foreground || !game.connected || game.hostActionInProgress) {
                return;
            }
            // A manual bitrate stops automatic bitrate for this session only; the saved choice stays.
            setAdaptiveBitrateEnabled(false, false);
            game.qualityAutoBitrate = false;
            bitrateDialog.dismiss();
            game.hostActionInProgress = true;
            final int generation = game.foregroundGeneration;
            new Thread(() -> {
                try {
                    int applied = game.conn.setBitrate(kbps);
                    game.runOnUiThread(() -> {
                        game.currentBitrate = applied;
                        game.healthOriginalBitrate = game.healthTrialBitrate = 0;
                        if (game.foreground && generation == game.foregroundGeneration && game.connected && !game.isFinishing()) {
                            game.messages.alert(game.getString(R.string.stream_bitrate_applied, applied / 1000.0), Toast.LENGTH_LONG);
                        }
                    });
                } catch (IOException | XmlPullParserException e) {
                    showHostActionError(e, generation);
                } finally {
                    game.runOnUiThread(() -> game.hostActionInProgress = false);
                }
            }, "Stream bitrate").start();
        });
    }

    // A settled bitrate slider sends through its dialog's OK, with the same checks as a typed value.
    private static void applyFromSlider(AlertDialog dialog) {
        if (dialog != null && dialog.isShowing()) dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
    }

    // Bitrate and frame rate in one 0x553C request; a field left as it is keeps its value.
    void showRateChangeDialog() {
        final StreamRateChanges stream = game.rateChanges;
        if (stream == null) return;
        float density = game.getResources().getDisplayMetrics().density;
        int padding = Math.round(24 * density);
        LinearLayout fields = new LinearLayout(game);
        fields.setOrientation(LinearLayout.VERTICAL);
        fields.setPadding(padding, 0, padding, 0);
        int bitrateKbps = game.currentBitrate == 0 ? game.prefConfig.bitrate : game.currentBitrate;
        String bitrateText = BigDecimal.valueOf(bitrateKbps, 3).stripTrailingZeros().toPlainString();
        com.google.android.material.textfield.TextInputLayout bitrateField = rateChangeField(
                game.getString(R.string.stream_bitrate), bitrateText, density);
        final AlertDialog[] opened = new AlertDialog[1];
        fields.addView(BitrateSlider.create(game, bitrateField.getEditText(), bitrateKbps,
                () -> applyFromSlider(opened[0])));
        fields.addView(bitrateField);
        // Automatic quality owns the frame rate while it runs.
        int fpsMillihz = game.reconfigureRequester.fpsMillihz() > 0 ? game.reconfigureRequester.fpsMillihz() : game.streamFps * 1000;
        String fpsText = BigDecimal.valueOf(fpsMillihz, 3).stripTrailingZeros().toPlainString();
        com.google.android.material.textfield.TextInputLayout fpsField = game.adaptiveQuality != null ? null :
                rateChangeField(game.getString(R.string.stream_rate_change_fps), fpsText, density);
        if (fpsField != null) fields.addView(fpsField);
        AlertDialog dialog = new NavigationDialogBuilder(game).setTitle(R.string.stream_rate_change)
                .setMessage(R.string.stream_rate_change_help).setView(fields)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(android.R.string.ok, null).create();
        opened[0] = dialog;
        game.menuActions.showStreamDialog(dialog);
        dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            // connected stays true until stopConnection, which closes this dialog: a termination
            // that has arrived but not yet stopped the stream counts as ended (streamEnding).
            if (!game.connected || game.streamConnection.streamEnding() || game.rateChanges != stream) {
                // The stream this dialog was opened for ended or restarted: nothing goes out.
                dialog.dismiss();
                LimeLog.warning("Rate change: the stream ended before the request was sent");
                game.messages.alert(R.string.stream_rate_change_stream_ended, Toast.LENGTH_LONG);
                return;
            }
            bitrateField.setError(null);
            if (fpsField != null) fpsField.setError(null);
            int kbps = parseRateChange(bitrateField, bitrateText, 500, StreamConfiguration.MAX_BITRATE_KBPS,
                    R.string.stream_bitrate_invalid);
            int millihz = fpsField == null ? 0 : parseRateChange(fpsField, fpsText, 10_000, 500_000,
                    R.string.stream_rate_change_fps_invalid);
            if (kbps < 0 || millihz < 0) return;
            if (kbps == 0 && millihz == 0) {
                bitrateField.setError(game.getString(R.string.stream_rate_change_nothing));
                return;
            }
            dialog.dismiss();
            if (!stream.sendManual(kbps, millihz, SystemClock.uptimeMillis())) {
                LimeLog.warning("Rate change: request not sent");
                game.messages.alert(R.string.stream_rate_change_not_sent, Toast.LENGTH_LONG);
                return;
            }
            scheduleRateChangeExpiry();
            LimeLog.info("Rate change: asking for " + kbps + " kbps, " + millihz + " mHz");
            if (kbps != 0) {
                // As with the bitrate dialog, a bitrate set by hand stops the host-side automatic
                // bitrate for this session; automatic bitrate on this device follows it instead.
                setAdaptiveBitrateEnabled(false, false);
                game.qualityAutoBitrate = false;
                game.healthOriginalBitrate = game.healthTrialBitrate = 0;
            }
        });
    }

    private com.google.android.material.textfield.TextInputLayout rateChangeField(String hint, String text, float density) {
        com.google.android.material.textfield.TextInputLayout field = new com.google.android.material.textfield.TextInputLayout(game);
        field.setBoxBackgroundMode(com.google.android.material.textfield.TextInputLayout.BOX_BACKGROUND_OUTLINE);
        field.setHint(hint);
        EditText input = new com.google.android.material.textfield.TextInputEditText(game);
        input.setMinHeight(Math.round(48 * density));
        input.setSingleLine(true);
        input.setInputType(InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL);
        input.setText(text);
        field.addView(input);
        return field;
    }

    // The field's value in thousandths (kbps from Mbps, mHz from fps); 0 when it is empty or as it
    // was, -1 with the error shown when it is outside min-max.
    private int parseRateChange(com.google.android.material.textfield.TextInputLayout field, String unchanged,
                                int min, int max, int error) {
        String text = field.getEditText().getText().toString().trim().replace(',', '.');
        if (text.isEmpty() || text.equals(unchanged)) return 0;
        try {
            int value = new BigDecimal(text).movePointRight(3).intValueExact();
            if (value >= min && value <= max) return value;
        } catch (NumberFormatException | ArithmeticException e) {
            // Shown below.
        }
        field.setError(game.getString(error));
        return -1;
    }

    // The host's answer to a 0x553C request, on the UI thread.
    void onRateChange(RateChangeResult result) {
        if (game.rateChanges == null || !game.connected || game.isFinishing()) return;
        StreamRateChanges.Outcome outcome = game.rateChanges.onResult(result, SystemClock.uptimeMillis());
        StreamRateChanges.Answer answer = outcome.answer;
        if (answer == StreamRateChanges.Answer.UNKNOWN) {
            LimeLog.warning("Rate change: a result for no request of this stream, " + result);
            return;
        }
        LimeLog.info("Rate change: " + result + " (" + answer + (outcome.takesBitrate ? ")" : ", older bitrate)"));
        // An answer that came after a later request's keeps that one's bitrate.
        if (outcome.takesBitrate) game.currentBitrate = result.bitrateKbps;
        if (result.fpsChanged() && result.fpsMillihz > 0 && result.fpsMillihz != game.reconfigureRequester.fpsMillihz()) {
            game.reconfigureRequester.rateChanged(result.fpsMillihz);
            game.videoOutput.setStreamFrameRate(result.fpsMillihz);
        }
        if (answer == StreamRateChanges.Answer.AUTOMATIC_STOPPED) {
            game.bitrateHandler.removeCallbacks(game.updateRateChangeBitrate);
            LimeLog.warning("Automatic bitrate: the host refused a rate change; off for this stream");
            game.messages.alert(game.getString(R.string.stream_rate_change_auto_stopped,
                    Game.rateChangeReason(game, result.bitrateReason)), Toast.LENGTH_LONG);
        } else if (answer == StreamRateChanges.Answer.MANUAL) {
            game.messages.alert(Game.rateChangeText(game, result), Toast.LENGTH_LONG);
        }
    }

    // At the stream's start: rate changes when the host offered them, and the automatic bitrate
    // the settings ask for, on this device or else on the host.
    void startStreamBitrate() {
        startStreamBitrate(offersRateChange() ? new StreamRateChanges(MoonBridge::sendRateChange) : null);
    }

    void startStreamBitrate(StreamRateChanges changes) {
        game.rateChanges = changes;
        if (game.rateChanges != null && game.prefConfig.rateChangeBitrate) {
            setRateChangeBitrateEnabled(true);
        } else if (autoBitrateAtStart()) {
            setAdaptiveBitrateEnabled(true, false);
        } else if (foundationAbrAtStart()) {
            startFoundationAbr();
        }
    }

    // Automatic bitrate on a PC without Rubylight's runtime bitrate: a Foundation Sunshine PC runs
    // its own controller (as Moonlight V+ asks it to); other PCs answer 404 and nothing changes.
    private boolean foundationAbrAtStart() {
        ComputerDetails details = game.conn.getHostDetails();
        return game.connected && game.app != null && game.app.getRole() == NvApp.Role.STREAM &&
                details.caps.family() != HostCapabilities.Family.RUBYLIGHT &&
                !details.caps.has(HostCapabilities.Feature.RUNTIME_BITRATE) &&
                AutoBitrateChoice.isOn(game, adaptiveBitratePreferences(), game.getIntent().getStringExtra(Game.EXTRA_PC_UUID));
    }

    private void startFoundationAbr() {
        NvConnection connection = game.conn;
        int maxKbps = game.currentBitrate == 0 ? game.prefConfig.bitrate : game.currentBitrate;
        new Thread(() -> {
            try {
                int initial = connection.startFoundationAbr(maxKbps);
                game.runOnUiThread(() -> {
                    if (initial < 0) {
                        LimeLog.info("The PC has no automatic bitrate");
                        return;
                    }
                    if (!game.connected || game.isFinishing() || game.conn != connection) {
                        // The stream ended while the PC turned it on: stopFoundationAbr had nothing to stop.
                        sendFoundationAbrStop(connection);
                        return;
                    }
                    foundationAbr = new com.limelight.nvstream.http.FoundationAbr(initial, maxKbps, SystemClock.uptimeMillis());
                    game.currentBitrate = foundationAbr.currentKbps();
                    LimeLog.info("The PC's automatic bitrate is on: " + game.currentBitrate + " kbps, at most " + maxKbps + " kbps");
                    game.bitrateHandler.postDelayed(foundationAbrTick, 1000);
                });
            } catch (IOException e) {
                LimeLog.warning("The PC did not start its automatic bitrate: " + e.getMessage());
                // Loud, as on desktop: the stream keeps its bitrate and the user should know why.
                game.runOnUiThread(() -> {
                    if (game.connected && !game.isFinishing() && game.conn == connection) {
                        game.messages.alert(game.getString(R.string.stream_host_abr_failed, e.getMessage()), Toast.LENGTH_LONG);
                    }
                });
            }
        }, "FoundationAbr").start();
    }

    // Turns a Foundation PC's controller off at the stream's end (V+ PC's stopSunshineAbr).
    void stopFoundationAbr() {
        if (foundationAbr == null) return;
        foundationAbr = null;
        game.bitrateHandler.removeCallbacks(foundationAbrTick);
        sendFoundationAbrStop(game.conn);
    }

    private static void sendFoundationAbrStop(NvConnection connection) {
        new Thread(() -> {
            try {
                connection.stopFoundationAbr();
            } catch (IOException e) {
                LimeLog.warning("The PC's automatic bitrate did not stop: " + e.getMessage());
            }
        }, "FoundationAbr").start();
    }

    // The host offered this stream 0x553C, which only the Rust engine asks for (rlRateChange).
    private boolean offersRateChange() {
        return game.connected && game.app != null && game.app.getRole() == NvApp.Role.STREAM && MoonBridge.isRustStream() &&
                MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_RATE_CHANGE);
    }

    // The next check for an unanswered 0x553C request, when one is outstanding.
    void scheduleRateChangeExpiry() {
        game.bitrateHandler.removeCallbacks(game.expireRateChanges);
        long at = game.rateChanges == null ? 0 : game.rateChanges.nextExpiryMs();
        if (at > 0) game.bitrateHandler.postAtTime(game.expireRateChanges, at);
    }

    boolean isRateChangeBitrateOn() {
        return game.rateChanges != null && game.rateChanges.automaticOn();
    }

    // Automatic bitrate on this device, for this stream; the host-side automatic bitrate stops,
    // since both would set the same encoder.
    private void setRateChangeBitrateEnabled(boolean enabled) {
        game.bitrateHandler.removeCallbacks(game.updateRateChangeBitrate);
        if (game.rateChanges == null) return;
        if (!enabled) {
            game.rateChanges.stopAutomatic();
            return;
        }
        if (isAdaptiveBitrateEnabled()) setAdaptiveBitrateEnabled(false, false);
        game.qualityAutoBitrate = false;
        game.healthOriginalBitrate = game.healthTrialBitrate = 0;
        game.rateChanges.startAutomatic(game.currentBitrate == 0 ? game.prefConfig.bitrate : game.currentBitrate,
                StreamRateChanges.clientLimitKbps(game.prefConfig.layers == null ? null : game.prefConfig.layers.network,
                        game.prefConfig.mobileBitrateLimit),
                SystemClock.uptimeMillis());
        game.bitrateHandler.postDelayed(game.updateRateChangeBitrate, 1000);
    }

    boolean supportsAdaptiveBitrate() {
        ComputerDetails details = game.conn.getHostDetails();
        return game.app.getRole() != NvApp.Role.INPUT_ONLY && details.caps.has(HostCapabilities.Feature.RUNTIME_BITRATE) &&
                details.encoderBitrateSupported &&
                details.hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH);
    }

    AdaptiveQualityController.Mode qualityMode() {
        return AdaptiveQualityController.Mode.parse(game.getSharedPreferences("AdaptiveQuality", Context.MODE_PRIVATE)
                .getString(game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), game.prefConfig.adaptiveQualityMode.preference()));
    }

    boolean supportsAdaptiveQuality() {
        return game.connected && game.decoderRenderer != null && game.app != null && game.app.getRole() == NvApp.Role.STREAM &&
                (game.decoderRenderer.getActiveVideoFormat() & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) == 0 &&
                MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_RECONFIGURE) &&
                game.decoderRenderer.getReconfigureLimits() != null;
    }

    void startAdaptiveQuality() {
        game.bitrateHandler.removeCallbacks(game.updateQuality);
        game.adaptiveQuality = null;
        game.pendingQualityTarget = null;
        game.qualityStatus = null;
        if (game.decoderRenderer != null) game.decoderRenderer.resetDecodeTiming();
        AdaptiveQualityController.Mode mode = qualityMode();
        if (mode == AdaptiveQualityController.Mode.OFF) {
            releaseQualityAutoBitrate();
            return;
        }
        String pcUuid = game.getIntent().getStringExtra(Game.EXTRA_PC_UUID);
        if (!supportsAdaptiveQuality()) {
            releaseQualityAutoBitrate();
            game.qualityStatus = game.getString(R.string.quality_unavailable);
            // Said once per PC; the performance overlay keeps showing it.
            if (game.foreground) game.messages.notice(pcUuid, HostNotices.Notice.AUTOMATIC_QUALITY_UNAVAILABLE, game.qualityStatus);
            return;
        }
        HostNotices.reset(game, pcUuid, HostNotices.Notice.AUTOMATIC_QUALITY_UNAVAILABLE);
        game.healthOriginalBitrate = game.healthTrialBitrate = 0;
        int[] limits = game.decoderRenderer.getReconfigureLimits();
        game.adaptiveQuality = new AdaptiveQualityController(mode, game.qualityMaxWidth, game.qualityMaxHeight, game.qualityMaxFps,
                game.prefConfig.qualityMinShortEdge, game.prefConfig.qualityMinFps, limits[2], limits[3], SystemClock.uptimeMillis());
        // A user changing the mode returns to the session's ceiling before the new policy starts.
        int[] shown = game.decoderRenderer.getStreamSize();
        if ((!game.adaptiveQuality.current().matches(shown[0], shown[1], game.streamFps) ||
                game.reconfigureRequester.fpsMillihz() != qualityCeilingFrameRateMilliHz()) &&
                !applyAdaptiveQuality(game.adaptiveQuality.current(), SystemClock.uptimeMillis())) return;
        // Automatic quality needs it for this session only; the saved choice stays the user's own.
        if (supportsAdaptiveBitrate() && !isAdaptiveBitrateEnabled() && !isRateChangeBitrateOn()) {
            setAdaptiveBitrateEnabled(true, false);
            game.qualityAutoBitrate = true;
        }
        game.bitrateHandler.postDelayed(game.updateQuality, 1000);
    }

    boolean applyAdaptiveQuality(AdaptiveQualityController.Target target, long now) {
        return applyAdaptiveQuality(target, now, qualityCeilingFrameRateMilliHz());
    }

    int qualityCeilingFrameRateMilliHz() {
        return game.qualityMaxFrameRateMilliHz > 0 ? game.qualityMaxFrameRateMilliHz : game.qualityMaxFps * 1000;
    }

    boolean applyAdaptiveQuality(AdaptiveQualityController.Target target, long now, int ceilingMilliHz) {
        int[] request = target.request();
        if (target.fps == Math.round(ceilingMilliHz / 1000f)) request[2] = ceilingMilliHz;
        if (!game.videoOutput.requestStreamReconfigure(request)) {
            stopAdaptiveQualityWithError();
            return false;
        }
        game.healthOriginalBitrate = game.healthTrialBitrate = 0;
        game.adaptiveQuality.applied(target, now);
        game.pendingQualityTarget = target;
        game.qualityRequestSentAt = now;
        return true;
    }

    void stopAdaptiveQualityWithError() {
        game.adaptiveQuality = null;
        game.pendingQualityTarget = null;
        game.bitrateHandler.removeCallbacks(game.updateQuality);
        releaseQualityAutoBitrate();
        game.qualityStatus = game.getString(R.string.quality_request_failed);
        LimeLog.warning("Automatic quality: requested size was not confirmed; adaptation stopped");
        if (game.foreground) game.messages.alert(game.qualityStatus, Toast.LENGTH_LONG);
    }

    void showAdaptiveQualityDialog() {
        String[] modes = game.getResources().getStringArray(R.array.adaptive_quality_names);
        String[] choices = new String[] {game.getString(R.string.quality_use_settings), modes[0], modes[1], modes[2], modes[3]};
        SharedPreferences saved = game.getSharedPreferences("AdaptiveQuality", Context.MODE_PRIVATE);
        String key = game.getIntent().getStringExtra(Game.EXTRA_PC_UUID);
        int selected = saved.contains(key) ? qualityMode().ordinal() + 1 : 0;
        game.menuActions.showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.title_adaptive_quality)
                .setSingleChoiceItems(choices, selected, (dialog, which) -> {
                    if (!game.foreground || !game.connected || game.hostActionInProgress) return;
                    if (which == 0) saved.edit().remove(key).apply();
                    else saved.edit().putString(key, AdaptiveQualityController.Mode.values()[which - 1].preference()).apply();
                    dialog.dismiss();
                    startAdaptiveQuality();
                    if (qualityMode() == AdaptiveQualityController.Mode.OFF && supportsAdaptiveQuality()) {
                        game.videoOutput.requestStreamReconfigure(new int[] {game.qualityMaxWidth, game.qualityMaxHeight, qualityCeilingFrameRateMilliHz()});
                    }
                })
                .setNeutralButton(R.string.quality_about, (dialog, which) ->
                        game.menuActions.showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.title_adaptive_quality)
                                .setMessage(game.getString(R.string.quality_stream_help) + "\n\n" +
                                        game.getString(supportsAdaptiveQuality() ? R.string.summary_adaptive_quality : R.string.quality_unavailable))
                                .setPositiveButton(android.R.string.ok, null).create()))
                .setNegativeButton(android.R.string.cancel, null).create());
    }

    // Automatic quality is off now: auto bitrate goes back to the person's choice, which was off.
    private void releaseQualityAutoBitrate() {
        if (!game.qualityAutoBitrate) return;
        game.qualityAutoBitrate = false;
        if (isAdaptiveBitrateEnabled()) setAdaptiveBitrateEnabled(false, false);
    }

    boolean isAdaptiveBitrateEnabled() {
        return game.adaptiveBitrate != null || game.pyroWaveBitrate != null;
    }

    private String adaptiveBitratePreferences() {
        return (game.decoderRenderer.getActiveVideoFormat() & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0 ?
                AutoBitrateChoice.PYROWAVE_STORE : AutoBitrateChoice.CODEC_STORE;
    }

    /** Whether the stream starts with automatic bitrate: this PC's choice, else the Settings switch. */
    boolean autoBitrateAtStart() {
        return supportsAdaptiveBitrate() &&
                AutoBitrateChoice.isOn(game, adaptiveBitratePreferences(), game.getIntent().getStringExtra(Game.EXTRA_PC_UUID));
    }

    private void setAdaptiveBitrateEnabled(boolean enabled, boolean remember) {
        if (remember) {
            AutoBitrateChoice.setForPc(game, adaptiveBitratePreferences(),
                    game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), enabled);
            // The person chose it themselves; Automatic quality no longer owns it.
            game.qualityAutoBitrate = false;
        }
        game.bitrateHandler.removeCallbacks(game.updateBitrate);
        game.adaptiveBitrate = null;
        game.pyroWaveBitrate = null;
        game.adaptiveBitrateRetryAt = 0;
        adaptiveBitrateClaimFailures = 0;
        if (enabled) {
            game.healthOriginalBitrate = game.healthTrialBitrate = 0;
            if ((game.decoderRenderer.getActiveVideoFormat() & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
                game.pyroWaveBitrate = new PyroWaveBitrateController(0, 0, game.streamFps,
                        SystemClock.uptimeMillis());
            } else {
                game.adaptiveBitrate = new AdaptiveBitrateController(0, 0, game.streamFps, SystemClock.uptimeMillis(),
                        game.prefConfig.width, game.prefConfig.height, adaptiveBitrateCodec());
            }
            // The preference includes FEC/audio, while runtime changes use encoder Kbps.
            // Read the actual encoder budget, then claim ownership at that same bitrate.
            applyAdaptiveBitrate(0);
            game.bitrateHandler.postDelayed(game.updateBitrate, 1000);
        }
    }

    // The codec sets how many bits a picture of this size needs, which the built-in floor follows.
    private AdaptiveBitrateController.Codec adaptiveBitrateCodec() {
        if (game.decoderRenderer == null) return null;
        int format = game.decoderRenderer.getActiveVideoFormat();
        if ((format & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0) return AdaptiveBitrateController.Codec.AV1;
        if ((format & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0) return AdaptiveBitrateController.Codec.HEVC;
        return AdaptiveBitrateController.Codec.H264;
    }

    void applyAdaptiveBitrate(int target) {
        AdaptiveBitrateController controller = game.adaptiveBitrate;
        PyroWaveBitrateController pyroController = game.pyroWaveBitrate;
        game.hostActionInProgress = true;
        new Thread(() -> {
            try {
                ComputerDetails details = target == 0 ? game.conn.refreshHostDetails() : null;
                if (details != null && (details.encoderBitrateKbps <= 0 || details.initialEncoderBitrateKbps <= 0)) {
                    // A host without the bitrate fields will not start sending them on a retry.
                    throw details.encoderBitrateSupported ?
                            new IOException("Host did not report the stream's encoder bitrate") :
                            new EncoderBitrateUnsupported();
                }
                // Claim bitrate ownership on hosts with their own controller, without probing a new rate.
                int applied = game.conn.setBitrate(details == null ? target : details.encoderBitrateKbps);
                game.runOnUiThread(() -> {
                    if (game.connected && !game.isFinishing()) {
                        if (game.adaptiveBitrate == controller && game.pyroWaveBitrate == pyroController) {
                            game.currentBitrate = applied;
                            game.adaptiveBitrateRetryAt = 0;
                            adaptiveBitrateClaimFailures = 0;
                            long now = SystemClock.uptimeMillis();
                            if (details != null) {
                                // The bitrate the stream started at (the bitrate setting, after profiles) is the
                                // ceiling; the built-in floor applies.
                                int ceiling = Math.max(details.initialEncoderBitrateKbps, applied);
                                if (controller != null) game.adaptiveBitrate = new AdaptiveBitrateController(ceiling, applied, game.streamFps, now,
                                        game.prefConfig.width, game.prefConfig.height, adaptiveBitrateCodec());
                                if (pyroController != null) game.pyroWaveBitrate = new PyroWaveBitrateController(ceiling, applied, game.streamFps, now);
                            } else {
                                if (controller != null) controller.applied(target, applied, now);
                                if (pyroController != null) pyroController.applied(target, applied, now);
                            }
                        }
                    }
                });
            } catch (IOException | XmlPullParserException e) {
                LimeLog.warning("Automatic bitrate update failed: " + e);
                game.runOnUiThread(() -> {
                    if (game.isFinishing() || game.isDestroyed()) return;
                    if (game.adaptiveBitrate == controller && game.pyroWaveBitrate == pyroController) {
                        if (target == 0 && (e instanceof EncoderBitrateUnsupported ||
                                ++adaptiveBitrateClaimFailures >= MAX_BITRATE_CLAIM_FAILURES)) {
                            // Without the encoder's bitrate there is nothing to adapt from: stop
                            // retrying for this stream and leave the PC's saved choice alone.
                            LimeLog.warning("Automatic bitrate: the host reports no encoder bitrate; off for this stream");
                            game.qualityAutoBitrate = false;
                            setAdaptiveBitrateEnabled(false, false);
                            if (game.foreground && game.connected && !game.isFinishing()) {
                                game.messages.alert(R.string.stream_auto_bitrate_unavailable, Toast.LENGTH_LONG);
                            }
                            return;
                        }
                        boolean firstFailure = game.adaptiveBitrateRetryAt == 0;
                        game.adaptiveBitrateRetryTarget = target;
                        game.adaptiveBitrateRetryAt = SystemClock.uptimeMillis() + 5000;
                        if (controller != null) controller.suspend();
                        if (pyroController != null) pyroController.suspend();
                        if (firstFailure && game.foreground && game.connected && !game.isFinishing()) {
                            game.messages.alert(R.string.stream_auto_bitrate_failed, Toast.LENGTH_LONG);
                        }
                    }
                });
            } finally {
                game.runOnUiThread(() -> game.hostActionInProgress = false);
            }
        }, "Automatic bitrate").start();
    }

    private static final class EncoderBitrateUnsupported extends IOException {
        EncoderBitrateUnsupported() {
            super("Host does not support runtime bitrate changes");
        }
    }

    void showHostActionError(Exception error, int generation) {
        game.runOnUiThread(() -> {
            if (game.foreground && generation == game.foregroundGeneration && game.connected && !game.isFinishing()) {
                game.messages.alert(game.getString(R.string.stream_host_action_failed, error.getMessage()), Toast.LENGTH_LONG);
            }
        });
    }
}
