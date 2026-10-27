package com.limelight;

import android.app.Activity;
import android.app.Service;
import android.content.ComponentName;
import android.content.Intent;
import android.content.ServiceConnection;
import android.net.Uri;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.SystemClock;

import com.limelight.computers.ComputerDatabaseManager;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.console.HostWaker;
import com.limelight.pin.PinGate;
import com.limelight.pin.PinLockActivity;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.wol.WakeOnLanSender;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.Dialog;
import com.limelight.utils.FrontendEntry;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.SpinnerDialog;
import com.limelight.utils.UiHelper;

import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.io.StringReader;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.UUID;

public class ShortcutTrampoline extends Activity {
    private String uuidString;
    private NvApp app;
    private ArrayList<Intent> intentStack = new ArrayList<>();

    private ComputerDetails computer;
    // The PC as the newest poll saw it, and the wake that waits for it to answer.
    private volatile ComputerDetails latest;
    private HostWaker waker;
    // The wait ran out or no packet went out: the PC is not woken again.
    private volatile boolean wakeEnded;
    private SpinnerDialog blockingLoadSpinner;

    private volatile ComputerManagerService.ComputerManagerBinder managerBinder;
    private boolean managerServiceBound;
    private Thread serviceWaitThread;
    private volatile boolean stopped;

    private final ServiceConnection serviceConnection = new ServiceConnection() {
        public void onServiceConnected(ComponentName className, IBinder binder) {
            final ComputerManagerService.ComputerManagerBinder localBinder =
                    ((ComputerManagerService.ComputerManagerBinder)binder);

            // Wait in a separate thread to avoid stalling the UI
            serviceWaitThread = new Thread() {
                @Override
                public void run() {
                    // Wait for the binder to be ready
                    if (!localBinder.waitForReady() || stopped || isFinishing() || isDestroyed()) {
                        return;
                    }

                    // Now make the binder visible
                    managerBinder = localBinder;

                    // Get the computer object
                    computer = localBinder.getComputer(uuidString);

                    if (computer == null) {
                        runOnUiThread(() -> {
                            if (stopped || isFinishing() || isDestroyed()) {
                                return;
                            }
                            Dialog.displayDialog(ShortcutTrampoline.this,
                                    getResources().getString(R.string.conn_error_title),
                                    getResources().getString(R.string.scut_pc_not_found),
                                    true);
                            if (blockingLoadSpinner != null) {
                                blockingLoadSpinner.dismiss();
                                blockingLoadSpinner = null;
                            }
                            if (managerServiceBound) {
                                unbindService(serviceConnection);
                                managerServiceBound = false;
                            }
                            managerBinder = null;
                        });

                        return;
                    }

                    // Force CMS to repoll this machine
                    localBinder.invalidateStateForComputer(computer.uuid);

                    // Start polling
                    localBinder.startPolling(new ComputerManagerListener() {
                        @Override
                        public void notifyComputerUpdated(final ComputerDetails details) {
                            if (stopped) {
                                return;
                            }
                            // Don't care about other computers
                            if (!details.uuid.equalsIgnoreCase(uuidString)) {
                                return;
                            }

                            latest = details;

                            // Wake the PC if it's offline, and wait for it before launching
                            if (details.state == ComputerDetails.State.OFFLINE && WakeTargets.hasMac(ShortcutTrampoline.this, details) && !wakeEnded) {
                                runOnUiThread(() -> wake(localBinder));
                                return;
                            }

                            if (details.state != ComputerDetails.State.UNKNOWN) {
                                runOnUiThread(() -> finishWith(details));
                            }
                        }
                    });
                    if (stopped) {
                        localBinder.stopPolling();
                    }
                }
            };
            serviceWaitThread.start();
        }

        public void onServiceDisconnected(ComponentName className) {
            managerBinder = null;
        }
    };

    /**
     * Wakes the PC and waits for it: the packets go out again while it stays away, and each one has the manager poll it
     * at once. A poll that finds it online launches; when the wait runs out, or no packet goes out, the shortcut says so.
     */
    private void wake(ComputerManagerService.ComputerManagerBinder binder) {
        if (stopped || wakeEnded || isFinishing() || isDestroyed()) {
            return;
        }
        if (waker == null) {
            waker = new HostWaker(new Handler(Looper.getMainLooper()), task -> new Thread(task, "Wake on LAN").start(),
                    host -> {
                        // A resend already on its way when the shortcut stopped neither wakes nor polls.
                        if (stopped) {
                            return;
                        }
                        sendWake(host);
                        // Nor does a send the shortcut stopped during.
                        if (stopped) {
                            return;
                        }
                        binder.invalidateStateForComputer(host.uuid);
                    }, () -> latest, SystemClock::elapsedRealtime, new HostWaker.Listener() {
                        @Override
                        public void onOnline() {
                            // The poll that saw it online launches.
                        }

                        @Override
                        public void onTimedOut() {
                            wakeEnded = true;
                            ComputerDetails pc = latest;
                            fail(getString(R.string.console_wake_timeout, pc == null ? uuidString : pc.displayName()));
                        }

                        @Override
                        public void onSendFailed(IOException error) {
                            LimeLog.warning("Shortcut wake: no packet went out: " + error);
                            wakeEnded = true;
                            fail(getString(R.string.wol_fail));
                        }
                    });
        }
        if (waker.start() && blockingLoadSpinner != null) {
            blockingLoadSpinner.setMessage(getString(R.string.wol_waking_pc));
        }
    }

    /** Sends the wake packets (off the UI thread); tests override this. */
    protected void sendWake(ComputerDetails host) throws IOException {
        WakeOnLanSender.sendWolPacket(this, host);
    }

    private void fail(String message) {
        if (blockingLoadSpinner != null) {
            blockingLoadSpinner.dismiss();
            blockingLoadSpinner = null;
        }
        if (stopped || managerBinder == null || isFinishing() || isDestroyed()) {
            finish();
            return;
        }
        Dialog.displayDialog(this, getString(R.string.conn_error_title), message, true);
        managerBinder.stopPolling();
        unbindService(serviceConnection);
        managerServiceBound = false;
        managerBinder = null;
    }

    /** Launches, or says why not, once the PC answered or the wait for it is over; then lets go of the manager. */
    private void finishWith(ComputerDetails details) {
        if (waker != null) {
            waker.cancel();
        }
        // Stop showing the spinner
        if (blockingLoadSpinner != null) {
            blockingLoadSpinner.dismiss();
            blockingLoadSpinner = null;
        }

        // If the managerBinder was destroyed before this callback,
        // just finish the activity.
        if (stopped || managerBinder == null || isFinishing() || isDestroyed()) {
            finish();
            return;
        }

        if (details.state == ComputerDetails.State.ONLINE && details.pairState == PairingManager.PairState.PAIRED) {

            // Launch game if provided app ID, otherwise launch app view
            if (app != null) {
                if (details.runningGameId == 0 || app.getControl() != NvApp.Control.NONE ||
                        details.caps.has(HostCapabilities.Feature.HOST_CONFIRMS_REPLACEMENT) ||
                        app.matchesRunningApp(details.runningGameId, details.runningGameUuid)) {
                    intentStack.add(ServerHelper.createStartIntent(ShortcutTrampoline.this, app, details, managerBinder));

                    // Close this activity
                    finish();

                    // Now start the activities
                    startActivities(intentStack.toArray(new Intent[]{}));
                } else {
                    // Create the start intent immediately, so we can safely unbind the managerBinder
                    // below before we return.
                    final Intent startIntent = ServerHelper.createStartIntent(ShortcutTrampoline.this, app, details, managerBinder);

                    UiHelper.displayQuitConfirmationDialog(ShortcutTrampoline.this, new Runnable() {
                        @Override
                        public void run() {
                            intentStack.add(startIntent);

                            // Close this activity
                            finish();

                            // Now start the activities
                            startActivities(intentStack.toArray(new Intent[]{}));
                        }
                    }, new Runnable() {
                        @Override
                        public void run() {
                            // Close this activity
                            finish();
                        }
                    });
                }
            } else {
                // Close this activity
                finish();

                // Add the PC view at the back (and clear the task)
                Intent i;
                i = new Intent(ShortcutTrampoline.this, PcView.class);
                i.setAction(Intent.ACTION_MAIN);
                i.setFlags(Intent.FLAG_ACTIVITY_CLEAR_TASK | Intent.FLAG_ACTIVITY_NEW_TASK);
                intentStack.add(i);

                // Take this intent's data and create an intent to start the app view
                i = new Intent(getIntent());
                i.setClass(ShortcutTrampoline.this, AppView.class);
                intentStack.add(i);

                // If a game is running, we'll make the stream the top level activity
                if (details.runningGameId != 0
                        && !PinGate.needsPinForStream(ShortcutTrampoline.this, uuidString)) {
                    intentStack.add(ServerHelper.createStartIntent(ShortcutTrampoline.this,
                            new NvApp(null, details.runningGameId, false), details, managerBinder));
                }

                // Now start the activities
                startActivities(intentStack.toArray(new Intent[]{}));
            }

        }
        else if (details.state == ComputerDetails.State.OFFLINE) {
            // Computer offline - display an error dialog
            Dialog.displayDialog(ShortcutTrampoline.this,
                    getResources().getString(R.string.conn_error_title),
                    getResources().getString(R.string.error_pc_offline),
                    true);
        } else if (details.pairState != PairingManager.PairState.PAIRED) {
            // Computer not paired - display an error dialog
            Dialog.displayDialog(ShortcutTrampoline.this,
                    getResources().getString(R.string.conn_error_title),
                    getResources().getString(R.string.scut_not_paired),
                    true);
        }

        // We don't want any more callbacks from now on, so go ahead
        // and unbind from the service
        if (managerBinder != null) {
            managerBinder.stopPolling();
            unbindService(serviceConnection);
            managerServiceBound = false;
            managerBinder = null;
        }
    }

    protected boolean validateInput(String uuidString, String appIdString, String nameString) {
        // Validate PC UUID/Name
        if (uuidString == null && nameString == null) {
            Dialog.displayDialog(ShortcutTrampoline.this,
                    getResources().getString(R.string.conn_error_title),
                    getResources().getString(R.string.scut_invalid_uuid),
                    true);
            return false;
        }

        if (uuidString != null && !uuidString.isEmpty()) {
            try {
                UUID.fromString(uuidString);
            } catch (IllegalArgumentException ex) {
                Dialog.displayDialog(ShortcutTrampoline.this,
                        getResources().getString(R.string.conn_error_title),
                        getResources().getString(R.string.scut_invalid_uuid),
                        true);
                return false;
            }
        } else {
            // UUID is null, so fallback to Name
            if (nameString == null || nameString.isEmpty()) {
                Dialog.displayDialog(ShortcutTrampoline.this,
                        getResources().getString(R.string.conn_error_title),
                        getResources().getString(R.string.scut_invalid_uuid),
                        true);
                return false;
            }
        }

        // Validate App ID (if provided)
        if (appIdString != null && !appIdString.isEmpty()) {
            try {
                Integer.parseInt(appIdString);
            } catch (NumberFormatException ex) {
                Dialog.displayDialog(ShortcutTrampoline.this,
                        getResources().getString(R.string.conn_error_title),
                        getResources().getString(R.string.scut_invalid_app_id),
                        true);
                return false;
            }
        }

        return true;
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        UiHelper.notifyNewRootView(this);

        // Beacon, ES-DE, Daijisho and other frontends hand us a game entry file instead of extras
        if (Intent.ACTION_VIEW.equals(getIntent().getAction()) && getIntent().getData() != null) {
            Intent entryIntent = readFrontendEntry(getIntent().getData());
            if (entryIntent == null) {
                return;
            }
            setIntent(entryIntent);
        }

        // PC arguments, both are optional, but at least one must be provided
        uuidString = getIntent().getStringExtra(AppView.UUID_EXTRA);
        String nameString = getIntent().getStringExtra(AppView.NAME_EXTRA);

        // App arguments, both are optional, but one must be provided in order to start an app
        String appIdString = getIntent().getStringExtra(Game.EXTRA_APP_ID);
        String appNameString = getIntent().getStringExtra(Game.EXTRA_APP_NAME);
        String appUuidString = getIntent().getStringExtra(Game.EXTRA_APP_UUID);

        if (!validateInput(uuidString, appIdString, nameString)) {
            // Invalid input, so just return
            return;
        }

        if (uuidString == null || uuidString.isEmpty()) {
            // Use nameString to find the corresponding UUID
            ComputerDetails _computer;
            ComputerDatabaseManager dbManager = new ComputerDatabaseManager(this);
            try {
                _computer = dbManager.getComputerByName(nameString);
            } finally {
                dbManager.close();
            }

            if (_computer == null) {
                Dialog.displayDialog(ShortcutTrampoline.this,
                        getResources().getString(R.string.conn_error_title),
                        getResources().getString(R.string.scut_pc_not_found),
                        true);
                return;
            }

            uuidString = _computer.uuid;

            // Set the AppView UUID intent, since it wasn't provided
            setIntent(new Intent(getIntent()).putExtra(AppView.UUID_EXTRA, uuidString));
        }

        if (appIdString != null && !appIdString.isEmpty() &&
                (appUuidString == null || appUuidString.isEmpty())) {
            app = new NvApp(getIntent().getStringExtra(Game.EXTRA_APP_NAME),
                    Integer.parseInt(appIdString),
                    getIntent().getBooleanExtra(Game.EXTRA_APP_HDR, false));
            app.setAppUuid(getIntent().getStringExtra(Game.EXTRA_APP_UUID));
            app.setMonitorResume(getIntent().getBooleanExtra(Game.EXTRA_MONITOR_RESUME, false));
            app.setHostCapabilities(HostCapabilityCache.load(this, uuidString));
        }
        else if (appUuidString != null && !appUuidString.isEmpty()) {
            // The host resolves UUIDs at launch, so renamed apps and changed numeric IDs still work.
            // The cached list only supplies the display name and HDR flag when it has them.
            app = findCachedApp(appUuidString, null);
            if (app == null) {
                app = new NvApp(appNameString != null ? appNameString : "",
                        0, getIntent().getBooleanExtra(Game.EXTRA_APP_HDR, false));
                app.setAppUuid(appUuidString);
                app.setHostCapabilities(HostCapabilityCache.load(this, uuidString));
            }
            setIntent(new Intent(getIntent())
                    .putExtra(Game.EXTRA_APP_ID, Integer.toString(app.getAppId()))
                    .putExtra(Game.EXTRA_APP_NAME, app.getAppName())
                    .putExtra(Game.EXTRA_APP_HDR, app.isHdrSupported()));
        }
        else if (appNameString != null && !appNameString.isEmpty()) {
            // Use appNameString to find the corresponding AppId
            try {
                String rawAppList = CacheHelper.readInputStreamToString(CacheHelper.openCacheFileForInput(getCacheDir(), "applist", uuidString));

                if (rawAppList.isEmpty()) {
                    Dialog.displayDialog(ShortcutTrampoline.this,
                            getResources().getString(R.string.conn_error_title),
                            getResources().getString(R.string.scut_invalid_app_id),
                            true);
                    return;
                }
                List<NvApp> applist = NvHTTP.getAppListByReader(new StringReader(rawAppList),
                        HostCapabilityCache.load(this, uuidString));

                for (NvApp _app : applist) {
                    if (_app.getAppName().equals(appNameString)) {
                        app = _app;
                        break;
                    }
                }
                if (app == null || app.getAppId() < 0) {
                    Dialog.displayDialog(ShortcutTrampoline.this,
                            getResources().getString(R.string.conn_error_title),
                            getResources().getString(R.string.scut_invalid_app_id),
                            true);
                    return;
                }
                app.setHdrSupported(getIntent().getBooleanExtra(Game.EXTRA_APP_HDR, app.isHdrSupported()));
                setIntent(new Intent(getIntent())
                        .putExtra(Game.EXTRA_APP_ID, Integer.toString(app.getAppId()))
                        .putExtra(Game.EXTRA_APP_UUID, app.getAppUuid())
                        .putExtra(Game.EXTRA_APP_HDR, app.isHdrSupported()));
            } catch (IOException | XmlPullParserException e) {
                Dialog.displayDialog(ShortcutTrampoline.this,
                        getResources().getString(R.string.conn_error_title),
                        getResources().getString(R.string.scut_invalid_app_id),
                        true);
                return;
            }
        }

        // PIN lock: ask first, then run this shortcut again
        if ((app != null || PinGate.needsPinForApp(this)) && PinGate.needsPinForStream(this, uuidString)) {
            startActivity(PinLockActivity.forTrampoline(this, getIntent(), uuidString, app != null));
            finish();
            return;
        }

        // Bind to the computer manager service
        managerServiceBound = bindService(new Intent(this, ComputerManagerService.class), serviceConnection,
                Service.BIND_AUTO_CREATE);

        blockingLoadSpinner = SpinnerDialog.displayDialog(this, getResources().getString(R.string.conn_establishing_title),
                getResources().getString(R.string.applist_connect_msg), true);
    }

    private Intent readFrontendEntry(Uri uri) {
        Map<String, String> entry;
        try (InputStream in = getContentResolver().openInputStream(uri)) {
            if (in == null) {
                throw new IOException("No data");
            }
            entry = FrontendEntry.parse(new InputStreamReader(in, StandardCharsets.UTF_8));
        } catch (IOException | SecurityException e) {
            LimeLog.warning("Unreadable frontend entry: " + e.getMessage());
            Dialog.displayDialog(this,
                    getResources().getString(R.string.conn_error_title),
                    getString(R.string.scut_invalid_entry, getString(R.string.app_name)),
                    true);
            return null;
        }

        // A file completely describes its target; unrelated incoming extras must not fill in
        // missing fields and turn a PC-library entry into an unintended game launch.
        Intent i = new Intent(this, ShortcutTrampoline.class);
        putIfPresent(i, AppView.UUID_EXTRA, entry.get(FrontendEntry.KEY_HOST_UUID));
        putIfPresent(i, AppView.NAME_EXTRA, entry.get(FrontendEntry.KEY_HOST_NAME));
        putIfPresent(i, Game.EXTRA_APP_UUID, entry.get(FrontendEntry.KEY_APP_UUID));
        putIfPresent(i, Game.EXTRA_APP_NAME, entry.get(FrontendEntry.KEY_APP_NAME));
        putIfPresent(i, Game.EXTRA_APP_ID, entry.get(FrontendEntry.KEY_APP_ID));
        return i;
    }

    private static void putIfPresent(Intent intent, String key, String value) {
        if (value != null && !value.isEmpty()) {
            intent.putExtra(key, value);
        }
    }

    private NvApp findCachedApp(String appUuid, String appName) {
        try {
            String rawAppList = CacheHelper.readInputStreamToString(
                    CacheHelper.openCacheFileForInput(getCacheDir(), "applist", uuidString));
            if (rawAppList.isEmpty()) {
                return null;
            }
            for (NvApp candidate : NvHTTP.getAppListByReader(new StringReader(rawAppList),
                    HostCapabilityCache.load(this, uuidString))) {
                if (appUuid != null && appUuid.equalsIgnoreCase(candidate.getAppUuid())) {
                    return candidate;
                }
                if (appName != null && appName.equals(candidate.getAppName())) {
                    return candidate;
                }
            }
        } catch (IOException | XmlPullParserException e) {
            // No usable cache yet; the caller falls back to host-side resolution
        }
        return null;
    }

    @Override
    protected void onStop() {
        super.onStop();
        stopped = true;
        if (waker != null) {
            waker.cancel();
        }

        if (blockingLoadSpinner != null) {
            blockingLoadSpinner.dismiss();
            blockingLoadSpinner = null;
        }

        Dialog.closeDialogs(ShortcutTrampoline.this);

        if (managerBinder != null) {
            managerBinder.stopPolling();
            managerBinder = null;
        }

        if (serviceWaitThread != null) {
            serviceWaitThread.interrupt();
        }
        if (managerServiceBound) {
            unbindService(serviceConnection);
            managerServiceBound = false;
        }
        finish();
    }
}
