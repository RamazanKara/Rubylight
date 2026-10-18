package com.limelight;

import android.content.ComponentName;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.ActivityNotFoundException;
import android.net.Uri;
import android.os.Bundle;
import android.os.IBinder;
import android.os.SystemClock;
import android.view.View;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.TextView;
import android.widget.Toast;
import com.limelight.ui.NightjarActivity;
import com.limelight.binding.PlatformBinding;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NetworkPathInfo;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.preferences.AddComputerManually;
import com.limelight.utils.ConnectionTestStats;
import com.limelight.utils.ServerHelper;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.UiHelper;
import java.io.IOException;
import org.xmlpull.v1.XmlPullParserException;

/** Explicit setup and bounded authenticated reachability checks. Never configures a router or VPN. */
public class RemoteSetupActivity extends NightjarActivity {
    public static final String EXTRA_PC_UUID = "remote_setup_pc_uuid";
    private int step, generation;
    private boolean bound, foreground, running;
    private ComputerManagerService.ComputerManagerBinder manager;
    private CheckRun activeRun;
    private static final class CheckRun {
        volatile boolean cancelled;
        volatile NvHTTP request;
        Thread worker;
        void cancel() {
            cancelled = true;
            if (worker != null) worker.interrupt();
            NvHTTP http = request;
            if (http != null) http.cancelPendingRequests();
        }
    }
    private TextView heading, description, result;
    private Button previous, next, test, link, address;
    private final ServiceConnection service = new ServiceConnection() {
        @Override public void onServiceConnected(ComponentName name, IBinder binder) {
            manager = (ComputerManagerService.ComputerManagerBinder) binder;
            showStep();
        }
        @Override public void onServiceDisconnected(ComponentName name) { manager = null; cancelTest(); showStep(); }
    };

    @Override protected void onCreate(Bundle state) {
        UiHelper.setLocale(this);
        super.onCreate(state);
        step = state == null ? 0 : Math.max(0, Math.min(2, state.getInt("step")));
        FrameLayout content = TopAppBar.setUpPage(this, R.string.remote_setup_title, this::finish);
        View guide = getLayoutInflater().inflate(R.layout.remote_setup, content, false);
        content.addView(guide);
        heading = guide.findViewById(R.id.remote_heading);
        description = guide.findViewById(R.id.remote_description);
        result = guide.findViewById(R.id.remote_result);
        previous = guide.findViewById(R.id.remote_previous);
        next = guide.findViewById(R.id.remote_next);
        test = guide.findViewById(R.id.remote_test);
        link = guide.findViewById(R.id.remote_link);
        address = guide.findViewById(R.id.remote_address);
        previous.setOnClickListener(v -> { cancelTest(); step = Math.max(0, step - 1); showStep(); });
        next.setOnClickListener(v -> { cancelTest(); step = Math.min(2, step + 1); showStep(); });
        link.setOnClickListener(v -> openGuide());
        address.setOnClickListener(v -> startActivity(new Intent(this, AddComputerManually.class)));
        test.setOnClickListener(v -> { if (running) { cancelTest(); result.setText(R.string.remote_cancelled); } else startTest(); });
        bound = bindService(new Intent(this, ComputerManagerService.class), service, BIND_AUTO_CREATE);
        showStep();
        UiHelper.notifyNewRootView(this);
    }

    private void showStep() {
        if (heading == null) return;
        heading.setText(getString(R.string.remote_step, step + 1));
        description.setText(new int[]{R.string.remote_prepare, R.string.remote_vpn, R.string.remote_verify}[step]);
        previous.setEnabled(step > 0 && !running);
        next.setVisibility(step == 2 ? View.GONE : View.VISIBLE);
        next.setEnabled(!running);
        link.setVisibility(step == 1 ? View.VISIBLE : View.GONE);
        address.setVisibility(step == 2 ? View.VISIBLE : View.GONE);
        test.setVisibility(step == 2 ? View.VISIBLE : View.GONE);
        test.setEnabled(manager != null);
        test.setText(running ? android.R.string.cancel : R.string.remote_test);
    }

    private void openGuide() {
        try { startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse("https://tailscale.com/download"))); }
        catch (ActivityNotFoundException | SecurityException e) { Toast.makeText(this, R.string.host_administration_no_browser, Toast.LENGTH_LONG).show(); }
    }

    private void startTest() {
        if (manager == null || !foreground || running) return;
        String uuid = getIntent().getStringExtra(EXTRA_PC_UUID);
        ComputerDetails saved = uuid == null ? null : manager.getComputer(uuid);
        if (saved == null || saved.serverCert == null || saved.httpsPort <= 0) {
            result.setText(R.string.remote_pair_first); return;
        }
        ComputerDetails computer = new ComputerDetails(saved);
        String uniqueId = manager.getUniqueId();
        running = true;
        final int current = ++generation;
        final CheckRun run = new CheckRun();
        activeRun = run;
        result.setText(R.string.remote_running);
        showStep();
        run.worker = new Thread(() -> {
            int message = R.string.remote_failed;
            ConnectionTestStats stats = new ConnectionTestStats();
            NetworkPathInfo.Kind route = NetworkPathInfo.Kind.UNKNOWN;
            try {
                // Prefer the saved manual VPN/public address over an old LAN discovery address.
                ComputerDetails.AddressTuple endpoint = computer.manualAddress != null ? computer.manualAddress :
                        ServerHelper.getCurrentAddressFromComputer(computer);
                NvHTTP http = new NvHTTP(endpoint, computer.httpsPort, uniqueId, computer.serverCert,
                        PlatformBinding.getCryptoProvider(getApplicationContext()));
                run.request = http;
                if (run.cancelled || Thread.currentThread().isInterrupted()) { http.cancelPendingRequests(); return; }
                ComputerDetails verified = null;
                for (int n = 0; n < 5; n++) {
                    if (run.cancelled) return;
                    long at = SystemClock.elapsedRealtimeNanos();
                    try {
                        ComputerDetails candidate = http.probePairedComputer();
                        int problem = verificationProblem(computer, candidate);
                        if (problem != 0) {
                            message = problem;
                            verified = null;
                            stats.add(Double.NaN);
                            break;
                        }
                        verified = candidate;
                        stats.add((SystemClock.elapsedRealtimeNanos() - at) / 1_000_000.0);
                    } catch (IOException | XmlPullParserException e) { stats.add(Double.NaN); }
                    Thread.sleep(150);
                }
                if (verified != null) {
                    message = R.string.remote_verified;
                    if (verified.caps.has(HostCapabilities.Feature.NETWORK_PATH)) {
                        try { route = http.getNetworkPath().kind; } catch (IOException ignored) { /* Keep route explicitly unknown. */ }
                    }
                }
            } catch (InterruptedException e) { Thread.currentThread().interrupt(); return; }
            catch (IOException | RuntimeException e) { /* Actionable local text, no raw addresses/errors. */ }
            finally { NvHTTP http = run.request; if (http != null) http.cancelPendingRequests(); }
            final int outcome = message;
            final NetworkPathInfo.Kind path = route;
            runOnUiThread(() -> {
                if (run.cancelled || !foreground || current != generation || isFinishing() || isDestroyed()) return;
                activeRun = null;
                running = false;
                int routeText = path == NetworkPathInfo.Kind.DIRECT ? R.string.remote_direct : path == NetworkPathInfo.Kind.RELAY ?
                        R.string.remote_relay : path == NetworkPathInfo.Kind.PEER_RELAY ? R.string.remote_peer_relay : R.string.remote_unknown;
                String summary = getString(outcome);
                if (outcome == R.string.remote_verified) summary += "\n\n" + getString(routeText) + "\n\n" +
                        getString(R.string.remote_measurements, stats.latencyMs(), stats.lossPercent());
                result.setText(summary);
                showStep();
            });
        }, "Remote setup verification");
        run.worker.start();
    }

    static int verificationProblem(ComputerDetails expected, ComputerDetails candidate) {
        if (expected.uuid == null || !expected.uuid.equals(candidate.uuid)) return R.string.remote_failed;
        if (candidate.pairState != PairingManager.PairState.PAIRED) return R.string.remote_pair_revoked;
        if (!candidate.hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH)) {
            return R.string.remote_permission_denied;
        }
        return 0;
    }

    private void cancelTest() {
        generation++;
        if (activeRun != null) activeRun.cancel();
        activeRun = null;
        if (running && result != null) result.setText(R.string.remote_cancelled);
        running = false;
        showStep();
    }
    @Override protected void onResume() { super.onResume(); foreground = true; }
    @Override protected void onPause() { foreground = false; cancelTest(); super.onPause(); }
    @Override protected void onSaveInstanceState(Bundle state) { state.putInt("step", step); super.onSaveInstanceState(state); }
    @Override protected void onDestroy() { cancelTest(); if (bound) unbindService(service); super.onDestroy(); }
}
