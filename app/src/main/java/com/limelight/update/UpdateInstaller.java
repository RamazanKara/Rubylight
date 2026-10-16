package com.limelight.update;

import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.pm.InstallSourceInfo;
import android.content.pm.PackageInstaller;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Build;
import android.provider.Settings;

import com.limelight.BuildConfig;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

/** Hands a verified APK to the PackageInstaller; Android asks for confirmation unless it can skip it. */
public final class UpdateInstaller {
    static final String ACTION_STATUS = BuildConfig.APPLICATION_ID + ".UPDATE_STATUS";

    private UpdateInstaller() {
    }

    /** The package that installed this app, or null when unknown (adb, file manager on older Android). */
    public static String installerPackage(Context context) {
        PackageManager pm = context.getPackageManager();
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                InstallSourceInfo info = pm.getInstallSourceInfo(context.getPackageName());
                return info.getInstallingPackageName();
            }
            return pm.getInstallerPackageName(context.getPackageName());
        } catch (Exception e) {
            return null;
        }
    }

    /** False when installed from Google Play or when this build is not distributed through GitHub. */
    public static boolean isAvailable(Context context) {
        return UpdatePolicy.isSelfUpdateAvailable(installerPackage(context), BuildConfig.GITHUB_UPDATES);
    }

    /** Android 8+ lets each app install packages only after the user allows it. */
    public static boolean needsInstallPermission(Context context) {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && !context.getPackageManager().canRequestPackageInstalls();
    }

    public static Intent installPermissionSettings(Context context) {
        return new Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:" + context.getPackageName()));
    }

    /** Streams the APK into an install session and commits it. The result arrives at UpdateInstallReceiver. */
    public static void install(Context context, File apk) throws IOException {
        PackageInstaller installer = context.getPackageManager().getPackageInstaller();
        PackageInstaller.SessionParams params = new PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // Lets later updates install without a prompt where Android allows it.
            params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED);
        }
        int sessionId = installer.createSession(params);
        try (PackageInstaller.Session session = installer.openSession(sessionId)) {
            try (InputStream in = new FileInputStream(apk); OutputStream out = session.openWrite("nightjar.apk", 0, apk.length())) {
                byte[] buffer = new byte[64 * 1024];
                int read;
                while ((read = in.read(buffer)) > 0) {
                    out.write(buffer, 0, read);
                }
                session.fsync(out);
            }
            Intent status = new Intent(context, UpdateInstallReceiver.class).setAction(ACTION_STATUS);
            // The installer adds the result to this intent, so it must be mutable.
            int flags = PendingIntent.FLAG_UPDATE_CURRENT | (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ? PendingIntent.FLAG_MUTABLE : 0);
            session.commit(PendingIntent.getBroadcast(context, sessionId, status, flags).getIntentSender());
        } catch (IOException | RuntimeException e) {
            installer.abandonSession(sessionId);
            throw e;
        }
    }
}
