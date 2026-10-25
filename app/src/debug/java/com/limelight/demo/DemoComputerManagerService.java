package com.limelight.demo;

import android.content.Intent;
import android.os.IBinder;

import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvHTTP;

import java.util.List;

public final class DemoComputerManagerService extends ComputerManagerService {
    /**
     * The PC settings screen binds with this action. It sees Living-room PC online at a documentation
     * address (RFC 5737, never routed), so its rows read as they do for an online PC. The PC list and
     * library keep addressless fixtures, so their pairing, quit and wake actions stay offline.
     */
    static final String ACTION_PC_SETTINGS = "com.limelight.demo.PC_SETTINGS";
    static final String SETTINGS_ADDRESS = "192.0.2.10";

    private final List<ComputerDetails> computers = DemoAsleep.apply(DemoFixtures.computers());
    private final ComputerManagerBinder binder = new FixtureBinder(false);
    private final ComputerManagerBinder settingsBinder = new FixtureBinder(true);

    private final class FixtureBinder extends ComputerManagerBinder {
        private final boolean addressed;
        private ComputerManagerListener listener;

        FixtureBinder(boolean addressed) {
            this.addressed = addressed;
        }

        @Override public boolean isPersistent() { return false; }
        @Override public boolean waitForReady() { return true; }
        @Override public void startPolling(ComputerManagerListener listener) {
            stopPolling();
            this.listener = listener;
            DemoAsleep.listen(listener);
            for (ComputerDetails computer : computers) listener.notifyComputerUpdated(computer);
        }
        @Override public void stopPolling() {
            if (listener != null) DemoAsleep.unlisten(listener);
            listener = null;
        }
        @Override public void waitForPollingStopped() { }
        @Override public String getUniqueId() { return "nightjar-demo"; }
        @Override public ComputerDetails getComputer(String uuid) {
            for (ComputerDetails computer : computers) {
                if (!computer.uuid.equals(uuid)) continue;
                if (!addressed || computer.state != ComputerDetails.State.ONLINE) return computer;
                ComputerDetails online = new ComputerDetails(computer);
                online.activeAddress = new ComputerDetails.AddressTuple(SETTINGS_ADDRESS, NvHTTP.DEFAULT_HTTP_PORT);
                return online;
            }
            return null;
        }
        @Override public ApplistPoller createAppListPoller(ComputerDetails computer) {
            return new ApplistPoller(computer) {
                @Override public void start() { }
                @Override public void stop() { }
                @Override public void pollNow() { }
            };
        }
        @Override public boolean addComputerBlocking(ComputerDetails computer) { return false; }
        @Override public void removeComputer(ComputerDetails computer) { }
        @Override public void invalidateStateForComputer(String uuid) { }
    }

    // The parent lifecycle starts discovery, database access and network callbacks.
    @Override public void onCreate() { }
    @Override public void onDestroy() { }
    @Override public boolean onUnbind(Intent intent) { return false; }
    @Override public IBinder onBind(Intent intent) {
        return ACTION_PC_SETTINGS.equals(intent.getAction()) ? settingsBinder : binder;
    }
}
