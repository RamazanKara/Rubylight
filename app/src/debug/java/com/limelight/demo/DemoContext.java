package com.limelight.demo;

import android.content.Context;
import android.content.ContextWrapper;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.SharedPreferences;
import android.content.res.Configuration;

import com.limelight.computers.ComputerManagerService;

import java.io.File;

// Keep capture preferences and artwork separate from the user's hosts and settings.
public final class DemoContext extends ContextWrapper {
    /** Set by the launcher's "--ez tv true": a handheld lays its screens out as a TV does, so a real GPU can measure them. */
    static boolean forceTelevision;

    /**
     * Set by the launcher's "--es look day" and "--es look dark": the system's night setting this run sees, so day can be
     * looked at on a device whose system is dark. {@link Configuration#UI_MODE_NIGHT_UNDEFINED} keeps the device's own.
     */
    static int forceNight = Configuration.UI_MODE_NIGHT_UNDEFINED;

    public DemoContext(Context base) {
        super(forced(base));
    }

    private static Context forced(Context base) {
        Configuration own = base.getResources().getConfiguration();
        int uiMode = own.uiMode;
        if (forceTelevision) uiMode = (uiMode & ~Configuration.UI_MODE_TYPE_MASK) | Configuration.UI_MODE_TYPE_TELEVISION;
        if (forceNight != Configuration.UI_MODE_NIGHT_UNDEFINED) uiMode = (uiMode & ~Configuration.UI_MODE_NIGHT_MASK) | forceNight;
        if (uiMode == own.uiMode) return base;
        Configuration configuration = new Configuration(own);
        configuration.uiMode = uiMode;
        return base.createConfigurationContext(configuration);
    }

    /** What "--es look ..." asks of the system's night setting. */
    static int appCompatNightOf(String look) {
        return "day".equals(look) ? androidx.appcompat.app.AppCompatDelegate.MODE_NIGHT_NO
                : "dark".equals(look) ? androidx.appcompat.app.AppCompatDelegate.MODE_NIGHT_YES
                : androidx.appcompat.app.AppCompatDelegate.MODE_NIGHT_FOLLOW_SYSTEM;
    }

    /** The Theme settings of that look. */
    static SharedPreferences.Editor putLook(SharedPreferences.Editor edit, String look) {
        return edit.putBoolean(com.limelight.ui.AppTheme.ALWAYS_DARK, "dark".equals(look))
                .putBoolean(com.limelight.ui.AppTheme.OLED_BLACK, "oled".equals(look))
                .putBoolean(com.limelight.ui.AppTheme.HIGH_CONTRAST, "contrast".equals(look));
    }

    static int nightOf(String look) {
        return "day".equals(look) ? Configuration.UI_MODE_NIGHT_NO
                : "dark".equals(look) ? Configuration.UI_MODE_NIGHT_YES : Configuration.UI_MODE_NIGHT_UNDEFINED;
    }

    @Override public Context getApplicationContext() {
        return this;
    }

    @Override public Context createConfigurationContext(Configuration configuration) {
        return new DemoContext(getBaseContext().createConfigurationContext(configuration));
    }

    @Override public SharedPreferences getSharedPreferences(String name, int mode) {
        return getBaseContext().getSharedPreferences("demo_" + name, mode);
    }

    @Override public File getCacheDir() {
        File directory = new File(getBaseContext().getCacheDir(), "demo");
        directory.mkdirs();
        return directory;
    }

    // GPU drivers live here: the demo's driver screen lists its own (empty) folder, not the user's drivers.
    @Override public File getNoBackupFilesDir() {
        File directory = new File(getBaseContext().getNoBackupFilesDir(), "demo");
        directory.mkdirs();
        return directory;
    }

    @Override public File getFilesDir() {
        File directory = new File(getBaseContext().getFilesDir(), "demo");
        directory.mkdirs();
        return directory;
    }

    @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
        if (intent.getComponent() != null &&
                ComputerManagerService.class.getName().equals(intent.getComponent().getClassName())) {
            // Keep the action: the PC settings screen asks for its own view of the fixtures with it.
            return super.bindService(new Intent(this, DemoComputerManagerService.class).setAction(intent.getAction()),
                    connection, flags);
        }
        return false;
    }
}
