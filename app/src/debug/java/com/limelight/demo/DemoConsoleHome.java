package com.limelight.demo;

import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.widget.Toast;

import com.limelight.AppView;
import com.limelight.PcView;
import com.limelight.R;
import com.limelight.console.ConsoleHomeActivity;
import com.limelight.console.GameDetailActivity;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.preferences.StreamSettings;

/** Home on the demo hosts: canned library, and every other screen it opens is the demo's own. */
public final class DemoConsoleHome extends ConsoleHomeActivity {
    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override protected ConsoleRepository createRepository(String uniqueId) {
        return DemoConsole.repository(this);
    }

    @Override protected Class<? extends GameDetailActivity> detailActivity() {
        return DemoGameDetail.class;
    }

    @Override public void startActivity(Intent intent, Bundle options) {
        String target = intent.getComponent() != null ? intent.getComponent().getClassName() : null;
        if (AppView.class.getName().equals(target)) {
            super.startActivity(new Intent(intent).setClass(this, DemoAppView.class), options);
        } else if (PcView.class.getName().equals(target)) {
            super.startActivity(new Intent(intent).setClass(this, DemoPcView.class), options);
        } else if (StreamSettings.class.getName().equals(target)) {
            super.startActivity(new Intent(this, DemoStreamSettings.class), options);
        } else if (target != null && target.startsWith("com.limelight.demo.")) {
            super.startActivity(intent, options);
        } else {
            Toast.makeText(this, R.string.demo_offline, Toast.LENGTH_SHORT).show();
        }
    }
}
