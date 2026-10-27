package com.limelight.demo;

import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Bundle;
import android.widget.Toast;

import com.limelight.R;
import com.limelight.preferences.HostStreamSettings;

/** The real PC settings screen for Living-room PC, with demo preferences and the fixture binder. */
public final class DemoHostStreamSettings extends HostStreamSettings {
    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override public boolean bindService(Intent service, ServiceConnection connection, int flags) {
        return super.bindService(new Intent(service).setAction(DemoComputerManagerService.ACTION_PC_SETTINGS),
                connection, flags);
    }

    // Remote setup, the frontend export's folder picker and the host's web UI leave the fixture.
    @Override public void startActivityForResult(Intent intent, int requestCode, Bundle options) {
        Toast.makeText(this, R.string.demo_offline, Toast.LENGTH_SHORT).show();
    }
}
