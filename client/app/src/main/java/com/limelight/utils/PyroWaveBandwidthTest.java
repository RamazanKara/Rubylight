package com.limelight.utils;

import android.app.Activity;
import android.content.Context;
import androidx.appcompat.app.AlertDialog;
import com.limelight.ui.NavigationDialogBuilder;
import com.google.android.material.progressindicator.LinearProgressIndicator;

import com.limelight.R;
import com.limelight.binding.PlatformBinding;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvHTTP;

import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;

public final class PyroWaveBandwidthTest {
    private final Activity activity;
    private final ComputerDetails computer;
    private final String uniqueId;
    private volatile boolean cancelled;
    private volatile NvHTTP http;
    private AlertDialog dialog;
    private Thread worker;

    public PyroWaveBandwidthTest(Activity activity, ComputerDetails computer, String uniqueId) {
        this.activity = activity;
        this.computer = new ComputerDetails(computer);
        this.uniqueId = uniqueId;
    }

    public void show() {
        dialog = new NavigationDialogBuilder(activity)
                .setTitle(R.string.connection_test_title)
                .setMessage(measuresSpeed() ? R.string.connection_test_intro : R.string.connection_test_intro_delay)
                .setPositiveButton(R.string.bandwidth_probe_start, (d, which) -> start())
                .setNegativeButton(android.R.string.cancel, (d, which) -> cancel())
                .setOnCancelListener(d -> cancel()).create();
        dialog.show();
    }

    private void start() {
        LinearProgressIndicator progress = new LinearProgressIndicator(activity);
        progress.setMax(100);
        progress.setIndeterminate(true);
        int padding = Math.round(24 * activity.getResources().getDisplayMetrics().density);
        progress.setPadding(padding, 0, padding, 0);
        dialog = new NavigationDialogBuilder(activity)
                .setTitle(R.string.connection_test_title)
                .setMessage(R.string.connection_test_running)
                .setView(progress)
                .setNegativeButton(android.R.string.cancel, (d, which) -> cancel())
                .setOnCancelListener(d -> cancel()).create();
        dialog.show();
        com.limelight.LimeLog.info("Connection test started");
        worker = new Thread(() -> {
            ConnectionTestStats stats = new ConnectionTestStats();
            // The second line of the result: the speed, or why it was not measured. Hosts that can
            // never measure speed get no second line.
            String speed = null;
            try {
                http = new NvHTTP(ServerHelper.getCurrentAddressFromComputer(computer), computer.httpsPort,
                        uniqueId, computer.serverCert, PlatformBinding.getCryptoProvider(activity.getApplicationContext()));
                if (cancelled) {
                    http.cancelPendingRequests();
                    return;
                }
                for (int sample = 0; sample < 10 && !cancelled; sample++) {
                    try {
                        stats.add(http.probeServerLatency());
                    } catch (IOException | XmlPullParserException e) {
                        stats.add(Double.NaN);
                    }
                    Thread.sleep(100);
                }
                if (cancelled) {
                    return;
                }
                if (!measuresSpeed()) {
                    speed = null;
                } else if (computer.serverCert == null) {
                    speed = activity.getString(R.string.connection_test_speed_needs_pairing);
                } else if (Double.isNaN(stats.latencyMs())) {
                    speed = activity.getString(R.string.connection_test_speed_no_answer);
                } else {
                    ComputerDetails current = http.getComputerDetails(true);
                    if (current.supportsPyroWaveBandwidthProbe()) {
                        double mbps = http.probePyroWaveBandwidth(current, percent -> activity.runOnUiThread(() -> {
                            if (!cancelled && !activity.isFinishing() && !activity.isDestroyed()) {
                                progress.setIndeterminate(false);
                                progress.setProgressCompat(percent, true);
                            }
                        }));
                        speed = speedText(activity, mbps, current.pyroWaveHostLinkMbps);
                    } else {
                        speed = activity.getString(R.string.connection_test_speed_unsupported);
                    }
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;
            } catch (IOException | XmlPullParserException e) {
                speed = measuresSpeed() ? activity.getString(R.string.connection_test_failed) : null;
            }
            com.limelight.LimeLog.info("Connection test finished");
            final String message = result(activity, stats, speed);
            activity.runOnUiThread(() -> {
                if (cancelled || activity.isFinishing() || activity.isDestroyed()) {
                    return;
                }
                dialog.dismiss();
                dialog = new NavigationDialogBuilder(activity)
                        .setTitle(R.string.connection_test_title)
                        .setMessage(message)
                        .setPositiveButton(android.R.string.ok, null)
                        .setNeutralButton(R.string.pair_try_again, (d, which) -> start()).create();
                dialog.show();
            });
        }, "Connection test");
        worker.start();
    }

    /** Only hosts with the download test measure speed; the others are tested for delay alone. */
    private boolean measuresSpeed() {
        return computer.caps.mayHave(HostCapabilities.Feature.PYROWAVE_BANDWIDTH_PROBE);
    }

    /** "Ping 63 ms · Jitter 41 ms · Loss 0.0%", then the speed line when there is one. */
    static String result(Context context, ConnectionTestStats stats, String speed) {
        String delay = context.getString(R.string.connection_test_stats, milliseconds(context, stats.latencyMs()),
                milliseconds(context, stats.jitterMs()), percent(context, stats.lossPercent()));
        return speed == null ? delay : delay + "\n" + speed;
    }

    static String speedText(Context context, double mbps, long linkMbps) {
        String speed = context.getString(R.string.connection_test_speed, mbps);
        return linkMbps > 0 ? speed + " · " + context.getString(R.string.bandwidth_probe_link, linkMbps) : speed;
    }

    /** Whole milliseconds, with one decimal below 10 ms where the difference still matters. */
    private static String milliseconds(Context context, double value) {
        if (Double.isNaN(value)) {
            return context.getString(R.string.connection_test_unavailable);
        }
        return context.getString(R.string.connection_test_ms,
                String.format(java.util.Locale.getDefault(), value < 10 ? "%.1f" : "%.0f", value));
    }

    private static String percent(Context context, double value) {
        if (Double.isNaN(value)) {
            return context.getString(R.string.connection_test_unavailable);
        }
        return context.getString(R.string.connection_test_percent, String.format(java.util.Locale.getDefault(), "%.1f", value));
    }

    public void cancel() {
        cancelled = true;
        if (worker != null) {
            worker.interrupt();
        }
        NvHTTP request = http;
        if (request != null) {
            request.cancelPendingRequests();
        }
        if (dialog != null) {
            dialog.dismiss();
        }
    }
}
