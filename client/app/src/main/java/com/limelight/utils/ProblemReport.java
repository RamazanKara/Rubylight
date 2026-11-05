package com.limelight.utils;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.Build;
import android.widget.Toast;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.files.FileTransferActivity;
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
                // A TV's share sheet may offer no app that takes a file: the report goes to a PC directly.
                .setNeutralButton(R.string.files_share_label, (dialog, which) -> sendToPc(activity))
                .show();
    }

    static void share(Activity activity) {
        try {
            Uri uri = write(activity);
            Intent intent = new Intent(Intent.ACTION_SEND).setType("text/plain")
                    .putExtra(Intent.EXTRA_STREAM, uri)
                    .putExtra(Intent.EXTRA_SUBJECT, activity.getString(R.string.report_problem))
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
            intent.setClipData(ClipData.newRawUri("Problem report", uri));
            Intent chooser = Intent.createChooser(intent, activity.getString(R.string.report_problem_share));
            // Nightjar's own Send to PC takes no file from the share sheet that is Nightjar's: Send to PC above does.
            chooser.putExtra(Intent.EXTRA_EXCLUDE_COMPONENTS,
                    new ComponentName[] {new ComponentName(activity, FileTransferActivity.class)});
            activity.startActivity(chooser);
        } catch (IOException | ActivityNotFoundException e) {
            Toast.makeText(activity, R.string.report_problem_failed, Toast.LENGTH_LONG).show();
        }
    }

    /** Opens the file screen with the report, through the alias that may send it; it asks which PC. */
    static void sendToPc(Activity activity) {
        try {
            activity.startActivity(sendToPcIntent(activity, write(activity)));
        } catch (IOException | ActivityNotFoundException e) {
            Toast.makeText(activity, R.string.report_problem_failed, Toast.LENGTH_LONG).show();
        }
    }

    public static Intent sendToPcIntent(Context context, Uri report) {
        return new Intent(Intent.ACTION_SEND).setClassName(context, FileTransferActivity.REPORT_ALIAS)
                .setType("text/plain").putExtra(Intent.EXTRA_STREAM, report);
    }

    /** The reports provider's root for the report (res/xml/report_paths.xml): cache-path "reports/" named so. */
    static final String REPORT_ROOT = "reports";
    static final String REPORT_NAME = "nightjar-problem.txt";

    private static File reportFile(Context context) {
        return new File(new File(context.getCacheDir(), REPORT_ROOT), REPORT_NAME);
    }

    /**
     * The report's content URI, the one file of the reports provider that Send to PC may send: what
     * FileProvider.getUriForFile gives for {@link #reportFile} (root name, then the file's path under the root),
     * built here because FileProvider cannot place a Windows path under its root in unit tests.
     */
    public static Uri reportUri() {
        return new Uri.Builder().scheme("content").authority(BuildConfig.APPLICATION_ID + ".reports")
                .appendPath(REPORT_ROOT).appendPath(REPORT_NAME).build();
    }

    /** The report's opening lines; the device line names exactly what device-specific fixes are gated on. */
    static String header() {
        return BuildConfig.BRAND_NAME + " " + BuildConfig.VERSION_NAME + " / Android API " + Build.VERSION.SDK_INT
                + "\nDevice: " + Build.MANUFACTURER + " " + Build.MODEL + ", hardware " + Build.HARDWARE
                + "\nRedacted event log for this app process, oldest first (up to " + RedactedLog.MAX_ENTRIES + " entries), times in UTC."
                + "\nAddresses, user-assigned device/host names, PINs, credentials and free-text diagnostic details are omitted.\n\n";
    }

    /** Writes the report to the reports provider's folder and returns its content URI. */
    private static Uri write(Activity activity) throws IOException {
        File report = reportFile(activity);
        File directory = report.getParentFile();
        if (!directory.isDirectory() && !directory.mkdirs()) {
            throw new IOException("Cannot create report directory");
        }
        String text = header()
                + engineSection()
                + "Last decoder configuration:\n" + decoderDiagnostics + "\n\n"
                + "Last audio configuration:\n" + audioDiagnostics + "\n\n"
                + LimeLog.getRedactedLog() + "\n\nLast crash record:\n" + CrashCapture.lastReport(activity);
        try (FileOutputStream output = new FileOutputStream(report)) {
            output.write(text.getBytes(StandardCharsets.UTF_8));
        }
        return reportUri();
    }
}
