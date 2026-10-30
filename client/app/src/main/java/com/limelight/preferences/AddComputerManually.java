package com.limelight.preferences;

import java.io.IOException;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.InterfaceAddress;
import java.net.NetworkInterface;
import java.net.SocketException;
import java.net.URI;
import java.net.URISyntaxException;
import java.net.UnknownHostException;
import java.util.Collections;
import java.util.concurrent.LinkedBlockingQueue;

import com.limelight.binding.PlatformBinding;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostActions;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingLink;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.utils.Dialog;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.SpinnerDialog;
import com.limelight.utils.UiHelper;
import com.limelight.ui.TopAppBar;

import com.limelight.ui.NightjarActivity;
import android.Manifest;
import android.app.Activity;
import android.app.Service;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.pm.PackageManager;
import android.os.Bundle;
import android.os.IBinder;
import android.text.Editable;
import android.text.TextWatcher;
import android.view.KeyEvent;
import android.view.View;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.widget.TextView;
import android.widget.Toast;

import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.ContextCompat;

public class AddComputerManually extends NightjarActivity {
    private TextView hostText;
    private com.google.android.material.textfield.TextInputLayout hostInput;
    private boolean submitting;
    private ComputerManagerService.ComputerManagerBinder managerBinder;
    private final LinkedBlockingQueue<String> computersToAdd = new LinkedBlockingQueue<>();
    private Thread addThread;
    private boolean managerServiceBound;
    private boolean foreground;
    // Pairs a PC added from a QR pairing link (Experimental); a typed address only adds it.
    private final HostActions hostActions = new HostActions(this, new HostActions.Callbacks() {
        @Override public ComputerManagerService.ComputerManagerBinder binder() { return managerBinder; }
        @Override public boolean isForeground() { return foreground; }
        @Override public void onPaired(ComputerDetails computer) {
            Toast.makeText(AddComputerManually.this, R.string.addpc_qr_paired, Toast.LENGTH_LONG).show();
            finish();
        }
        @Override public void onRemoved(ComputerDetails computer) { }
        @Override public void onRenamed(ComputerDetails computer) { }
        @Override public void onWakeSent(ComputerDetails computer) { }
        @Override public void pausePolling() { }
        @Override public void resumePolling() { }
    });
    private final ActivityResultLauncher<Intent> scanner = registerForActivityResult(
            new ActivityResultContracts.StartActivityForResult(), result -> {
                Intent data = result.getData();
                if (result.getResultCode() == Activity.RESULT_OK && data != null) {
                    String link = data.getStringExtra(QrScanActivity.EXTRA_LINK);
                    if (link != null && !submitting) submit(link);
                }
            });
    // The camera is asked for only when Scan QR code is tapped.
    private final ActivityResultLauncher<String> cameraPermission = registerForActivityResult(
            new ActivityResultContracts.RequestPermission(), granted -> {
                if (granted) {
                    scanner.launch(new Intent(this, QrScanActivity.class));
                } else {
                    Dialog.displayDialog(this, getString(R.string.conn_error_title),
                            getString(R.string.addpc_qr_camera_denied), false);
                }
            });
    private final ServiceConnection serviceConnection = new ServiceConnection() {
        public void onServiceConnected(ComponentName className, final IBinder binder) {
            if (isFinishing() || isDestroyed()) {
                return;
            }
            managerBinder = ((ComputerManagerService.ComputerManagerBinder)binder);
            startAddThread();
        }

        public void onServiceDisconnected(ComponentName className) {
            joinAddThread();
            managerBinder = null;
        }
    };

    private boolean isWrongSubnetSiteLocalAddress(String address) {
        try {
            InetAddress targetAddress = InetAddress.getByName(address);
            if (!(targetAddress instanceof Inet4Address) || !targetAddress.isSiteLocalAddress()) {
                return false;
            }

            // We have a site-local address. Look for a matching local interface.
            for (NetworkInterface iface : Collections.list(NetworkInterface.getNetworkInterfaces())) {
                for (InterfaceAddress addr : iface.getInterfaceAddresses()) {
                    if (!(addr.getAddress() instanceof Inet4Address) || !addr.getAddress().isSiteLocalAddress()) {
                        // Skip non-site-local or non-IPv4 addresses
                        continue;
                    }

                    if (isInSubnet(targetAddress.getAddress(), addr.getAddress().getAddress(), addr.getNetworkPrefixLength())) {
                        return false;
                    }
                }
            }

            // Couldn't find a matching interface
            return true;
        } catch (Exception e) {
            // Catch all exceptions because some broken Android devices
            // will throw an NPE from inside getNetworkInterfaces().
            e.printStackTrace();
            return false;
        }
    }

    static boolean isInSubnet(byte[] address, byte[] subnet, int prefixLength) {
        for (int i = 0; i < prefixLength; i++) {
            int mask = 0x80 >> (i % 8);
            if ((address[i / 8] & mask) != (subnet[i / 8] & mask)) {
                return false;
            }
        }
        return true;
    }

    static URI parseRawUserInputToUri(String rawUserInput) {
        try {
            // Try adding a scheme and parsing the remaining input.
            // This handles input like 127.0.0.1:47989, [::1], [::1]:47989, and 127.0.0.1.
            URI uri = new URI("moonlight://" + rawUserInput);
            if (isHostAddress(uri)) {
                return uri;
            }
        } catch (URISyntaxException ignored) {}

        try {
            // Attempt to escape the input as an IPv6 literal.
            // This handles input like ::1.
            URI uri = new URI("moonlight://[" + rawUserInput + "]");
            if (isHostAddress(uri)) {
                return uri;
            }
        } catch (URISyntaxException ignored) {}

        return null;
    }

    private static boolean isHostAddress(URI uri) {
        return uri.getHost() != null && !uri.getHost().isEmpty()
                && uri.getRawUserInfo() == null && uri.getRawQuery() == null && uri.getRawFragment() == null
                && (uri.getRawPath() == null || uri.getRawPath().isEmpty())
                && (uri.getPort() == -1 || (uri.getPort() > 0 && uri.getPort() <= 65535));
    }

    private void doAddPc(String rawUserInput, ComputerManagerService.ComputerManagerBinder binder) throws InterruptedException {
        boolean wrongSiteLocal = false;
        boolean invalidInput = false;
        PairingLink link = PairingLink.parse(rawUserInput);
        boolean invalidLink = link == null && PairingLink.looksLikeLink(rawUserInput);
        ComputerDetails details = new ComputerDetails();
        boolean success;
        int portTestResult;

        SpinnerDialog dialog = SpinnerDialog.displayDialog(this, getResources().getString(R.string.title_add_pc),
            getResources().getString(R.string.msg_add_pc), true);

        try {
            // Check if we parsed a host address successfully
            URI uri = link != null || invalidLink ? null : parseRawUserInputToUri(rawUserInput);
            if (link != null) {
                details.manualAddress = new ComputerDetails.AddressTuple(link.host, link.port);
                success = binder.addComputerBlocking(details);
                if (!success) {
                    wrongSiteLocal = isWrongSubnetSiteLocalAddress(link.host);
                }
            } else if (uri != null && uri.getHost() != null && !uri.getHost().isEmpty()) {
                String host = uri.getHost();
                int port = uri.getPort();

                // If a port was not specified, use the default
                if (port == -1) {
                    port = NvHTTP.DEFAULT_HTTP_PORT;
                }

                details.manualAddress = new ComputerDetails.AddressTuple(host, port);
                success = binder.addComputerBlocking(details);
                if (!success){
                    wrongSiteLocal = isWrongSubnetSiteLocalAddress(host);
                }
            } else {
                // Invalid user input
                success = false;
                invalidInput = true;
            }
        } catch (InterruptedException e) {
            // Propagate the InterruptedException to the caller for proper handling
            dialog.dismiss();
            throw e;
        } catch (IllegalArgumentException e) {
            // This can be thrown from OkHttp if the host fails to canonicalize to a valid name.
            // https://github.com/square/okhttp/blob/okhttp_27/okhttp/src/main/java/com/squareup/okhttp/HttpUrl.java#L705
            e.printStackTrace();
            success = false;
            invalidInput = true;
        }

        if (Thread.currentThread().isInterrupted()) {
            dialog.dismiss();
            throw new InterruptedException();
        }

        // Keep the SpinnerDialog open while testing connectivity
        if (!success && !wrongSiteLocal && !invalidInput) {
            // Run the test before dismissing the spinner because it can take a few seconds.
            portTestResult = MoonBridge.testClientConnectivity(ServerHelper.CONNECTION_TEST_SERVER, 443,
                    MoonBridge.ML_PORT_FLAG_TCP_47984 | MoonBridge.ML_PORT_FLAG_TCP_47989);
        } else {
            // Don't bother with the test if we succeeded or the IP address was bogus
            portTestResult = MoonBridge.ML_TEST_RESULT_INCONCLUSIVE;
        }

        dialog.dismiss();

        if (Thread.currentThread().isInterrupted() || isFinishing() || isDestroyed()) {
            throw new InterruptedException();
        }

        if (invalidLink) {
            Dialog.displayDialog(this, getResources().getString(R.string.conn_error_title), getResources().getString(R.string.addpc_qr_invalid), false);
        }
        else if (invalidInput) {
            Dialog.displayDialog(this, getResources().getString(R.string.conn_error_title), getResources().getString(R.string.addpc_unknown_host), false);
        }
        else if (wrongSiteLocal) {
            Dialog.displayDialog(this, getResources().getString(R.string.conn_error_title), getResources().getString(R.string.addpc_wrong_sitelocal), false);
        }
        else if (!success) {
            String dialogText;
            if (portTestResult != MoonBridge.ML_TEST_RESULT_INCONCLUSIVE && portTestResult != 0)  {
                dialogText = getResources().getString(R.string.nettest_text_blocked);
            }
            else {
                dialogText = getResources().getString(R.string.addpc_fail);
            }
            Dialog.displayDialog(this, getResources().getString(R.string.conn_error_title), dialogText, false);
        }
        else if (link != null) {
            // Added: pair now with the link's one-time PIN, refused if the PC's certificate is not the link's.
            final PairingLink pairingLink = link;
            ComputerDetails stored = binder.getComputer(details.uuid);
            final ComputerDetails computer = stored != null ? stored : details;
            runOnUiThread(() -> {
                if (!isFinishing() && !isDestroyed()) hostActions.pairWithLink(computer, pairingLink);
            });
        }
        else {
            AddComputerManually.this.runOnUiThread(new Runnable() {
                @Override
                public void run() {
                Toast.makeText(AddComputerManually.this, getResources().getString(R.string.addpc_success), Toast.LENGTH_LONG).show();

                if (!isFinishing()) {
                    // Close the activity
                    AddComputerManually.this.finish();
                }
                }
            });
        }

    }

    private void startAddThread() {
        final ComputerManagerService.ComputerManagerBinder binder = managerBinder;
        addThread = new Thread() {
            @Override
            public void run() {
                while (!isInterrupted()) {
                    try {
                        String computer = computersToAdd.take();
                        try {
                            doAddPc(computer, binder);
                        } finally {
                            runOnUiThread(() -> {
                                if (!isFinishing() && !isDestroyed()) setSubmitting(false);
                            });
                        }
                    } catch (InterruptedException e) {
                        return;
                    }
                }
            }
        };
        addThread.setName("UI - AddComputerManually");
        addThread.start();
    }

    private void joinAddThread() {
        if (addThread != null) {
            addThread.interrupt();

            addThread = null;
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        foreground = true;
        hostActions.deliverPendingResult();
    }

    @Override
    protected void onPause() {
        foreground = false;
        super.onPause();
    }

    @Override
    protected void onStop() {
        super.onStop();

        Dialog.closeDialogs(AddComputerManually.this);
        SpinnerDialog.closeDialogs(this);
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();

        hostActions.destroy();
        joinAddThread();
        if (managerServiceBound) {
            unbindService(serviceConnection);
            managerServiceBound = false;
        }
        managerBinder = null;
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        UiHelper.setLocale(this);

        setContentView(R.layout.activity_add_computer_manually);
        View content = findViewById(android.R.id.content);
        TopAppBar.setNavigation(TopAppBar.configure(this, R.id.add_pc_toolbar, getString(R.string.title_add_pc), 0),
                R.drawable.ic_close, android.R.string.cancel, R.id.add_pc_back, v -> finish());

        UiHelper.notifyNewRootView(this);

        this.hostText = content.findViewById(R.id.hostTextView);
        this.hostInput = content.findViewById(R.id.host_input);
        hostText.addTextChangedListener(new TextWatcher() {
            @Override public void beforeTextChanged(CharSequence s, int start, int count, int after) { }
            @Override public void onTextChanged(CharSequence s, int start, int before, int count) {
                hostInput.setError(null);
            }
            @Override public void afterTextChanged(Editable value) { }
        });
        if (savedInstanceState != null) hostText.setText(savedInstanceState.getString("address", ""));
        hostText.setImeOptions(EditorInfo.IME_ACTION_DONE);
        hostText.setOnEditorActionListener(new TextView.OnEditorActionListener() {
            @Override
            public boolean onEditorAction(TextView textView, int actionId, KeyEvent keyEvent) {
                if (actionId == EditorInfo.IME_ACTION_DONE ||
                        (keyEvent != null &&
                                keyEvent.getAction() == KeyEvent.ACTION_DOWN &&
                                keyEvent.getKeyCode() == KeyEvent.KEYCODE_ENTER)) {
                    return handleDoneEvent();
                }
                else if (actionId == EditorInfo.IME_ACTION_PREVIOUS) {
                    // This is how the Fire TV dismisses the keyboard
                    InputMethodManager imm = (InputMethodManager)getSystemService(Context.INPUT_METHOD_SERVICE);
                    imm.hideSoftInputFromWindow(hostText.getWindowToken(), 0);
                    return false;
                }

                return false;
            }
        });

        content.findViewById(R.id.addPcButton).setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View view) {
                handleDoneEvent();
            }
        });

        View scan = content.findViewById(R.id.scanQrButton);
        if (getPackageManager().hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY)) {
            scan.setVisibility(View.VISIBLE);
            content.findViewById(R.id.scanQrHint).setVisibility(View.VISIBLE);
            scan.setOnClickListener(view -> scanQr());
        }

        // Bind to the ComputerManager service
        managerServiceBound = bindService(new Intent(AddComputerManually.this,
                    ComputerManagerService.class), serviceConnection, Service.BIND_AUTO_CREATE);
    }

    @Override
    protected void onSaveInstanceState(Bundle state) {
        super.onSaveInstanceState(state);
        state.putString("address", hostText.getText().toString());
    }

    // Returns true if the event should be eaten
    private boolean handleDoneEvent() {
        if (submitting) return true;
        return submit(hostText.getText().toString());
    }

    private void scanQr() {
        if (submitting) return;
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            scanner.launch(new Intent(this, QrScanActivity.class));
        } else {
            cameraPermission.launch(Manifest.permission.CAMERA);
        }
    }

    /** Adds the PC at a typed address, or adds and pairs the one a pairing link (typed, pasted or scanned) names. */
    private boolean submit(String hostAddress) {
        hostAddress = hostAddress.trim();
        if (hostAddress.length() == 0) {
            hostInput.setError(getString(R.string.addpc_enter_ip));
            hostText.requestFocus();
            return true;
        }
        boolean isLink = PairingLink.looksLikeLink(hostAddress);
        if (isLink && PairingLink.parse(hostAddress) == null) {
            hostInput.setError(getString(R.string.addpc_qr_invalid));
            return true;
        }
        if (!isLink && parseRawUserInputToUri(hostAddress) == null) {
            hostInput.setError(getString(R.string.add_pc_address_error));
            hostText.requestFocus();
            return true;
        }
        if (!managerServiceBound) {
            hostInput.setError(getString(R.string.add_pc_service_unavailable));
            return true;
        }

        hostInput.setError(null);
        setSubmitting(true);
        InputMethodManager imm = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
        if (imm != null) imm.hideSoftInputFromWindow(hostText.getWindowToken(), 0);
        computersToAdd.add(hostAddress);
        return true;
    }

    private void setSubmitting(boolean submitting) {
        this.submitting = submitting;
        hostText.setEnabled(!submitting);
        TextView button = findViewById(R.id.addPcButton);
        button.setEnabled(!submitting);
        button.setText(submitting ? R.string.add_pc_connecting : R.string.addpc_button);
    }
}
