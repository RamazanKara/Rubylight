package com.limelight.utils;

import android.app.Activity;
import android.content.Intent;
import android.widget.Toast;

import com.limelight.AppView;
import com.limelight.Game;
import com.limelight.R;
import com.limelight.ShortcutTrampoline;
import com.limelight.binding.PlatformBinding;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.JniNativeApiTransport;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.PreferencesRecentsStore;
import com.limelight.nativeapi.RecentsStore;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.HostHttpResponseException;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;

import org.xmlpull.v1.XmlPullParserException;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.net.UnknownHostException;
import java.security.cert.CertificateEncodingException;

import okhttp3.HttpUrl;

public class ServerHelper {
    public static final String CONNECTION_TEST_SERVER = "android.conntest.moonlight-stream.org";

    public static ComputerDetails.AddressTuple getCurrentAddressFromComputer(ComputerDetails computer) throws IOException {
        if (computer.activeAddress == null) {
            throw new IOException("No active address for "+computer.name);
        }
        return computer.activeAddress;
    }

    public static HttpUrl getHostSettingsUrl(ComputerDetails computer) {
        if (computer.state != ComputerDetails.State.ONLINE || !computer.caps.mayHave(HostCapabilities.Feature.WEB_UI) ||
                computer.activeAddress == null || computer.httpsPort < 0 || computer.httpsPort > 65529) {
            return null;
        }
        // Sunshine, Apollo, Vibepollo and Rubylight serve their web UI six ports above the
        // HTTPS port (the HTTP base port + 1) unless the user moved it.
        int port = computer.httpsPort == 0 ? computer.activeAddress.port + 1 : computer.httpsPort + 6;
        if (port <= 0 || port > 65535) {
            return null;
        }
        try {
            return new HttpUrl.Builder().scheme("https").host(computer.activeAddress.address)
                    .port(port).build();
        } catch (IllegalArgumentException e) {
            return null;
        }
    }

    public static Intent createPcShortcutIntent(Activity parent, ComputerDetails computer) {
        Intent i = new Intent(parent, ShortcutTrampoline.class);
        i.putExtra(AppView.NAME_EXTRA, computer.name);
        i.putExtra(AppView.UUID_EXTRA, computer.uuid);
        i.setAction(Intent.ACTION_DEFAULT);
        return i;
    }

    public static Intent createAppShortcutIntent(Activity parent, ComputerDetails computer, NvApp app) {
        Intent i = new Intent(parent, ShortcutTrampoline.class);
        i.putExtra(AppView.NAME_EXTRA, computer.name);
        i.putExtra(AppView.UUID_EXTRA, computer.uuid);
        i.putExtra(Game.EXTRA_APP_NAME, app.getAppName());
        i.putExtra(Game.EXTRA_APP_ID, ""+app.getAppId());
        i.putExtra(Game.EXTRA_APP_HDR, app.isHdrSupported());
        i.putExtra(Game.EXTRA_APP_UUID, app.getAppUuid());
        i.putExtra(Game.EXTRA_MONITOR_RESUME, app.getRole() == NvApp.Role.REMOTE_MONITOR);
        i.setAction(Intent.ACTION_DEFAULT);
        return i;
    }

    public static Intent createStartIntent(Activity parent, NvApp app, ComputerDetails computer,
                                           ComputerManagerService.ComputerManagerBinder managerBinder) {
        if (app.getRole() == NvApp.Role.STREAM) {
            // Every start goes through here, so this is the list "Continue playing" reads.
            new PreferencesRecentsStore(parent).record(
                    RecentsStore.Recent.of(computer, app, System.currentTimeMillis()));
        }
        Intent intent = new Intent(parent, Game.class);
        intent.putExtra(Game.EXTRA_HOST, computer.activeAddress.address);
        intent.putExtra(Game.EXTRA_PORT, computer.activeAddress.port);
        intent.putExtra(Game.EXTRA_HTTPS_PORT, computer.httpsPort);
        intent.putExtra(Game.EXTRA_APP_NAME, app.getAppName());
        intent.putExtra(Game.EXTRA_APP_ID, app.getAppId());
        intent.putExtra(Game.EXTRA_APP_HDR, app.isHdrSupported());
        intent.putExtra(Game.EXTRA_APP_UUID, app.getAppUuid());
        intent.putExtra(Game.EXTRA_MONITOR_RESUME, app.getRole() == NvApp.Role.REMOTE_MONITOR);
        intent.putExtra(Game.EXTRA_UNIQUEID, managerBinder.getUniqueId());
        intent.putExtra(Game.EXTRA_PC_UUID, computer.uuid);
        intent.putExtra(Game.EXTRA_PC_NAME, computer.name);
        try {
            if (computer.serverCert != null) {
                intent.putExtra(Game.EXTRA_SERVER_CERT, computer.serverCert.getEncoded());
            }
        } catch (CertificateEncodingException e) {
            e.printStackTrace();
        }
        return intent;
    }

    public static void doStart(Activity parent, NvApp app, ComputerDetails computer,
                               ComputerManagerService.ComputerManagerBinder managerBinder) {
        doStart(parent, app, computer, managerBinder, false);
    }

    /** Starts the game; with {@code smallStreamInputOnly}, as Input-only over a small stream this device does not show. */
    public static void doStart(Activity parent, NvApp app, ComputerDetails computer,
                               ComputerManagerService.ComputerManagerBinder managerBinder, boolean smallStreamInputOnly) {
        if (computer.state == ComputerDetails.State.OFFLINE || computer.activeAddress == null) {
            Toast.makeText(parent, parent.getResources().getString(R.string.pair_pc_offline), Toast.LENGTH_SHORT).show();
            return;
        }
        Intent intent = createStartIntent(parent, app, computer, managerBinder);
        if (smallStreamInputOnly) {
            intent.putExtra(Game.EXTRA_SMALL_STREAM_INPUT_ONLY, true);
        }
        parent.startActivity(intent);
    }

    public static void doQuit(final Activity parent,
                              final ComputerDetails computer,
                              final NvApp app,
                              final ComputerManagerService.ComputerManagerBinder managerBinder,
                              final Runnable onComplete) {
        doQuitReporting(parent, computer, app, managerBinder, ok -> {
            if (onComplete != null) {
                onComplete.run();
            }
        });
    }

    /** Like the plain quit, but {@code onDone} learns whether the host closed the game; it runs on any thread. */
    public static void doQuitReporting(final Activity parent,
                              final ComputerDetails computer,
                              final NvApp app,
                              final ComputerManagerService.ComputerManagerBinder managerBinder,
                              final java.util.function.Consumer<Boolean> onDone) {
        if (!computer.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
            Toast.makeText(parent, R.string.host_launch_permission_denied, Toast.LENGTH_LONG).show();
            onDone.accept(false);
            return;
        }
        Toast.makeText(parent, parent.getResources().getString(R.string.applist_quit_app) + " " + app.getAppName() + "...", Toast.LENGTH_SHORT).show();
        new Thread(new Runnable() {
            @Override
            public void run() {
                NvHTTP httpConn;
                String message;
                boolean quit = false;
                try {
                    boolean closed = false;
                    if (app.getRole() == NvApp.Role.STREAM && computer.caps.has(HostCapabilities.Feature.NATIVE_API)) {
                        // Close game needs host control; without it the classic Quit app below still works.
                        HostEndpoint endpoint = HostEndpoint.of(computer, managerBinder.getUniqueId(),
                                PlatformBinding.getCryptoProvider(parent));
                        closed = endpoint != null && new NativeApiClient(new JniNativeApiTransport(), endpoint)
                                .closeGameIfAllowed();
                    }
                    if (closed) {
                        quit = true;
                        message = parent.getResources().getString(R.string.applist_quit_success) + " " + app.getAppName();
                    } else {
                        httpConn = new NvHTTP(ServerHelper.getCurrentAddressFromComputer(computer), computer.httpsPort,
                                managerBinder.getUniqueId(), computer.serverCert, PlatformBinding.getCryptoProvider(parent));
                        if (app.getRole() != NvApp.Role.STREAM ? httpConn.disconnectRole(app.getRole()) : httpConn.quitApp()) {
                            quit = true;
                            message = parent.getResources().getString(R.string.applist_quit_success) + " " + app.getAppName();
                        } else {
                            message = parent.getResources().getString(R.string.applist_quit_fail) + " " + app.getAppName();
                        }
                    }
                } catch (NativeApiException e) {
                    message = e.getMessage();
                } catch (HostHttpResponseException e) {
                    if (e.getErrorCode() == 599) {
                        message = "This session wasn't started by this device," +
                                " so it cannot be quit. End streaming on the original " +
                                "device or the PC itself. (Error code: "+e.getErrorCode()+")";
                    }
                    else {
                        message = e.getMessage();
                    }
                } catch (UnknownHostException e) {
                    message = parent.getResources().getString(R.string.error_unknown_host);
                } catch (FileNotFoundException e) {
                    message = parent.getResources().getString(R.string.error_404);
                } catch (IOException | XmlPullParserException e) {
                    message = e.getMessage();
                    e.printStackTrace();
                } finally {
                    onDone.accept(quit);
                }

                final String toastMessage = message;
                parent.runOnUiThread(new Runnable() {
                    @Override
                    public void run() {
                        Toast.makeText(parent, toastMessage, Toast.LENGTH_LONG).show();
                    }
                });
            }
        }).start();
    }
}
