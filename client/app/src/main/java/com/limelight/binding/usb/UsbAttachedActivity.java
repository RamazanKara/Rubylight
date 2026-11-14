package com.limelight.binding.usb;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;

/**
 * Declares the USB devices Nightjar drives, so that Android's permission dialog offers "Always allow" and keeps the
 * answer across restarts; Android grants that only for devices an activity is declared for (USB_DEVICE_ATTACHED). The
 * activity shows nothing: the driver service and the forwarder find the device on their own once it is plugged in.
 */
public class UsbAttachedActivity extends Activity {
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        finish();
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        finish();
    }

    /** Any device, for USB forwarding: enabled only while forwarding is on (see UsbForwarding.syncPersistentPermission). */
    public static final class Forwarding extends UsbAttachedActivity {
    }
}
