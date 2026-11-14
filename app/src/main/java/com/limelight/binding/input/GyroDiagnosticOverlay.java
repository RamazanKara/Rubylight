package com.limelight.binding.input;

import android.app.Activity;
import android.content.Intent;
import android.graphics.Color;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.preference.PreferenceManager;
import android.text.TextUtils;
import android.view.Gravity;
import android.view.InputDevice;
import android.view.View;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;

import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

/**
 * Optional in-stream motion readings ("Show motion readings"). Samples are reported where
 * controller motion is sent, after its mapping, not from a separate sensor subscription. The
 * overlay can record them to a CSV, list the connected controllers and force the Flydigi preset
 * to compare it with the controller's own choice.
 *
 * Ported from jvanderv42's Nightjar fork (jvanderv42/nightjar, GPL-3.0, commit d0e14cf25,
 * app/src/main/java/com/limelight/binding/input/GyroDiagnosticOverlay.java), with translated
 * text and buttons a controller cannot focus.
 */
public final class GyroDiagnosticOverlay {
    public static final String PREF_SHOW_DIAGNOSTICS = "checkbox_show_gyro_diagnostics";

    private static final Object lock = new Object();
    private static final int MAX_ROWS = 1800;
    private static final List<String> rows = new ArrayList<>();
    private static final Handler mainHandler = new Handler(Looper.getMainLooper());
    private static volatile String latest;
    private static volatile boolean recording;
    private static volatile boolean flydigiForced;
    private static volatile boolean enabled;
    private static volatile TextView display;
    private static volatile Activity activity;

    private GyroDiagnosticOverlay() {}

    /** Whether readings are shown; callers skip the work of reporting otherwise. */
    static boolean isEnabled() {
        return enabled;
    }

    /** The overlay's compare switch; only a tap on it turns this on, and leaving the stream ends it. */
    static boolean isFlydigiForced() {
        return flydigiForced;
    }

    static void report(String path, int controller, byte type, float x, float y, float z) {
        if (!enabled) return;
        String label = type == MoonBridge.LI_MOTION_TYPE_GYRO ? "gyro" : "accel";
        latest = String.format(Locale.US, "%s controller=%d %s\nX=%+.4f  Y=%+.4f  Z=%+.4f",
                path, controller, label, x, y, z);
        synchronized (lock) {
            if (recording && rows.size() < MAX_ROWS) {
                rows.add(String.format(Locale.US, "%d,%s,%d,%s,%.6f,%.6f,%.6f",
                        SystemClock.elapsedRealtimeNanos(), path, controller, label, x, y, z));
            }
        }
    }

    private static Button button(Activity host, int text) {
        Button button = new Button(host);
        button.setText(text);
        // Touch only: a controller's D-pad and A drive the game, never these.
        button.setFocusable(false);
        button.setFocusableInTouchMode(false);
        return button;
    }

    public static void attach(Activity host) {
        enabled = PreferenceManager.getDefaultSharedPreferences(host)
                .getBoolean(PREF_SHOW_DIAGNOSTICS, false);
        if (!enabled) return;
        flydigiForced = false;
        recording = false;
        latest = host.getString(R.string.gyro_diag_waiting);
        activity = host;
        FrameLayout root = host.findViewById(android.R.id.content);
        LinearLayout panel = new LinearLayout(host);
        panel.setOrientation(LinearLayout.VERTICAL);
        int pad = Math.round(8 * host.getResources().getDisplayMetrics().density);
        panel.setPadding(pad * 2, pad, pad * 2, pad);
        panel.setBackgroundColor(0xDD101820);
        panel.setFocusable(false);

        TextView label = new TextView(host);
        label.setTextColor(Color.WHITE);
        label.setTextSize(12);
        panel.addView(label);
        display = label;

        LinearLayout buttons = new LinearLayout(host);
        Button capture = button(host, R.string.gyro_diag_record);
        capture.setOnClickListener(v -> {
            synchronized (lock) {
                if (!recording) {
                    rows.clear();
                    rows.add("elapsedNanos,path,controller,type,x,y,z");
                }
                recording = !recording;
                capture.setText(recording ? R.string.gyro_diag_stop : R.string.gyro_diag_record);
            }
        });
        buttons.addView(capture);

        Button mapping = button(host, R.string.gyro_diag_mapping_settings);
        mapping.setOnClickListener(v -> {
            flydigiForced = !flydigiForced;
            mapping.setText(flydigiForced ? R.string.gyro_diag_mapping_flydigi : R.string.gyro_diag_mapping_settings);
        });
        buttons.addView(mapping);

        Button identify = button(host, R.string.gyro_diag_identify);
        identify.setOnClickListener(v -> {
            List<String> devices = new ArrayList<>();
            for (int deviceId : InputDevice.getDeviceIds()) {
                InputDevice device = InputDevice.getDevice(deviceId);
                if (device == null) continue;
                int sources = device.getSources();
                if ((sources & InputDevice.SOURCE_GAMEPAD) != InputDevice.SOURCE_GAMEPAD
                        && (sources & InputDevice.SOURCE_JOYSTICK) != InputDevice.SOURCE_JOYSTICK) {
                    continue;
                }
                devices.add(String.format(Locale.US, "ID=%d name=%s VID=%04x PID=%04x descriptor=%s sources=0x%x",
                        deviceId, device.getName(), device.getVendorId(), device.getProductId(),
                        device.getDescriptor(), sources));
            }
            latest = host.getString(R.string.gyro_diag_devices) + "\n" + TextUtils.join("\n", devices);
            synchronized (lock) {
                // In the shared CSV too, as comment lines.
                for (String device : devices) rows.add("# " + device);
            }
        });
        buttons.addView(identify);

        Button share = button(host, R.string.gyro_diag_share);
        share.setOnClickListener(v -> {
            String csv;
            synchronized (lock) {
                csv = TextUtils.join("\n", rows);
            }
            Intent intent = new Intent(Intent.ACTION_SEND);
            intent.setType("text/plain");
            intent.putExtra(Intent.EXTRA_SUBJECT, "nightjar-motion.csv");
            intent.putExtra(Intent.EXTRA_TEXT, csv);
            host.startActivity(Intent.createChooser(intent, host.getString(R.string.gyro_diag_share_title)));
        });
        buttons.addView(share);

        Button hide = button(host, R.string.gyro_diag_hide);
        hide.setOnClickListener(v -> {
            boolean show = label.getVisibility() != View.VISIBLE;
            int visibility = show ? View.VISIBLE : View.GONE;
            label.setVisibility(visibility);
            capture.setVisibility(visibility);
            share.setVisibility(visibility);
            mapping.setVisibility(visibility);
            identify.setVisibility(visibility);
            hide.setText(show ? R.string.gyro_diag_hide : R.string.gyro_diag_show);
        });
        buttons.addView(hide);
        panel.addView(buttons);

        root.addView(panel, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.WRAP_CONTENT, FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.TOP | Gravity.START));
        mainHandler.removeCallbacks(refresh);
        mainHandler.post(refresh);
    }

    private static final Runnable refresh = new Runnable() {
        @Override public void run() {
            TextView target = display;
            Activity host = activity;
            if (target != null && host != null) {
                target.setText(host.getString(R.string.gyro_diag_title) + "\n" + latest
                        + (recording ? "\n" + host.getString(R.string.gyro_diag_recording) : ""));
                mainHandler.postDelayed(this, 100);
            }
        }
    };

    public static void detach(Activity host) {
        if (activity == host) {
            enabled = false;
            mainHandler.removeCallbacks(refresh);
            display = null;
            activity = null;
            recording = false;
            flydigiForced = false;
        }
    }
}
