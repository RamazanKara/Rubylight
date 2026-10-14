package com.limelight.utils;

import android.app.Activity;
import androidx.appcompat.app.AlertDialog;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
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
        dialog = new MaterialAlertDialogBuilder(activity)
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
        dialog = new MaterialAlertDialogBuilder(activity)
                .setTitle(R.string.connection_test_title)
                .setMessage(R.string.connection_test_running)
                .setView(progress)
                .setNegativeButton(android.R.string.cancel, (d, which) -> cancel())
                .setOnCancelListener(d -> cancel()).create();
        dialog.show();
        com.limelight.LimeLog.info("Connection test started");
        worker = new Thread(() -> {
            ConnectionTestStats stats = new ConnectionTestStats();
            String bandwidth = activity.getString(R.string.connection_test_no_bandwidth);
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
                if (computer.serverCert != null && !Double.isNaN(stats.latencyMs())) {
                    ComputerDetails current = http.getComputerDetails(true);
                    if (current.supportsPyroWaveBandwidthProbe()) {
                        double mbps = http.probePyroWaveBandwidth(current, percent -> activity.runOnUiThread(() -> {
                            if (!cancelled && !activity.isFinishing() && !activity.isDestroyed()) {
                                progress.setIndeterminate(false);
                                progress.setProgressCompat(percent, true);
                            }
                        }));
                        bandwidth = activity.getString(R.string.bandwidth_probe_result, mbps);
                        if (current.pyroWaveHostLinkMbps > 0) {
                            bandwidth += "\n" + activity.getString(R.string.bandwidth_probe_link, current.pyroWaveHostLinkMbps);
                        }
                    }
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;
            } catch (IOException | XmlPullParserException e) {
                bandwidth = activity.getString(R.string.connection_test_failed);
            }
            com.limelight.LimeLog.info("Connection test finished");
            final String message = activity.getString(R.string.connection_test_stats,
                    measurement(stats.latencyMs()), measurement(stats.jitterMs()), measurement(stats.lossPercent()))
                    + (measuresSpeed() ? " · " + bandwidth : "");
            activity.runOnUiThread(() -> {
                if (cancelled || activity.isFinishing() || activity.isDestroyed()) {
                    return;
                }
                dialog.dismiss();
                dialog = new MaterialAlertDialogBuilder(activity)
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

    private String measurement(double value) {
        return Double.isNaN(value) ? activity.getString(R.string.connection_test_unavailable) :
                String.format(java.util.Locale.getDefault(), "%.1f", value);
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
