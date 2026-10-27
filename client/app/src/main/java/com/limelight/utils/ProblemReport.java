package com.limelight.utils;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.Intent;
import android.net.Uri;
import android.os.Build;
import android.widget.Toast;

import androidx.core.content.FileProvider;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.jni.StreamEngine;
import com.limelight.ui.NavigationDialogBuilder;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;

public final class ProblemReport {
    private static volatile String decoderDiagnostics = "No decoder configured in this app process.";
    private static volatile String audioDiagnostics = "No audio output configured in this app process.";

    public static void setDecoderDiagnostics(String diagnostics) {
        // Only local codec capabilities/settings belong here, never host replies or stream metadata.
        decoderDiagnostics = diagnostics;
    }

    public static void setAudioDiagnostics(String diagnostics) {
        audioDiagnostics = diagnostics;
    }

    /** The engine of the last stream, and why the C engine ran if the Rust engine was requested. */
    static String engineSection() {
        return "Streaming engine for the last stream:\n" + StreamEngine.summary() + "\n\n";
    }

    public static void show(Activity activity) {
        new NavigationDialogBuilder(activity)
                .setTitle(R.string.report_problem)
                .setMessage(R.string.report_problem_details)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.report_problem_share, (dialog, which) -> share(activity))
                .show();
    }

    static void share(Activity activity) {
        try {
            File directory = new File(activity.getCacheDir(), "reports");
            if (!directory.isDirectory() && !directory.mkdirs()) {
                throw new IOException("Cannot create report directory");
            }
            File report = new File(directory, "nightjar-problem.txt");
            String text = BuildConfig.BRAND_NAME + " " + BuildConfig.VERSION_NAME + " / Android API " + Build.VERSION.SDK_INT
                    + "\nRedacted event log for this app process, oldest first (up to 200 entries)."
                    + "\nAddresses, user-assigned device/host names, PINs, credentials and free-text diagnostic details are omitted.\n\n"
                    + engineSection()
                    + "Last decoder configuration:\n" + decoderDiagnostics + "\n\n"
                    + "Last audio configuration:\n" + audioDiagnostics + "\n\n"
                    + LimeLog.getRedactedLog() + "\n\nLast crash record:\n" + CrashCapture.lastReport(activity);
            try (FileOutputStream output = new FileOutputStream(report)) {
                output.write(text.getBytes(StandardCharsets.UTF_8));
            }
            Uri uri = FileProvider.getUriForFile(activity, BuildConfig.APPLICATION_ID + ".reports", report);
            Intent intent = new Intent(Intent.ACTION_SEND).setType("text/plain")
                    .putExtra(Intent.EXTRA_STREAM, uri)
                    .putExtra(Intent.EXTRA_SUBJECT, activity.getString(R.string.report_problem))
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
            intent.setClipData(ClipData.newRawUri("Problem report", uri));
            activity.startActivity(Intent.createChooser(intent, activity.getString(R.string.report_problem_share)));
        } catch (IOException | ActivityNotFoundException e) {
            Toast.makeText(activity, R.string.report_problem_failed, Toast.LENGTH_LONG).show();
        }
    }
}
