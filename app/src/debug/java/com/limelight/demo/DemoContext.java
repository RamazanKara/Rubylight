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
    public DemoContext(Context base) {
        super(base);
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
