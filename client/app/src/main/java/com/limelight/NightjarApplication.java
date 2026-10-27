package com.limelight;

import android.app.Application;
import android.content.Context;

import com.limelight.utils.CrashCapture;

public class NightjarApplication extends Application {
    private CrashCapture crashCapture;

    @Override
    protected void attachBaseContext(Context base) {
        super.attachBaseContext(base);
        crashCapture = new CrashCapture(this);
        crashCapture.installHandler();
    }

    @Override
    public void onCreate() {
        super.onCreate();
        crashCapture.start(this);
        com.limelight.pin.PinGate.install(this);
    }
}
