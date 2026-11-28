package com.limelight.utils;

import android.app.Activity;
import android.app.ActivityManager;
import android.app.Application;
import android.app.ApplicationExitInfo;
import android.content.Context;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.Process;

import androidx.annotation.RequiresApi;
import androidx.appcompat.view.ContextThemeWrapper;

import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.BuildConfig;
import com.limelight.R;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;

public final class CrashCapture implements Application.ActivityLifecycleCallbacks {
    private final Context context;
    private final File directory;
    private final Object lock = new Object();
    private Activity resumedActivity;
    private CrashRecord pending;

    public CrashCapture(Context context) {
        this.context = context;
        directory = context.getNoBackupFilesDir();
    }

    private static final class CapturingHandler implements Thread.UncaughtExceptionHandler {
        final CrashCapture owner;
        final Thread.UncaughtExceptionHandler previous;

        CapturingHandler(CrashCapture owner, Thread.UncaughtExceptionHandler previous) {
            this.owner = owner;
            this.previous = previous;
        }

        @Override
        public void uncaughtException(Thread thread, Throwable exception) {
            try {
                synchronized (owner.lock) {
                    CrashRecord.javaCrash(System.currentTimeMillis(), Process.myPid(),
                            BuildConfig.VERSION_NAME + " (" + BuildConfig.VERSION_CODE + ")", Build.VERSION.SDK_INT,
                            Build.MODEL, thread.getName(), exception).save(owner.directory);
                }
            } catch (Throwable ignored) {
                // Reporting must not prevent Android's crash handler from terminating the process, even on OOM.
            } finally {
                if (previous != null) {
                    previous.uncaughtException(thread, exception);
                } else if (!"robolectric".equals(Build.FINGERPRINT)) {
                    Process.killProcess(Process.myPid());
                    System.exit(10);
                } else {
                    exception.printStackTrace(System.err);
                }
            }
        }
    }

    public void installHandler() {
        Thread.UncaughtExceptionHandler previous = Thread.getDefaultUncaughtExceptionHandler();
        // Unwrap an earlier CrashCapture handler so repeated Application creation in unit tests
        // never builds an unbounded chain pinning every prior Application and Activity in memory.
        while (previous instanceof CapturingHandler) {
            previous = ((CapturingHandler) previous).previous;
        }
        Thread.setDefaultUncaughtExceptionHandler(new CapturingHandler(this, previous));
    }

    public void start(Application application) {
        application.registerActivityLifecycleCallbacks(this);
        Handler main = new Handler(Looper.getMainLooper());
        new Thread(() -> {
            synchronized (lock) {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    collectExitReason();
                }
                try {
                    CrashRecord record = CrashRecord.read(directory);
                    main.post(() -> {
                        pending = record;
                        showPendingReport();
                    });
                } catch (IOException ignored) {
                    // A missing or incomplete crash record must not affect startup.
                }
            }
        }, "Crash history").start();
    }

    @RequiresApi(Build.VERSION_CODES.R)
    void collectExitReason() {
        ActivityManager manager = (ActivityManager) context.getSystemService(Context.ACTIVITY_SERVICE);
        try {
            CrashRecord last = CrashRecord.read(directory);
            for (ApplicationExitInfo exit : manager.getHistoricalProcessExitReasons(null, 0, 16)) {
                if (last != null && exit.getTimestamp() <= last.timestamp) {
                    break;
                }
                String reason = CrashRecord.exitReason(exit.getReason());
                if (reason == null || !context.getPackageName().equals(exit.getProcessName())) {
                    continue;
                }
                String trace = "";
                if (exit.getReason() == ApplicationExitInfo.REASON_CRASH_NATIVE) {
                    trace = "Tombstone unavailable.\n";
                    // API 30 only supplies ANR text, even when the eventual exit is a native crash.
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                        try (InputStream input = exit.getTraceInputStream()) {
                            if (input != null) {
                                trace = RedactedLog.nativeTrace(input);
                            }
                        } catch (IOException | RuntimeException ignored) {
                            // Tombstones can be evicted independently of the exit history.
                        }
                    }
                } else if (exit.getReason() == ApplicationExitInfo.REASON_ANR) {
                    // Where the main thread hung, which the record otherwise lacks (#73).
                    trace = "ANR trace unavailable.\n";
                    try (InputStream input = exit.getTraceInputStream()) {
                        if (input != null) {
                            trace = AnrTrace.redact(input);
                        }
                    } catch (IOException | RuntimeException ignored) {
                        // The trace can be evicted independently of the exit history.
                    }
                }
                CrashRecord.create(exit.getTimestamp(), exit.getPid(), reason,
                        BuildConfig.VERSION_NAME + " (" + BuildConfig.VERSION_CODE + ", at collection)",
                        Build.VERSION.SDK_INT, Build.MODEL, trace).save(directory);
                break;
            }
        } catch (IOException | RuntimeException ignored) {
            // Exit history is best effort on devices that do not retain it.
        }
    }

    private void showPendingReport() {
        Activity activity = resumedActivity;
        if (pending == null || activity == null || activity.isFinishing() || activity.isDestroyed()) {
            return;
        }
        CrashRecord record = pending;
        pending = null;
        try {
            if (!record.markShown(directory)) {
                return;
            }
        } catch (IOException e) {
            return;
        }
        new NavigationDialogBuilder(new ContextThemeWrapper(activity, R.style.AppTheme))
                .setTitle(activity.getString(R.string.crash_report_title, activity.getString(R.string.app_name)))
                .setMessage(R.string.crash_report_details)
                .setPositiveButton(R.string.crash_report_share, (dialog, which) -> ProblemReport.share(activity))
                .setNegativeButton(R.string.crash_report_not_now, null)
                .show();
    }

    static String lastReport(Context context) {
        try {
            CrashRecord record = CrashRecord.read(context.getNoBackupFilesDir());
            return record == null ? "No crash recorded.\n" : record.text;
        } catch (IOException e) {
            return "Crash record unavailable.\n";
        }
    }

    @Override
    public void onActivityResumed(Activity activity) {
        resumedActivity = activity;
        showPendingReport();
    }

    @Override
    public void onActivityPaused(Activity activity) {
        if (resumedActivity == activity) {
            resumedActivity = null;
        }
    }

    @Override public void onActivityCreated(Activity activity, Bundle state) {}
    @Override public void onActivityStarted(Activity activity) {}
    @Override public void onActivityStopped(Activity activity) {}
    @Override public void onActivitySaveInstanceState(Activity activity, Bundle state) {}
    @Override public void onActivityDestroyed(Activity activity) {}
}
