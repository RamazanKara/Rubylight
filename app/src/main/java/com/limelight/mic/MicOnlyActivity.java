package com.limelight.mic;

import android.Manifest;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Bundle;
import android.widget.Toast;

import androidx.activity.ComponentActivity;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * Asks for the microphone (and, on Android 13 and later, the notification) and starts {@link MicOnlyService}. A
 * see-through activity with no window of its own: Android starts a microphone service only from the foreground and
 * only with the permission granted, so both happen here.
 */
public class MicOnlyActivity extends ComponentActivity {
    private static final String EXTRA_START = "start";

    private final ActivityResultLauncher<String[]> permissions = registerForActivityResult(
            new ActivityResultContracts.RequestMultiplePermissions(), this::granted);

    /** Starts a Microphone only session with {@code computer}; false when the PC has no address to reach. */
    public static boolean start(Context context, ComputerDetails computer, String uniqueId) {
        Intent service = MicOnlyService.startIntent(context, computer, uniqueId);
        if (service == null) return false;
        context.startActivity(new Intent(context, MicOnlyActivity.class).putExtra(EXTRA_START, service));
        return true;
    }

    /** The permissions to ask for: the microphone always, notifications where Android asks for them. */
    static String[] wanted(int sdk) {
        List<String> wanted = new ArrayList<>();
        wanted.add(Manifest.permission.RECORD_AUDIO);
        if (sdk >= Build.VERSION_CODES.TIRAMISU) wanted.add(Manifest.permission.POST_NOTIFICATIONS);
        return wanted.toArray(new String[0]);
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        // A recreated activity has already asked; the answer comes to the new launcher.
        if (savedInstanceState != null) return;
        List<String> missing = new ArrayList<>();
        for (String permission : wanted(Build.VERSION.SDK_INT)) {
            if (ContextCompat.checkSelfPermission(this, permission) != PackageManager.PERMISSION_GRANTED) {
                missing.add(permission);
            }
        }
        if (missing.isEmpty()) {
            startService();
        } else {
            permissions.launch(missing.toArray(new String[0]));
        }
    }

    private void granted(Map<String, Boolean> answers) {
        // Without notifications the session still runs; Android shows it in the task manager.
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
            startService();
        } else {
            LimeLog.warning("Microphone only: the microphone permission was refused");
            Toast.makeText(this, R.string.mic_only_denied, Toast.LENGTH_LONG).show();
            finish();
        }
    }

    private void startService() {
        Intent service = getIntent().getParcelableExtra(EXTRA_START);
        if (service != null) {
            ContextCompat.startForegroundService(this, service);
        } else {
            LimeLog.warning("Microphone only: started without a PC");
        }
        finish();
    }
}
