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

    public DemoContext(Context base) {
        super(forceTelevision ? television(base) : base);
    }

    private static Context television(Context base) {
        Configuration configuration = new Configuration(base.getResources().getConfiguration());
        if ((configuration.uiMode & Configuration.UI_MODE_TYPE_MASK) == Configuration.UI_MODE_TYPE_TELEVISION) return base;
        configuration.uiMode = (configuration.uiMode & ~Configuration.UI_MODE_TYPE_MASK) | Configuration.UI_MODE_TYPE_TELEVISION;
        return base.createConfigurationContext(configuration);
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
