package com.limelight.utils;

import android.app.Activity;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.google.android.material.progressindicator.LinearProgressIndicator;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.PlatformBinding;
import com.limelight.nvstream.PyroWavePace;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostHttpResponseException;
import com.limelight.nvstream.http.NvHTTP;

import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;

/**
 * Finds the fastest frame pace a PyroWave host can burst frames to this device without losing their
 * tails, and stores it for the PC (PyroWavePaceStore). Streams then send it to the host as
 * x-ss-video[0].pyrowavePaceMbps. Needs a host with frame-shaped UDP probes (Vibeshine).
 */
public final class PyroWavePaceCalibration {
    /** Stream packet size; probe packets are this size, as the stream's are on a local network. */
    static final int PACKET_SIZE = 1392;

    private final Activity activity;
    private final ComputerDetails computer;
    private final String uniqueId;
    private final int bitrateKbps;
    private final int fps;
    private final Runnable onSaved;
    private volatile boolean cancelled;
    private volatile NvHTTP http;
    private AlertDialog dialog;
    private Thread worker;

    public PyroWavePaceCalibration(Activity activity, ComputerDetails computer, String uniqueId,
                                   int bitrateKbps, int fps, Runnable onSaved) {
        this.activity = activity;
        this.computer = new ComputerDetails(computer);
        this.uniqueId = uniqueId;
        this.bitrateKbps = bitrateKbps;
        this.fps = fps;
        this.onSaved = onSaved;
    }

    public void show() {
        MaterialAlertDialogBuilder builder = new MaterialAlertDialogBuilder(activity)
                .setTitle(R.string.pace_calibration_title)
                .setMessage(R.string.pace_calibration_intro)
                .setPositiveButton(R.string.bandwidth_probe_start, (d, which) -> start())
                .setNegativeButton(android.R.string.cancel, (d, which) -> cancel())
                .setOnCancelListener(d -> cancel());
        if (PyroWavePaceStore.get(activity, computer.uuid) != null) {
            builder.setNeutralButton(R.string.pace_calibration_clear, (d, which) -> {
                PyroWavePaceStore.clear(activity, computer.uuid);
                if (onSaved != null) {
                    onSaved.run();
                }
            });
        }
        dialog = builder.create();
        dialog.show();
    }

    private void start() {
        LinearProgressIndicator progress = new LinearProgressIndicator(activity);
        progress.setIndeterminate(true);
        int padding = Math.round(24 * activity.getResources().getDisplayMetrics().density);
        progress.setPadding(padding, 0, padding, 0);
        dialog = new MaterialAlertDialogBuilder(activity)
                .setTitle(R.string.pace_calibration_title)
                .setMessage(R.string.pace_calibration_preparing)
                .setView(progress)
                .setNegativeButton(android.R.string.cancel, (d, which) -> cancel())
                .setOnCancelListener(d -> cancel()).create();
        dialog.show();
        AlertDialog running = dialog;
        worker = new Thread(() -> {
            String message;
            try {
                message = calibrate(paceKbps -> activity.runOnUiThread(() -> {
                    if (!cancelled && running.isShowing()) {
                        running.setMessage(activity.getString(R.string.pace_calibration_running, paceKbps / 1000));
                    }
                }));
            } catch (InterruptedException | java.io.InterruptedIOException e) {
                return;
            } catch (HostHttpResponseException e) {
                // Vibeshine answers 400 while a stream runs or another probe is in progress.
                LimeLog.warning("Pace calibration: host refused the probe: " + e.getErrorCode());
                message = activity.getString(e.getErrorCode() == 400 ?
                        R.string.pace_calibration_streaming : R.string.pace_calibration_failed);
            } catch (Exception e) {
                LimeLog.warning("Pace calibration failed: " + e);
                message = activity.getString(R.string.pace_calibration_failed);
            }
            if (cancelled) {
                return;
            }
            final String result = message;
            activity.runOnUiThread(() -> {
                if (cancelled || activity.isFinishing() || activity.isDestroyed()) {
                    return;
                }
                dialog.dismiss();
                dialog = new MaterialAlertDialogBuilder(activity)
                        .setTitle(R.string.pace_calibration_title)
                        .setMessage(result)
                        .setPositiveButton(android.R.string.ok, null).create();
                dialog.show();
                if (onSaved != null) {
                    onSaved.run();
                }
            });
        }, "Pace calibration");
        worker.start();
    }

    private interface Progress {
        void testing(int paceKbps);
    }

    private String calibrate(Progress progress) throws Exception {
        ComputerDetails.AddressTuple address = ServerHelper.getCurrentAddressFromComputer(computer);
        http = new NvHTTP(address, computer.httpsPort, uniqueId, computer.serverCert,
                PlatformBinding.getCryptoProvider(activity.getApplicationContext()));
        if (cancelled) {
            http.cancelPendingRequests();
            throw new InterruptedException();
        }
        ComputerDetails current;
        try {
            current = http.getComputerDetails(true);
        } catch (XmlPullParserException e) {
            throw new IOException(e);
        }
        if (!current.supportsPyroWavePaceCalibration()) {
            return activity.getString(R.string.pace_calibration_unsupported);
        }
        int clientLinkMbps = WiredLink.mbpsToward(activity, address.address);
        int hostLinkMbps = (int) Math.min(current.pyroWaveHostLinkMbps, Integer.MAX_VALUE);
        int linkMbps = pathLinkMbps(clientLinkMbps, hostLinkMbps);
        if (linkMbps <= 0) {
            return activity.getString(R.string.pace_calibration_no_link);
        }
        int probeKbps = PyroWavePace.probeKbps(bitrateKbps, fps, PACKET_SIZE,
                current.pyroWaveCriticalFecPercentage, current.pyroWaveMinParityShards);
        int floorKbps = PyroWavePace.floorKbps(probeKbps);
        if (floorKbps >= linkMbps * 1000) {
            return activity.getString(R.string.pace_calibration_too_fast, probeKbps / 1000, linkMbps);
        }
        int probeFps = Math.max(10, Math.min(500, fps));
        LimeLog.info("Pace calibration: link " + linkMbps + " Mbps (this device " + clientLinkMbps +
                ", host " + hostLinkMbps + "), probes " + probeKbps + " kbps at " + probeFps + " FPS");
        PyroWavePace.Outcome outcome = PyroWavePace.search(linkMbps * 1000, floorKbps, paceKbps -> {
            if (cancelled) throw new InterruptedException();
            progress.testing(paceKbps);
            int pace = Math.max(paceKbps, probeKbps);
            PyroWavePace.Result result = http.probePyroWaveUdp(probeKbps, PACKET_SIZE, probeFps, pace);
            LimeLog.info("Pace probe at " + pace + " kbps: " + result +
                    (PyroWavePace.qualified(result) ? " (pass)" : " (fail)"));
            return result;
        }, () -> cancelled);
        if (cancelled) {
            throw new InterruptedException();
        }
        if (outcome.paceKbps <= 0) {
            return activity.getString(R.string.pace_calibration_none);
        }
        int paceMbps = outcome.paceKbps / 1000;
        PyroWavePaceStore.put(activity, computer.uuid,
                new PyroWavePaceStore.Calibration(paceMbps, clientLinkMbps, hostLinkMbps, outcome.qualified));
        LimeLog.info("Pace calibration: " + paceMbps + " Mbps, " + (outcome.qualified ? "confirmed" : "best available"));
        return activity.getString(outcome.qualified ? R.string.pace_calibration_result :
                R.string.pace_calibration_result_unqualified, paceMbps, linkMbps);
    }

    /** The slower known wired link of the two ends; 0 when neither is known. */
    static int pathLinkMbps(int clientLinkMbps, int hostLinkMbps) {
        int link = 0;
        for (int known : new int[] {clientLinkMbps, hostLinkMbps}) {
            if (known > 0) link = link == 0 ? known : Math.min(link, known);
        }
        return Math.min(link, 9999);
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
