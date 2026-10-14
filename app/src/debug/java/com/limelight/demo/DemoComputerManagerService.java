package com.limelight.demo;

import android.content.Intent;
import android.os.IBinder;

import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nvstream.http.ComputerDetails;

import java.util.List;

public final class DemoComputerManagerService extends ComputerManagerService {
    private final List<ComputerDetails> computers = DemoFixtures.computers();
    private final ComputerManagerBinder binder = new ComputerManagerBinder() {
        @Override public boolean isPersistent() { return false; }
        @Override public boolean waitForReady() { return true; }
        @Override public void startPolling(ComputerManagerListener listener) {
            for (ComputerDetails computer : computers) listener.notifyComputerUpdated(computer);
        }
        @Override public void stopPolling() { }
        @Override public void waitForPollingStopped() { }
        @Override public String getUniqueId() { return "nightjar-demo"; }
        @Override public ComputerDetails getComputer(String uuid) {
            for (ComputerDetails computer : computers) {
                if (computer.uuid.equals(uuid)) return computer;
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
    };

    // The parent lifecycle starts discovery, database access and network callbacks.
    @Override public void onCreate() { }
    @Override public void onDestroy() { }
    @Override public boolean onUnbind(Intent intent) { return false; }
    @Override public IBinder onBind(Intent intent) { return binder; }
}
