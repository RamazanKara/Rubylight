package com.limelight.demo;

import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.widget.Toast;

import com.limelight.R;
import com.limelight.preferences.GpuDriverSettings;

/**
 * The real Snapdragon GPU driver screen, shown as on an arm64 Adreno phone so the demo can present it on any
 * device. It lists the demo's own (empty) driver folder, imports nothing and never loads a driver.
 */
public final class DemoGpuDriverSettings extends GpuDriverSettings {
    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override protected boolean supportsDrivers() {
        return true;
    }

    // Import driver ZIP would open the system file picker.
    @Override public void startActivityForResult(Intent intent, int requestCode, Bundle options) {
        Toast.makeText(this, R.string.demo_no_driver_import, Toast.LENGTH_SHORT).show();
    }
}
