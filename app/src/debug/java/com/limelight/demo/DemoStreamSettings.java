package com.limelight.demo;

import android.app.Fragment;
import android.app.FragmentManager;
import android.content.Context;
import android.content.Intent;
import android.os.Build;
import android.os.Bundle;
import android.preference.Preference;
import android.preference.PreferenceFragment;

import androidx.annotation.RequiresApi;

import com.limelight.pin.PinLockSettingsActivity;
import com.limelight.preferences.ControllerMappingActivity;
import com.limelight.preferences.StreamSettings;

/**
 * The real settings with demo preferences. The GPU driver row and the USB controller rows are shown as on a
 * phone that supports them, so the demo can present them on any device; their screens stay in the fixture.
 */
public final class DemoStreamSettings extends StreamSettings {
    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override protected void onCreate(Bundle state) {
        // Before the settings fragment is restored or created: point its GPU driver row at the demo's screen.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) openDemoDriverScreen();
        super.onCreate(state);
    }

    @RequiresApi(Build.VERSION_CODES.O)
    private void openDemoDriverScreen() {
        getFragmentManager().registerFragmentLifecycleCallbacks(new FragmentManager.FragmentLifecycleCallbacks() {
            @Override public void onFragmentActivityCreated(FragmentManager manager, Fragment fragment, Bundle saved) {
                if (!(fragment instanceof PreferenceFragment)) return;
                Preference driver = ((PreferenceFragment) fragment).findPreference("gpu_driver");
                if (driver != null) driver.setOnPreferenceClickListener(preference -> {
                    startActivity(new Intent(DemoStreamSettings.this, DemoGpuDriverSettings.class));
                    return true;
                });
            }
        }, false);
    }

    @Override protected boolean supportsGpuDrivers() {
        return true;
    }

    @Override protected boolean supportsUsbHost() {
        return true;
    }

    @Override public void startActivityForResult(Intent intent, int requestCode, Bundle options) {
        String target = intent.getComponent() != null ? intent.getComponent().getClassName() : null;
        if (ControllerMappingActivity.class.getName().equals(target)) {
            super.startActivityForResult(new Intent(this, DemoControllerMappingActivity.class), requestCode, options);
        } else if (DemoGpuDriverSettings.class.getName().equals(target)
                || PinLockSettingsActivity.class.getName().equals(target)) {
            super.startActivityForResult(intent, requestCode, options);
        }
    }
}
