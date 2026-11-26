package com.limelight;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.PersistableBundle;
import android.os.SystemClock;
import android.widget.TextView;
import android.widget.Toast;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.binding.video.LatencyProbe;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.http.ClipboardImageException;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.utils.ClipboardImages;
import com.limelight.utils.ServerHelper;

import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;
import java.util.Locale;

/**
 * Host-side remote actions and diagnostics invoked from {@link GameMenuActions}: host audio
 * toggles, native couch menu, host web UI and file transfer prompts, microphone notices, stream
 * health and latency tests, host status, USB forwarding, and text/image clipboard transfers.
 */
final class GameHostActions {
    private final Game game;
    private final GameMenuActions menu;

    // Night mode and headphone surround as the host last answered, for the stream menu; null until it does. UI thread.
    private com.limelight.nativeapi.NativeAudio hostAudio;
    // The host offers the switches but its answer didn't come: the menu says so instead of leaving them out silently.
    private boolean hostAudioUnread;
    private boolean hostAudioBusy;
    private com.limelight.console.NativeMenuController nativeMenuController;
    private final com.limelight.binding.audio.MicNotice.Once microphoneNotice = new com.limelight.binding.audio.MicNotice.Once();
    private boolean latencyTestRunning;
    // Suspend game and Resume game in the stream menu, on a host that offers Quick Suspend.
    final com.limelight.ui.StreamMenuSuspendSection quickSuspend;

    GameHostActions(Game game, GameMenuActions menu) {
        this.game = game;
        this.menu = menu;
        this.quickSuspend = new com.limelight.ui.StreamMenuSuspendSection(game, new QuickSuspendHost());
    }

    // Quick Suspend's reads and requests share the native menu thread with the audio switches and the Radeon panel.
    private final class QuickSuspendHost implements com.limelight.ui.StreamMenuSuspendSection.Host {
        @Override public boolean offered() {
            return game.conn != null && game.app.getRole() == NvApp.Role.STREAM && game.conn.offersQuickSuspend();
        }

        @Override public com.limelight.nativeapi.NativeApiClient client() {
            return game.conn == null ? null : game.conn.nativeApiClient();
        }

        @Override public boolean background(Runnable task) {
            try {
                nativeMenuThread().execute(task);
                return true;
            } catch (java.util.concurrent.RejectedExecutionException e) {
                // The stream is over.
                return false;
            }
        }

        // say() checks that the stream still shows; the state an answer brings stays right whatever happened meanwhile.
        @Override public void ui(Runnable task) {
            game.runOnUiThread(() -> {
                if (!game.isFinishing()) task.run();
            });
        }

        @Override public void say(CharSequence text) {
            if (game.foreground && game.connected && !game.isFinishing()) game.messages.alert(text, Toast.LENGTH_LONG);
        }

        @Override public void refreshMenu() {
            menu.refreshStreamMenu();
        }
    }

    void resetHostAudio() {
        hostAudio = null;
        hostAudioUnread = false;
    }

    com.limelight.nativeapi.NativeAudio hostAudio() {
        return hostAudio;
    }

    boolean hostAudioUnread() {
        return hostAudioUnread;
    }

    private boolean offersHostAudio() {
        return game.conn != null && game.app.getRole() == NvApp.Role.STREAM &&
                (game.conn.offersHostAudio(com.limelight.nativeapi.NativeCapabilities.NIGHT_MODE) ||
                        game.conn.offersHostAudio(com.limelight.nativeapi.NativeCapabilities.HEADPHONE_SURROUND));
    }

    // Reads the host's audio switches on the native menu thread; the open menu shows them once it answers.
    void refreshHostAudio() {
        if (!offersHostAudio()) return;
        NvConnection connection = game.conn;
        int generation = game.foregroundGeneration;
        try {
            nativeMenuThread().execute(() -> {
                com.limelight.nativeapi.NativeAudio audio = null;
                try {
                    audio = connection.hostAudio();
                } catch (IOException e) {
                    LimeLog.warning("Host audio not read: " + e.getMessage());
                }
                com.limelight.nativeapi.NativeAudio answer = audio;
                game.runOnUiThread(() -> showHostAudio(answer, generation));
            });
        } catch (java.util.concurrent.RejectedExecutionException ignored) {
            // The stream is over.
        }
    }

    private void showHostAudio(com.limelight.nativeapi.NativeAudio audio, int generation) {
        if (generation != game.foregroundGeneration || game.isFinishing()) return;
        hostAudio = audio;
        hostAudioUnread = audio == null;
        menu.refreshStreamMenu();
    }

    /**
     * A quick toggle for one of the host's audio switches: the host applies it to this stream from its next audio
     * packet, and the menu shows the host's answer rather than guessing. A refusal is shown with the host's message.
     */
    void toggleHostAudio(String id) {
        com.limelight.nativeapi.NativeAudio audio = hostAudio;
        com.limelight.nativeapi.NativeAudio.Setting setting = audio == null ? null : audio.get(id);
        if (setting == null || !setting.switchable() || hostAudioBusy || game.conn == null) return;
        boolean on = !setting.value;
        NvConnection connection = game.conn;
        int generation = game.foregroundGeneration;
        hostAudioBusy = true;
        try {
            nativeMenuThread().execute(() -> {
                com.limelight.nativeapi.NativeAudio answer = null;
                Throwable failure = null;
                try {
                    answer = connection.writeHostAudio(id, on);
                } catch (IOException | RuntimeException e) {
                    failure = e;
                }
                com.limelight.nativeapi.NativeAudio changed = answer;
                Throwable refused = failure;
                game.runOnUiThread(() -> {
                    hostAudioBusy = false;
                    if (refused != null || changed == null) {
                        LimeLog.warning("Host audio not changed: " + (refused != null ? refused.getMessage() : "no reply"));
                        if (game.foreground && generation == game.foregroundGeneration && game.connected && !game.isFinishing()) {
                            String msg = refused != null && refused.getMessage() != null ? refused.getMessage() : "";
                            game.messages.alert(game.getString(R.string.stream_audio_failed, msg), Toast.LENGTH_LONG);
                        }
                        // Show what the host has instead of the switch's old state.
                        refreshHostAudio();
                        return;
                    }
                    showHostAudio(changed, generation);
                    com.limelight.nativeapi.NativeAudio.Setting now = changed.get(id);
                    // Why the stream doesn't apply it yet, in the host's words unless it is just the next stream.
                    String note = now == null || now.reason == null ? null :
                            com.limelight.nativeapi.NativeAudio.NEXT_STREAM.equals(now.reason) ?
                                    game.getString(R.string.stream_audio_next_stream) : now.message;
                    if (on && note != null && game.foreground && generation == game.foregroundGeneration) {
                        game.messages.alert(note, Toast.LENGTH_LONG);
                    }
                });
            });
        } catch (java.util.concurrent.RejectedExecutionException ignored) {
            // The stream is over.
            hostAudioBusy = false;
        }
    }

    // The Radeon panel's reads and writes: one background thread, never the render, decode or input threads.
    com.limelight.console.NativeMenuController nativeMenuController() {
        if (game.conn == null || !game.conn.hasNativeSessionActions() || game.app.getRole() != NvApp.Role.STREAM) {
            return null;
        }
        if (nativeMenuController == null) {
            java.util.concurrent.ScheduledExecutorService thread = nativeMenuThread();
            Handler main = new Handler(Looper.getMainLooper());
            nativeMenuController = new com.limelight.console.NativeMenuController(game.conn::nativeApiClient,
                    (task, delayMs) -> {
                        try {
                            thread.schedule(task, delayMs, java.util.concurrent.TimeUnit.MILLISECONDS);
                        } catch (java.util.concurrent.RejectedExecutionException ignored) {
                            // The stream is over.
                        }
                    }, main::post);
        }
        return nativeMenuController;
    }

    private java.util.concurrent.ScheduledExecutorService nativeMenuThread() {
        if (game.nativeMenuThread == null) {
            game.nativeMenuThread = java.util.concurrent.Executors.newSingleThreadScheduledExecutor(task -> {
                Thread thread = new Thread(task, "Native menu");
                thread.setDaemon(true);
                return thread;
            });
        }
        return game.nativeMenuThread;
    }

    /**
     * Opens the menu the host draws into the stream. Like every menu action it runs once Nightjar's own sheet has
     * closed, so input reaches the stream (and the host's menu) again; the request runs on the native menu thread and
     * a refusal is shown with the host's message.
     */
    void openHostMenu() {
        if (game.conn == null || !game.connected) return;
        NvConnection connection = game.conn;
        int generation = game.foregroundGeneration;
        try {
            nativeMenuThread().execute(() -> {
                try {
                    connection.openHostMenu();
                } catch (IOException e) {
                    LimeLog.warning("Host menu not opened: " + e.getMessage());
                    game.adaptation.showHostActionError(e, generation);
                }
            });
        } catch (java.util.concurrent.RejectedExecutionException ignored) {
            // The stream is over.
        }
    }

    // The host web UI's address for this stream: the address and HTTPS port the stream connected to.
    okhttp3.HttpUrl hostWebUiUrl() {
        ComputerDetails details = game.conn.getHostDetails();
        String address = game.getIntent().getStringExtra(Game.EXTRA_HOST);
        if (details == null || address == null) return null;
        ComputerDetails target = new ComputerDetails();
        target.state = ComputerDetails.State.ONLINE;
        target.caps = details.caps;
        target.activeAddress = new ComputerDetails.AddressTuple(address,
                game.getIntent().getIntExtra(Game.EXTRA_PORT, NvHTTP.DEFAULT_HTTP_PORT));
        target.httpsPort = details.httpsPort != 0 ? details.httpsPort : game.getIntent().getIntExtra(Game.EXTRA_HTTPS_PORT, 0);
        return ServerHelper.getHostSettingsUrl(target);
    }

    void showHostWebUi() {
        okhttp3.HttpUrl url = hostWebUiUrl();
        if (url == null) return;
        AlertDialog dialog = new NavigationDialogBuilder(game).setTitle(R.string.host_administration_menu)
                .setMessage(game.getString(R.string.host_administration_help, url.toString()) + "\n\n" +
                        game.getString(R.string.stream_web_ui_leaves))
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.host_administration_open, (prompt, which) -> game.streamMenuAction = () -> {
                    try {
                        game.startActivity(new Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url.toString())));
                    } catch (android.content.ActivityNotFoundException | SecurityException e) {
                        game.messages.alert(R.string.host_administration_no_browser, Toast.LENGTH_LONG);
                    }
                }).create();
        menu.showStreamDialog(dialog);
        TextView message = dialog.findViewById(android.R.id.message);
        if (message != null) message.setTextIsSelectable(true);
    }

    /** The PC's file transfer screen; it opens over the stream, so the menu says it leaves it first. */
    void showFiles() {
        String uuid = game.getIntent().getStringExtra(Game.EXTRA_PC_UUID);
        if (uuid == null) return;
        menu.showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.files_menu)
                .setMessage(R.string.files_stream_leaves)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.files_open, (prompt, which) -> game.streamMenuAction = () ->
                        game.startActivity(com.limelight.files.FileTransferActivity.forPc(game, uuid)))
                .create());
    }

    // The microphone settings ask for processing this device lacks: say so in the stream, once per difference.
    // A microphone that failed and was muted is said every time, since each unmute tries again.
    void showMicrophoneNotice() {
        if (game.conn == null || game.isFinishing() || game.isDestroyed()) return;
        if (game.conn.takeMicrophoneFailure()) {
            game.messages.alert(game.getString(R.string.mic_failed), Toast.LENGTH_LONG);
            if (game.micButton != null) game.micButton.refresh();
        }
        String text = microphoneNotice.next(com.limelight.binding.audio.MicNotice.text(game, game.conn.takeMicrophoneNotice()));
        if (text != null) game.messages.alert(text, Toast.LENGTH_LONG);
    }

    void toggleMicrophoneMute() {
        if (game.conn == null) return;
        game.conn.setMicrophoneMuted(!game.conn.isMicrophoneMuted());
        if (game.micButton != null) game.micButton.refresh();
    }

    void showStreamHealth() {
        com.limelight.utils.StreamHealth.Report report = game.streamHealth.report(SystemClock.uptimeMillis());
        StringBuilder text = Game.healthText(game, report, wifiText(0));
        boolean manual = Game.healthTrialAllowed(game.adaptation.isAdaptiveBitrateEnabled(), game.adaptation.isRateChangeBitrateOn(), game.adaptiveQuality != null);
        boolean undo = game.healthOriginalBitrate > 0 && game.currentBitrate == game.healthTrialBitrate && manual;
        boolean suggest = report.canSuggestBitrate() && game.adaptation.supportsAdaptiveBitrate() && manual &&
                game.currentBitrate > 500 && !game.hostActionInProgress && !undo;
        if (undo || suggest) text.append("\n\n").append(game.getString(R.string.health_trial_help));
        MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(game).setTitle(R.string.health_title)
                .setMessage(text).setNegativeButton(android.R.string.cancel, null)
                .setNeutralButton(R.string.overlay_copy, (dialog, which) -> {
                    ClipboardManager clipboard = (ClipboardManager) game.getSystemService(Context.CLIPBOARD_SERVICE);
                    clipboard.setPrimaryClip(ClipData.newPlainText(game.getString(R.string.health_title), text));
                });
        if (undo || suggest) builder.setPositiveButton(undo ? R.string.health_undo : R.string.health_try,
                (dialog, which) -> game.adaptation.applyHealthBitrate(undo));
        else builder.setPositiveButton(android.R.string.ok, null);
        menu.showStreamDialog(builder.create());
    }

    String wifiText(long stallPeriodMs) {
        com.limelight.utils.WifiDiagnostics.Lock lock = Build.VERSION.SDK_INT < Build.VERSION_CODES.Q ?
                com.limelight.utils.WifiDiagnostics.Lock.UNSUPPORTED :
                game.lowLatencyWifiLock != null && game.lowLatencyWifiLock.isHeld() ?
                        com.limelight.utils.WifiDiagnostics.Lock.HELD : com.limelight.utils.WifiDiagnostics.Lock.NOT_HELD;
        return com.limelight.utils.WifiDiagnostics.describe(game,
                game.wifiDiagnostics != null ? game.wifiDiagnostics.latest() : null, lock, stallPeriodMs);
    }

    // Nudges the host mouse back and forth on a still screen and times each nudge until its frame is shown.
    void runLatencyTest() {
        if (latencyTestRunning || !game.connected) return;
        latencyTestRunning = true;
        game.messages.alert(R.string.latency_test_running, Toast.LENGTH_LONG);
        LatencyProbe probe = game.decoderRenderer.getLatencyProbe();
        NvConnection connection = game.conn;
        new Thread(() -> {
            final int probes = 15;
            long[] samples = new long[probes];
            int count = 0;
            short step = 8;
            long giveUpNs = System.nanoTime() + 20_000_000_000L;
            try {
                while (count < probes && game.connected && System.nanoTime() < giveUpNs) {
                    if (!probe.isIdle(System.nanoTime())) {
                        Thread.sleep(10);
                        continue;
                    }
                    probe.arm(System.nanoTime());
                    connection.sendMouseMove(step, (short) 0);
                    step = (short) -step;
                    long ns = probe.await(500);
                    if (ns > 0) samples[count++] = ns;
                }
            } catch (InterruptedException ignored) {
            }
            int measured = count;
            float[] result = LatencyProbe.summarize(samples, measured);
            game.runOnUiThread(() -> {
                latencyTestRunning = false;
                if (!game.foreground || !game.connected || game.isFinishing() || game.streamMenu != null) return;
                String message = result == null || measured < 5 ? game.getString(R.string.latency_test_failed) :
                        game.getString(R.string.latency_test_result, result[1], result[0], result[2], measured);
                LimeLog.info("Latency test: " + message);
                menu.showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.latency_test)
                        .setMessage(message).setPositiveButton(android.R.string.ok, null).create());
            });
        }, "Latency test").start();
    }

    void refreshHostStatus() {
        if (game.hostActionInProgress) {
            return;
        }
        final int generation = game.foregroundGeneration;
        final Object actionToken = GameAdaptation.beginHostAction(game);
        new Thread(() -> {
            try {
                ComputerDetails details = game.conn.refreshHostDetails();
                game.runOnUiThread(() -> {
                    if (!game.foreground || generation != game.foregroundGeneration || !game.connected || game.isFinishing() || game.streamMenu != null) {
                        return;
                    }
                    showHostStatus(details);
                });
            } catch (IOException | XmlPullParserException | RuntimeException e) {
                game.adaptation.showHostActionError(e, generation);
            } finally {
                GameAdaptation.endHostAction(game, actionToken);
            }
        }, "Host status").start();
    }

    // The devices plugged into this one that may go to the PC; picking a forwarded one detaches it.
    void showUsbDevices() {
        if (!game.connected) return;
        com.limelight.binding.usb.UsbForwarding forwarding = usbForwarding();
        java.util.List<android.hardware.usb.UsbDevice> devices = forwarding.candidates();
        com.google.android.material.dialog.MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(game)
                .setTitle(R.string.usb_forwarding_pick_title);
        if (devices.isEmpty()) {
            builder.setMessage(R.string.usb_forwarding_none).setPositiveButton(android.R.string.ok, null);
        } else {
            CharSequence[] labels = new CharSequence[devices.size()];
            for (int i = 0; i < labels.length; i++) labels[i] = forwarding.label(devices.get(i));
            builder.setItems(labels, (dialog, which) -> forwarding.choose(devices.get(which)))
                    .setNeutralButton(R.string.usb_forwarding_auto_button, (dialog, which) -> showUsbAuto(devices))
                    .setNegativeButton(android.R.string.cancel, null);
        }
        menu.showStreamDialog(builder.create());
    }

    // Ticks the devices that go to the PC whenever they are plugged in during a stream.
    private void showUsbAuto(java.util.List<android.hardware.usb.UsbDevice> devices) {
        if (!game.connected) return;
        com.limelight.binding.usb.UsbForwarding forwarding = usbForwarding();
        CharSequence[] labels = new CharSequence[devices.size()];
        boolean[] ticked = new boolean[devices.size()];
        for (int i = 0; i < labels.length; i++) {
            labels[i] = com.limelight.binding.usb.UsbForwarding.name(devices.get(i));
            ticked[i] = forwarding.isAuto(devices.get(i));
        }
        menu.showStreamDialog(new NavigationDialogBuilder(game)
                .setTitle(R.string.usb_forwarding_auto_title)
                .setMultiChoiceItems(labels, ticked, (dialog, which, on) -> ticked[which] = on)
                .setPositiveButton(android.R.string.ok, (dialog, which) -> forwarding.setAuto(devices, ticked))
                .setNegativeButton(android.R.string.cancel, null)
                .create());
    }

    /** This stream's USB forwarding, made the first time it is needed. */
    com.limelight.binding.usb.UsbForwarding usbForwarding() {
        if (game.usbForwarding == null) {
            game.usbForwarding = new com.limelight.binding.usb.UsbForwarding(game, game.prefConfig, game.conn,
                    text -> game.messages.alert(text, Toast.LENGTH_LONG));
        }
        return game.usbForwarding;
    }

    /** Once the PC is known to take this device's USB devices: forwards the ones ticked to go automatically. */
    void startUsbAutoForwarding() {
        if (!game.connected || game.isFinishing() || !game.prefConfig.usbForwarding
                || !game.conn.offersUsbForwarding()) {
            return;
        }
        usbForwarding().startAuto();
    }

    void transferClipboard(boolean send) {
        if (game.hostActionInProgress || !game.foreground || !game.connected) {
            return;
        }
        ClipboardManager clipboard = (ClipboardManager) game.getSystemService(Context.CLIPBOARD_SERVICE);
        String text = null;
        if (send) {
            final ClipData clip;
            try {
                clip = clipboard.getPrimaryClip();
            } catch (RuntimeException e) {
                game.adaptation.showHostActionError(e, game.foregroundGeneration);
                return;
            }
            if (clip == null || clip.getItemCount() == 0 || clip.getItemAt(0).getText() == null) {
                game.messages.alert(R.string.stream_clipboard_empty, Toast.LENGTH_SHORT);
                return;
            }
            text = clip.getItemAt(0).getText().toString();
        }
        final String outgoing = text;
        final int generation = game.foregroundGeneration;
        final Object actionToken = GameAdaptation.beginHostAction(game);
        new Thread(() -> {
            try {
                if (send) {
                    game.conn.sendClipboard(outgoing);
                }
                String incoming = send ? null : game.conn.getClipboard();
                game.runOnUiThread(() -> {
                    // Window focus is not asked: a toast or the stream menu's dialog holds it while the stream stays.
                    if (!game.foreground || generation != game.foregroundGeneration || !game.connected || game.isFinishing()) {
                        if (!send) {
                            game.messages.alert(R.string.stream_clipboard_not_copied, Toast.LENGTH_LONG);
                        }
                        return;
                    }
                    if (!send) {
                        ClipData clip = ClipData.newPlainText(BuildConfig.BRAND_NAME, incoming);
                        PersistableBundle extras = new PersistableBundle();
                        extras.putBoolean("android.content.extra.IS_SENSITIVE", true);
                        clip.getDescription().setExtras(extras);
                        try {
                            clipboard.setPrimaryClip(clip);
                        } catch (RuntimeException e) {
                            game.adaptation.showHostActionError(e, generation);
                            return;
                        }
                    }
                    game.messages.alert(send ? R.string.stream_clipboard_sent : R.string.stream_clipboard_received, Toast.LENGTH_SHORT);
                });
            } catch (IOException | XmlPullParserException | RuntimeException e) {
                game.adaptation.showHostActionError(e, generation);
            } finally {
                GameAdaptation.endHostAction(game, actionToken);
            }
        }, "Clipboard transfer").start();
    }

    /**
     * Sends the image on this device's clipboard to the host as a PNG, or puts the host's PNG on this device's
     * clipboard as a content URI. Android 13 and later confirm a copy themselves, so only older ones get a toast.
     */
    void transferClipboardImage(boolean send) {
        if (game.hostActionInProgress || !game.foreground || !game.connected) {
            return;
        }
        ClipboardManager clipboard = (ClipboardManager) game.getSystemService(Context.CLIPBOARD_SERVICE);
        ClipData current = null;
        if (send) {
            try {
                current = clipboard.getPrimaryClip();
            } catch (RuntimeException e) {
                game.adaptation.showHostActionError(e, game.foregroundGeneration);
                return;
            }
            if (current == null || current.getItemCount() == 0) {
                game.messages.alert(R.string.stream_clipboard_image_empty, Toast.LENGTH_SHORT);
                return;
            }
        }
        final ClipData outgoing = current;
        final int generation = game.foregroundGeneration;
        final Object actionToken = GameAdaptation.beginHostAction(game);
        new Thread(() -> {
            java.io.File staged = null;
            try {
                if (send) {
                    // The image's type comes from its provider, which may block: never asked on the main thread.
                    android.net.Uri image = ClipboardImages.imageUri(outgoing, game.getContentResolver());
                    if (image == null) {
                        showClipboardImageResult(R.string.stream_clipboard_image_empty, generation);
                        return;
                    }
                    staged = ClipboardImages.readPng(game, image);
                    game.conn.sendClipboardImage(staged);
                    showClipboardImageResult(R.string.stream_clipboard_image_sent, generation);
                    return;
                }
                staged = ClipboardImages.newIncoming(game);
                boolean found;
                try (java.io.OutputStream out = new java.io.FileOutputStream(staged)) {
                    found = game.conn.getClipboardImage(out);
                }
                if (!found) {
                    showClipboardImageResult(R.string.stream_clipboard_image_none, generation);
                    return;
                }
                java.io.File incoming = staged;
                staged = null;
                game.runOnUiThread(() -> placeClipboardImage(clipboard, incoming, generation));
            } catch (ClipboardImageException e) {
                LimeLog.warning("Clipboard image refused: " + e.getMessage());
                showClipboardImageResult(ClipboardImages.message(e.reason, send), generation);
            } catch (OutOfMemoryError e) {
                LimeLog.warning("Clipboard image out of memory: " + e);
                showClipboardImageResult(ClipboardImages.message(ClipboardImageException.Reason.TOO_LARGE, send),
                        generation);
            } catch (IOException | XmlPullParserException | RuntimeException e) {
                game.adaptation.showHostActionError(e, generation);
            } finally {
                if (staged != null) {
                    ClipboardImages.discard(staged);
                }
                GameAdaptation.endHostAction(game, actionToken);
            }
        }, "Clipboard image transfer").start();
    }

    /**
     * Puts the host's image, streamed to {@code image}, on this device's clipboard; the images before it go only once
     * it is there. A stream that closed meanwhile gets it said, not a silent drop.
     */
    private void placeClipboardImage(ClipboardManager clipboard, java.io.File image, int generation) {
        // Window focus is not asked: a toast or the stream menu's dialog holds it while the stream stays.
        if (!game.foreground || generation != game.foregroundGeneration || !game.connected || game.isFinishing()) {
            ClipboardImages.discard(image);
            game.messages.alert(R.string.stream_clipboard_image_not_copied, Toast.LENGTH_LONG);
            return;
        }
        try {
            ClipData clip = ClipData.newUri(game.getContentResolver(), BuildConfig.BRAND_NAME, ClipboardImages.share(game, image));
            PersistableBundle extras = new PersistableBundle();
            extras.putBoolean("android.content.extra.IS_SENSITIVE", true);
            clip.getDescription().setExtras(extras);
            clipboard.setPrimaryClip(clip);
        } catch (RuntimeException e) {
            ClipboardImages.discard(image);
            game.adaptation.showHostActionError(e, generation);
            return;
        }
        ClipboardImages.keepOnly(game, image);
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            game.messages.alert(R.string.stream_clipboard_image_received, Toast.LENGTH_SHORT);
        }
    }

    private void showClipboardImageResult(int message, int generation) {
        game.runOnUiThread(() -> {
            if (game.foreground && generation == game.foregroundGeneration && game.connected && !game.isFinishing()) {
                game.messages.alert(message, Toast.LENGTH_LONG);
            }
        });
    }

    private void showHostStatus(ComputerDetails details) {
        StringBuilder status = new StringBuilder();
        if (details.permission == -1) {
            status.append(game.getString(R.string.stream_permissions_unknown));
        } else {
            String[] names = game.getResources().getStringArray(R.array.host_permission_names);
            int[] masks = ComputerDetails.PERMISSION_DISPLAY_MASKS;
            for (int i = 0; i < names.length; i++) {
                status.append(names[i]).append(": ").append(game.getString(details.hasPermission(masks[i]) ?
                        R.string.stream_permission_allowed : R.string.stream_permission_denied)).append('\n');
            }
        }
        if (details.permission != -1) {
            status.append("\n\n").append(game.getString(R.string.stream_permissions_help));
        }
        if (details.caps.has(HostCapabilities.Feature.FRAME_LIMITER_STATUS)) {
            status.append("\n\n").append(game.getString(R.string.stream_limiter_status,
                    game.getString(details.frameLimiterEnabled ? R.string.stream_enabled : R.string.stream_disabled),
                    game.getString(details.virtualDisplayFrameLimiterEnabled ? R.string.stream_enabled : R.string.stream_disabled),
                    details.frameLimiterFpsLimitMilliHz == 0 ? game.getString(R.string.stream_limiter_stream_rate) :
                            String.format(Locale.getDefault(), "%.3f FPS", details.frameLimiterFpsLimitMilliHz / 1000.0)));
        }
        status.append("\n\n").append(game.getString(!game.prefConfig.vrr ? R.string.stream_android_pacing :
                game.useArr ? R.string.stream_android_arr : R.string.stream_android_max_refresh));
        menu.showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.stream_host_status)
                .setMessage(status).setPositiveButton(android.R.string.ok, null).create());
    }
}
