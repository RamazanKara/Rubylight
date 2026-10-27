package com.limelight;

import android.app.Activity;
import android.os.Bundle;
import android.os.Process;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;

public class CrashTestActivity extends Activity {
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        String crash = getIntent().getStringExtra("crash");
        if ("java".equals(crash)) {
            throw new IllegalStateException("Debug crash: https://private-host.local/192.168.1.1");
        }
        if ("native".equals(crash)) {
            try {
                Os.kill(Process.myPid(), OsConstants.SIGABRT);
            } catch (ErrnoException e) {
                throw new IllegalStateException(e);
            }
        }
        finish();
    }
}
