package com.limelight.computers;

import java.io.IOException;
import java.io.OutputStream;
import java.io.StringReader;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.UnknownHostException;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.LinkedList;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.locks.Lock;
import java.util.concurrent.locks.ReentrantLock;

import com.limelight.AppView;
import com.limelight.LimeLog;
import com.limelight.binding.PlatformBinding;
import com.limelight.discovery.DiscoveryService;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.JniNativeApiTransport;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.mdns.MdnsComputer;
import com.limelight.nvstream.mdns.MdnsDiscoveryListener;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.nvstream.wol.WakeTargetsPoll;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.preferences.HostStreamProfile;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.FrontendExportStore;
import com.limelight.utils.FrontendSync;
import com.limelight.utils.NetHelper;
import com.limelight.utils.RedactedLog;
import com.limelight.utils.ServerHelper;

import android.app.Service;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.Binder;
import android.os.Build;
import android.os.IBinder;
import android.os.SystemClock;

import org.xmlpull.v1.XmlPullParserException;

public class ComputerManagerService extends Service {
    private static final int SERVERINFO_POLLING_PERIOD_MS = 1500;
    private static final int APPLIST_POLLING_PERIOD_MS = 30000;
    private static final int APPLIST_FAILED_POLLING_RETRY_MS = 2000;
    private static final int MDNS_QUERY_PERIOD_MS = 1000;
    private static final int OFFLINE_POLL_TRIES = 3;
    private static final int INITIAL_POLL_TRIES = 2;
    private static final int EMPTY_LIST_THRESHOLD = 3;
    private static final int POLL_DATA_TTL_MS = 30000;

    private final ComputerManagerBinder binder = new ComputerManagerBinder();

    private ComputerDatabaseManager dbManager;
    private final AtomicInteger dbRefCount = new AtomicInteger(0);

    private IdentityManager idManager;
    // Keeps the wake targets of the polled PCs, for wakes from the PC list and shortcuts.
    private WakeTargetsPoll wakeTargets;
    // Its HTTPS requests, off the polling threads and their network locks.
    private ExecutorService wakeRequests;
    private final LinkedList<PollingTuple> pollingTuples = new LinkedList<>();
    // Orders a poll's save of a PC against removeComputer's delete (see runPoll).
    private final Object removalLock = new Object();
    private volatile ComputerManagerListener listener;
    private final AtomicInteger activePolls = new AtomicInteger(0);
    private volatile boolean pollingActive;
    private volatile boolean destroyed;
    private boolean discoveryServiceBound;
    private final Lock defaultNetworkLock = new ReentrantLock();

    private ConnectivityManager.NetworkCallback networkCallback;

    private volatile DiscoveryService.DiscoveryBinder discoveryBinder;
    private final ServiceConnection discoveryServiceConnection = new ServiceConnection() {
        public void onServiceConnected(ComponentName className, IBinder binder) {
            synchronized (discoveryServiceConnection) {
                if (destroyed) {
                    return;
                }
                DiscoveryService.DiscoveryBinder privateBinder = ((DiscoveryService.DiscoveryBinder)binder);

                // Set us as the event listener
                privateBinder.setListener(createDiscoveryListener());

                // Signal a possible waiter that we're all setup
                discoveryBinder = privateBinder;
                if (pollingActive) {
                    privateBinder.startDiscovery(MDNS_QUERY_PERIOD_MS);
                }
                discoveryServiceConnection.notifyAll();
            }
        }

        public void onServiceDisconnected(ComponentName className) {
            discoveryBinder = null;
        }
    };

    // Returns true if the details object was modified. A PC being added has no tuple yet.
    private boolean runPoll(ComputerDetails details, PollingTuple tuple, int offlineCount) throws InterruptedException {
        if (!getLocalDatabaseReference()) {
            return false;
        }

        try {
            final boolean newPc = tuple == null;
            final int pollTriesBeforeOffline = details.state == ComputerDetails.State.UNKNOWN ?
                    INITIAL_POLL_TRIES : OFFLINE_POLL_TRIES;

            activePolls.incrementAndGet();

            // Poll the machine
            try {
                if (!pollComputer(details)) {
                    if (!newPc && offlineCount < pollTriesBeforeOffline) {
                        // Return without calling the listener
                        return false;
                    }

                    details.state = ComputerDetails.State.OFFLINE;
                }
            } finally {
                activePolls.decrementAndGet();
            }

            // If it's online, update our persistent state
            ComputerDetails existingComputer = null;
            if (details.state == ComputerDetails.State.ONLINE) {
                existingComputer = dbManager.getComputerByUUID(details.uuid);

                // Check if it's in the database because it could have been
                // removed after this was issued
                if (!newPc && existingComputer == null) {
                    // It's gone
                    return false;
                }

                if (existingComputer == null) {
                    try {
                        // If the active address is a site-local address (RFC 1918),
                        // then use STUN to populate the external address field if
                        // it's not set already.
                        if (details.remoteAddress == null) {
                            InetAddress addr = InetAddress.getByName(details.activeAddress.address);
                            if (addr.isSiteLocalAddress()) {
                                populateExternalAddress(details);
                            }
                        }
                    } catch (UnknownHostException ignored) {}
                }
            }

            // removeComputer marks the tuple removed before it deletes the PC under this lock, so a PC removed
            // while it was polled is neither saved again nor shown, and its wake targets are not asked for again.
            // Not pollingTuples: the poll holds the tuple's network lock, which invalidateStateForComputer takes.
            synchronized (removalLock) {
                if (!newPc && tuple.removed) {
                    return false;
                }

                if (details.state == ComputerDetails.State.ONLINE) {
                    // If we already have an entry for this computer in the DB, we must
                    // combine the existing data with this new data (which may be partially available
                    // due to detecting the PC via mDNS) without the saved external address. If we
                    // write to the DB without doing this first, we can overwrite our existing data.
                    HostCapabilityCache.save(this, details.uuid, details.caps);
                    if (existingComputer != null) {
                        existingComputer.update(details);
                        dbManager.updateComputer(existingComputer);
                    }
                    else {
                        dbManager.updateComputer(details);
                    }
                }

                // Don't call the listener if this is a failed lookup of a new PC
                ComputerManagerListener currentListener = listener;
                if ((!newPc || details.state == ComputerDetails.State.ONLINE) && currentListener != null) {
                    if (newPc) {
                        // A rediscovered PC keeps the user's name and pinned address from the first update on.
                        details.alias = HostNames.get(this, details.uuid);
                        details.pinnedAddress = PinnedAddresses.get(this, details.uuid);
                    }
                    currentListener.notifyComputerUpdated(details);
                }

                // After the listener, so a PC that answers is shown before its native API is asked.
                wakeTargets.polled(details);
            }
            return true;
        } finally {
            releaseLocalDatabaseReference();
        }
    }

    private Thread createPollingThread(final PollingTuple tuple) {
        Thread t = new Thread() {
            @Override
            public void run() {

                int offlineCount = 0;
                while (!isInterrupted() && pollingActive && tuple.thread == this) {
                    try {
                        // Only allow one request to the machine at a time
                        synchronized (tuple.networkLock) {
                            // Check if this poll has modified the details
                            if (!runPoll(tuple.computer, tuple, offlineCount)) {
                                LimeLog.info("PC is offline (try " + offlineCount + ")");
                                offlineCount++;
                            } else {
                                tuple.lastSuccessfulPollMs = SystemClock.elapsedRealtime();
                                offlineCount = 0;
                            }
                        }

                        // Wait until the next polling interval
                        Thread.sleep(SERVERINFO_POLLING_PERIOD_MS);
                    } catch (InterruptedException e) {
                        break;
                    }
                }
            }
        };
        t.setName("Polling thread for " + tuple.computer.name);
        return t;
    }

    public class ComputerManagerBinder extends Binder {
        public boolean isPersistent() {
            return true;
        }

        public void startPolling(ComputerManagerListener listener) {
            if (destroyed) {
                return;
            }
            // Polling is active
            pollingActive = true;

            // Set the listener
            ComputerManagerService.this.listener = listener;

            // Start mDNS autodiscovery too
            DiscoveryService.DiscoveryBinder discovery = discoveryBinder;
            if (discovery != null) {
                discovery.startDiscovery(MDNS_QUERY_PERIOD_MS);
            }

            synchronized (pollingTuples) {
                for (PollingTuple tuple : pollingTuples) {
                    // Enforce the poll data TTL
                    if (SystemClock.elapsedRealtime() - tuple.lastSuccessfulPollMs > POLL_DATA_TTL_MS) {
                        LimeLog.info("Timing out polled state for "+tuple.computer.name);
                        tuple.computer.state = ComputerDetails.State.UNKNOWN;
                    }

                    // Report this computer initially
                    listener.notifyComputerUpdated(tuple.computer);

                    // This polling thread might already be there
                    if (tuple.thread == null) {
                        tuple.thread = createPollingThread(tuple);
                        tuple.thread.start();
                    }
                }
            }
        }

        public boolean waitForReady() {
            synchronized (discoveryServiceConnection) {
                try {
                    while (discoveryBinder == null && discoveryServiceBound && !destroyed) {
                        // Wait for the bind notification
                        discoveryServiceConnection.wait(1000);
                    }
                } catch (InterruptedException e) {
                    e.printStackTrace();

                    // InterruptedException clears the thread's interrupt status. Since we can't
                    // handle that here, we will re-interrupt the thread to set the interrupt
                    // status back to true.
                    Thread.currentThread().interrupt();
                    return false;
                }
                return discoveryBinder != null && !destroyed;
            }
        }

        public void waitForPollingStopped() {
            while (activePolls.get() != 0) {
                try {
                    Thread.sleep(250);
                } catch (InterruptedException e) {
                    e.printStackTrace();

                    // InterruptedException clears the thread's interrupt status. Since we can't
                    // handle that here, we will re-interrupt the thread to set the interrupt
                    // status back to true.
                    Thread.currentThread().interrupt();
                    return;
                }
            }
        }

        public boolean addComputerBlocking(ComputerDetails fakeDetails) throws InterruptedException {
            return ComputerManagerService.this.addComputerBlocking(fakeDetails);
        }

        public void removeComputer(ComputerDetails computer) {
            ComputerManagerService.this.removeComputer(computer);
        }

        /** Gives the PC a name on this device only; null shows the PC's own name again. */
        public void renameComputer(String uuid, String alias) {
            ComputerManagerService.this.renameComputer(uuid, alias);
        }

        public void pinAddress(String uuid, ComputerDetails.AddressTuple address) {
            ComputerManagerService.this.pinAddress(uuid, address);
        }

        public void stopPolling() {
            // Just call the unbind handler to cleanup
            ComputerManagerService.this.onUnbind(null);
        }

        public ApplistPoller createAppListPoller(ComputerDetails computer) {
            return new ApplistPoller(computer);
        }

        public String getUniqueId() {
            return idManager.getUniqueId();
        }

        public ComputerDetails getComputer(String uuid) {
            synchronized (pollingTuples) {
                for (PollingTuple tuple : pollingTuples) {
                    if (uuid.equalsIgnoreCase(tuple.computer.uuid)) {
                        return tuple.computer;
                    }
                }
            }

            return null;
        }

        public void invalidateStateForComputer(String uuid) {
            // Not under pollingTuples: a poll holds the network lock for its whole run.
            List<PollingTuple> matching = new ArrayList<>();
            synchronized (pollingTuples) {
                for (PollingTuple tuple : pollingTuples) {
                    if (uuid.equals(tuple.computer.uuid)) {
                        matching.add(tuple);
                    }
                }
            }
            for (PollingTuple tuple : matching) {
                // We need the network lock to prevent a concurrent poll
                // from wiping this change out
                synchronized (tuple.networkLock) {
                    tuple.computer.state = ComputerDetails.State.UNKNOWN;
                }
            }
        }
    }

    @Override
    public boolean onUnbind(Intent intent) {
        DiscoveryService.DiscoveryBinder discovery = discoveryBinder;
        if (discovery != null) {
            discovery.stopDiscovery();
        }

        // Stop polling
        pollingActive = false;
        synchronized (pollingTuples) {
            for (PollingTuple tuple : pollingTuples) {
                if (tuple.thread != null) {
                    // Interrupt and remove the thread
                    tuple.thread.interrupt();
                    tuple.thread = null;
                }
            }
        }

        // Remove the listener
        listener = null;

        return false;
    }

    private void populateExternalAddress(ComputerDetails details) {
        boolean boundToNetwork = false;
        ConnectivityManager connMgr = (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
        defaultNetworkLock.lock();
        try {
            boolean activeNetworkIsVpn = NetHelper.isActiveNetworkVpn(this);
            if (activeNetworkIsVpn) {
                // STUN must use an underlying LAN, not the VPN's public endpoint.
                for (Network net : connMgr.getAllNetworks()) {
                    NetworkCapabilities netCaps = connMgr.getNetworkCapabilities(net);
                    if (netCaps != null && !netCaps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) &&
                            !netCaps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) {
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                            boundToNetwork = connMgr.bindProcessToNetwork(net);
                        } else {
                            boundToNetwork = ConnectivityManager.setProcessDefaultNetwork(net);
                        }
                        if (boundToNetwork) {
                            break;
                        }
                    }
                }
            }

            if (!activeNetworkIsVpn || boundToNetwork) {
                String stunResolvedAddress = NvConnection.findExternalAddressForMdns("stun.moonlight-stream.org", 3478);
                if (stunResolvedAddress != null) {
                    details.remoteAddress = new ComputerDetails.AddressTuple(stunResolvedAddress, details.guessExternalPort());
                }
            }
        } finally {
            try {
                if (boundToNetwork) {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                        connMgr.bindProcessToNetwork(null);
                    } else {
                        ConnectivityManager.setProcessDefaultNetwork(null);
                    }
                }
            } finally {
                defaultNetworkLock.unlock();
            }
        }
    }

    private MdnsDiscoveryListener createDiscoveryListener() {
        return new MdnsDiscoveryListener() {
            @Override
            public boolean notifyComputerAdded(MdnsComputer computer) {
                ComputerDetails details = new ComputerDetails();

                // Populate the computer template with mDNS info
                if (computer.getLocalAddress() != null) {
                    details.localAddress = new ComputerDetails.AddressTuple(computer.getLocalAddress().getHostAddress(), computer.getPort());

                    // Since we're on the same network, we can use STUN to find
                    // our WAN address, which is also very likely the WAN address
                    // of the PC. We can use this later to connect remotely.
                    if (computer.getLocalAddress() instanceof Inet4Address) {
                        populateExternalAddress(details);
                    }
                }
                if (computer.getIpv6Address() != null) {
                    details.ipv6Address = new ComputerDetails.AddressTuple(computer.getIpv6Address().getHostAddress(), computer.getPort());
                }

                try {
                    // Kick off a blocking serverinfo poll on this machine
                    if (!addComputerBlocking(details)) {
                        LimeLog.warning("Auto-discovered PC failed to respond");
                        return false;
                    }
                    return true;
                } catch (InterruptedException e) {
                    e.printStackTrace();

                    // InterruptedException clears the thread's interrupt status. Since we can't
                    // handle that here, we will re-interrupt the thread to set the interrupt
                    // status back to true.
                    Thread.currentThread().interrupt();
                    return false;
                }
            }

            @Override
            public void notifyDiscoveryFailure(Exception e) {
                LimeLog.severe("mDNS discovery failed");
                e.printStackTrace();
            }
        };
    }

    private void addTuple(ComputerDetails details) {
        // The user's name and pinned address for the PC are stored beside the database, not in it.
        details.alias = HostNames.get(this, details.uuid);
        details.pinnedAddress = PinnedAddresses.get(this, details.uuid);
        synchronized (pollingTuples) {
            for (PollingTuple tuple : pollingTuples) {
                // Check if this is the same computer
                if (tuple.computer.uuid.equals(details.uuid)) {
                    // Update the saved computer with potentially new details
                    tuple.computer.update(details);
                    if (!java.util.Objects.equals(tuple.computer.alias, details.alias)) {
                        tuple.computer.alias = details.alias;
                    }

                    // Start a polling thread if polling is active
                    if (pollingActive && tuple.thread == null) {
                        tuple.thread = createPollingThread(tuple);
                        tuple.thread.start();
                    }

                    // Found an entry so we're done
                    return;
                }
            }

            // If we got here, we didn't find an entry
            PollingTuple tuple = new PollingTuple(details, null);
            if (pollingActive) {
                tuple.thread = createPollingThread(tuple);
            }
            pollingTuples.add(tuple);
            if (tuple.thread != null) {
                tuple.thread.start();
            }
        }
    }

    public boolean addComputerBlocking(ComputerDetails fakeDetails) throws InterruptedException {
        // Block while we try to fill the details

        // We cannot use runPoll() here because it will attempt to persist the state of the machine
        // in the database, which would be bad because we don't have our pinned cert loaded yet.
        if (pollComputer(fakeDetails)) {
            // See if we have record of this PC to pull its pinned cert
            synchronized (pollingTuples) {
                for (PollingTuple tuple : pollingTuples) {
                    if (tuple.computer.uuid.equals(fakeDetails.uuid)) {
                        fakeDetails.serverCert = tuple.computer.serverCert;
                        break;
                    }
                }
            }

            // Poll again, possibly with the pinned cert, to get accurate pairing information.
            // This will insert the host into the database too.
            runPoll(fakeDetails, null, 0);
        }

        // If the machine is reachable, it was successful
        if (fakeDetails.state == ComputerDetails.State.ONLINE) {
            LimeLog.info("New PC ("+fakeDetails.name+") is UUID "+fakeDetails.uuid);

            // Start a polling thread for this machine
            addTuple(fakeDetails);
            return true;
        }
        else {
            return false;
        }
    }

    /**
     * Pins the PC to one of its known addresses, which then is the only one polled and streamed to, or
     * brings back the automatic pick ({@code address} null), as Moonlight V+ PC's address pinning.
     */
    public void pinAddress(String uuid, ComputerDetails.AddressTuple address) {
        PinnedAddresses.set(this, uuid, address);
        synchronized (pollingTuples) {
            for (PollingTuple tuple : pollingTuples) {
                if (uuid.equals(tuple.computer.uuid)) {
                    tuple.computer.pinnedAddress = address;
                    if (address != null) {
                        tuple.computer.activeAddress = address;
                    }
                }
            }
        }
    }

    public void renameComputer(String uuid, String alias) {
        HostNames.set(this, uuid, alias);
        synchronized (pollingTuples) {
            for (PollingTuple tuple : pollingTuples) {
                if (uuid.equals(tuple.computer.uuid)) {
                    tuple.computer.alias = alias;
                }
            }
        }
    }

    public void removeComputer(ComputerDetails computer) {
        if (!getLocalDatabaseReference()) {
            return;
        }

        try {
            // Its polling first: a poll that ends after this saves and shows nothing (see runPoll).
            synchronized (pollingTuples) {
                for (PollingTuple tuple : pollingTuples) {
                    if (tuple.computer.uuid.equals(computer.uuid)) {
                        tuple.removed = true;
                        if (tuple.thread != null) {
                            // Interrupt the thread on this entry
                            tuple.thread.interrupt();
                            tuple.thread = null;
                        }
                        pollingTuples.remove(tuple);
                        break;
                    }
                }
            }

            // Under the lock a poll saves under, after the tuple is marked (see runPoll).
            synchronized (removalLock) {
                // Remove it from the database
                dbManager.deleteComputer(computer);
                // Before its stored targets go, so an ask still in flight can't store them again.
                wakeTargets.forget(computer.uuid);
                HostStreamProfile.reset(this, computer.uuid);
                HostDisplayChoice.save(this, computer.uuid, HostDisplayChoice.HOST_DEFAULT);
                HostCapabilityCache.forget(this, computer.uuid);
                com.limelight.nativeapi.ConsoleRepository.forgetHost(this, computer.uuid);
                HostNotices.forget(this, computer.uuid);
                HostNames.forget(this, computer.uuid);
                PinnedAddresses.set(this, computer.uuid, null);
            }
            com.limelight.preferences.AppQuitOnLeave.forget(this, computer.uuid);
            com.limelight.preferences.AppStreamSettings.forget(this, computer.uuid);
            com.limelight.preferences.AutoBitrateChoice.forget(this, computer.uuid);
            getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, MODE_PRIVATE)
                    .edit().remove(computer.uuid).apply();
            FrontendExportStore.forgetHost(this, computer.uuid);
            CacheHelper.deleteCacheFile(getCacheDir(), "applist", computer.uuid);
            new DiskAssetLoader(this).deleteAssetsForComputer(computer.uuid);

            // Discovery skips hosts that already answered, so the deleted PC would never return.
            DiscoveryService.DiscoveryBinder discovery = discoveryBinder;
            if (discovery != null) {
                discovery.forgetAnsweredComputers();
            }
        } finally {
            releaseLocalDatabaseReference();
        }
    }

    private boolean getLocalDatabaseReference() {
        int count;
        do {
            count = dbRefCount.get();
            if (count == 0) {
                return false;
            }
        } while (!dbRefCount.compareAndSet(count, count + 1));
        return true;
    }

    private void releaseLocalDatabaseReference() {
        if (dbRefCount.decrementAndGet() == 0) {
            dbManager.close();
        }
    }

    private ComputerDetails tryPollIp(ComputerDetails details, ComputerDetails.AddressTuple address) {
        try {
            // If the current address's port number matches the active address's port number, we can also assume
            // the HTTPS port will also match. This assumption is currently safe because Sunshine sets all ports
            // as offsets from the base HTTP port and doesn't allow custom HttpsPort responses for WAN vs LAN.
            boolean portMatchesActiveAddress = details.state == ComputerDetails.State.ONLINE &&
                    details.activeAddress != null && address.port == details.activeAddress.port;

            NvHTTP http = new NvHTTP(address, portMatchesActiveAddress ? details.httpsPort : 0, idManager.getUniqueId(), details.serverCert,
                    PlatformBinding.getCryptoProvider(ComputerManagerService.this));

            // If this PC is currently online at this address, extend the timeouts to allow more time for the PC to respond.
            boolean isLikelyOnline = details.state == ComputerDetails.State.ONLINE && address.equals(details.activeAddress);

            ComputerDetails newDetails = http.getComputerDetails(isLikelyOnline);

            // Check if this is the PC we expected
            if (newDetails.uuid == null) {
                LimeLog.severe("Polling returned no UUID!");
                return null;
            }
            // details.uuid can be null on initial PC add
            else if (details.uuid != null && !details.uuid.equals(newDetails.uuid)) {
                // We got the wrong PC!
                LimeLog.info("Polling returned the wrong PC!");
                return null;
            }

            return newDetails;
        } catch (XmlPullParserException e) {
            e.printStackTrace();
            return null;
        } catch (IOException e) {
            return null;
        }
    }

    private static class ParallelPollTuple {
        public ComputerDetails.AddressTuple address;
        public ComputerDetails existingDetails;

        public boolean complete;
        public Thread pollingThread;
        public ComputerDetails returnedDetails;

        public ParallelPollTuple(ComputerDetails.AddressTuple address, ComputerDetails existingDetails) {
            this.address = address;
            this.existingDetails = existingDetails;
        }

        public void interrupt() {
            if (pollingThread != null) {
                pollingThread.interrupt();
            }
        }
    }

    private void startParallelPollThread(ParallelPollTuple tuple, HashSet<ComputerDetails.AddressTuple> uniqueAddresses) {
        // Don't bother starting a polling thread for an address that doesn't exist
        // or if the address has already been polled with an earlier tuple
        if (tuple.address == null || !uniqueAddresses.add(tuple.address)) {
            tuple.complete = true;
            tuple.returnedDetails = null;
            return;
        }

        tuple.pollingThread = new Thread() {
            @Override
            public void run() {
                ComputerDetails details = tryPollIp(tuple.existingDetails, tuple.address);

                synchronized (tuple) {
                    tuple.complete = true; // Done
                    tuple.returnedDetails = details; // Polling result

                    tuple.notify();
                }
            }
        };
        tuple.pollingThread.setName("Parallel Poll - "+tuple.address+" - "+tuple.existingDetails.name);
        tuple.pollingThread.start();
    }

    private ComputerDetails parallelPollPc(ComputerDetails details) throws InterruptedException {
        // A pinned PC is polled, and so streamed to, only at the address the user chose.
        if (details.pinnedAddress != null) {
            ComputerDetails polled = tryPollIp(details, details.pinnedAddress);
            if (polled != null) {
                polled.activeAddress = details.pinnedAddress;
            }
            return polled;
        }
        ParallelPollTuple localInfo = new ParallelPollTuple(details.localAddress, details);
        ParallelPollTuple manualInfo = new ParallelPollTuple(details.manualAddress, details);
        ParallelPollTuple remoteInfo = new ParallelPollTuple(details.remoteAddress, details);
        ParallelPollTuple ipv6Info = new ParallelPollTuple(details.ipv6Address, details);

        // These must be started in order of precedence for the deduplication algorithm
        // to result in the correct behavior.
        HashSet<ComputerDetails.AddressTuple> uniqueAddresses = new HashSet<>();
        startParallelPollThread(localInfo, uniqueAddresses);
        startParallelPollThread(manualInfo, uniqueAddresses);
        startParallelPollThread(remoteInfo, uniqueAddresses);
        startParallelPollThread(ipv6Info, uniqueAddresses);

        try {
            // Check local first
            synchronized (localInfo) {
                while (!localInfo.complete) {
                    localInfo.wait();
                }

                if (localInfo.returnedDetails != null) {
                    localInfo.returnedDetails.activeAddress = localInfo.address;
                    return localInfo.returnedDetails;
                }
            }

            // Now manual
            synchronized (manualInfo) {
                while (!manualInfo.complete) {
                    manualInfo.wait();
                }

                if (manualInfo.returnedDetails != null) {
                    manualInfo.returnedDetails.activeAddress = manualInfo.address;
                    return manualInfo.returnedDetails;
                }
            }

            // Now remote IPv4
            synchronized (remoteInfo) {
                while (!remoteInfo.complete) {
                    remoteInfo.wait();
                }

                if (remoteInfo.returnedDetails != null) {
                    remoteInfo.returnedDetails.activeAddress = remoteInfo.address;
                    return remoteInfo.returnedDetails;
                }
            }

            // Now global IPv6
            synchronized (ipv6Info) {
                while (!ipv6Info.complete) {
                    ipv6Info.wait();
                }

                if (ipv6Info.returnedDetails != null) {
                    ipv6Info.returnedDetails.activeAddress = ipv6Info.address;
                    return ipv6Info.returnedDetails;
                }
            }
        } finally {
            // Stop any further polling if we've found a working address or we've been
            // interrupted by an attempt to stop polling.
            localInfo.interrupt();
            manualInfo.interrupt();
            remoteInfo.interrupt();
            ipv6Info.interrupt();
        }

        return null;
    }

    /** The PC's native API, or null before it is paired (no address or pinned certificate). */
    NativeApiClient nativeClient(ComputerDetails computer) {
        HostEndpoint endpoint = HostEndpoint.of(computer, idManager.getUniqueId(), PlatformBinding.getCryptoProvider(this));
        return endpoint == null ? null : new NativeApiClient(new JniNativeApiTransport(), endpoint);
    }

    boolean pollComputer(ComputerDetails details) throws InterruptedException {
        RedactedLog.addComputerSecrets(details);
        // Poll all addresses in parallel to speed up the process
        LimeLog.info("Starting parallel poll for "+details.name+" ("+details.localAddress +", "+details.remoteAddress +", "+details.manualAddress+", "+details.ipv6Address+")");
        ComputerDetails polledDetails = parallelPollPc(details);
        LimeLog.info("Parallel poll for "+details.name+" returned address: "+details.activeAddress);

        if (polledDetails != null) {
            RedactedLog.addComputerSecrets(polledDetails);
            details.update(polledDetails);
            return true;
        }
        else {
            return false;
        }
    }

    @Override
    public void onCreate() {
        super.onCreate();
        // Bind to the discovery service
        discoveryServiceBound = bindService(new Intent(this, DiscoveryService.class),
                discoveryServiceConnection, Service.BIND_AUTO_CREATE);

        // Lookup or generate this device's UID
        idManager = new IdentityManager(this);
        wakeRequests = Executors.newSingleThreadExecutor(task -> new Thread(task, "Wake targets"));
        wakeTargets = new WakeTargetsPoll(new WakeTargets(this), this::nativeClient, wakeRequests,
                SystemClock::elapsedRealtime);

        // Initialize the DB
        dbManager = new ComputerDatabaseManager(this);
        dbRefCount.set(1);

        // Grab known machines into our computer list
        if (!getLocalDatabaseReference()) {
            return;
        }

        for (ComputerDetails computer : dbManager.getAllComputers()) {
            // Offline PCs keep showing what they last reported
            computer.caps = HostCapabilityCache.load(this, computer.uuid);
            // Add tuples for each computer
            addTuple(computer);
        }

        releaseLocalDatabaseReference();

        // Monitor for network changes to invalidate our PC state
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            networkCallback = new ConnectivityManager.NetworkCallback() {
                @Override
                public void onAvailable(Network network) {
                    LimeLog.info("Resetting PC state for new available network");
                    synchronized (pollingTuples) {
                        for (PollingTuple tuple : pollingTuples) {
                            tuple.computer.state = ComputerDetails.State.UNKNOWN;
                            ComputerManagerListener currentListener = listener;
                            if (currentListener != null) {
                                currentListener.notifyComputerUpdated(tuple.computer);
                            }
                        }
                    }
                }

                @Override
                public void onLost(Network network) {
                    LimeLog.info("Offlining PCs due to network loss");
                    synchronized (pollingTuples) {
                        for (PollingTuple tuple : pollingTuples) {
                            tuple.computer.state = ComputerDetails.State.OFFLINE;
                            ComputerManagerListener currentListener = listener;
                            if (currentListener != null) {
                                currentListener.notifyComputerUpdated(tuple.computer);
                            }
                        }
                    }
                }
            };

            ConnectivityManager connMgr = (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
            connMgr.registerDefaultNetworkCallback(networkCallback);
        }
    }

    @Override
    public void onDestroy() {
        synchronized (discoveryServiceConnection) {
            destroyed = true;
            discoveryServiceConnection.notifyAll();
        }
        onUnbind(null);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            ConnectivityManager connMgr = (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
            connMgr.unregisterNetworkCallback(networkCallback);
        }

        if (discoveryServiceBound) {
            unbindService(discoveryServiceConnection);
            discoveryServiceBound = false;
        }
        discoveryBinder = null;
        wakeRequests.shutdownNow();
        wakeTargets.shutdown();

        // FIXME: Should await termination here but we have timeout issues in HttpURLConnection

        // Remove the initial DB reference
        releaseLocalDatabaseReference();
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return binder;
    }

    public class ApplistPoller {
        private volatile Thread thread;
        private final ComputerDetails computer;
        private final Object pollEvent = new Object();
        private boolean receivedAppList = false;
        private NvHTTP activeRequest;

        public ApplistPoller(ComputerDetails computer) {
            this.computer = computer;
        }

        public void pollNow() {
            synchronized (pollEvent) {
                pollEvent.notify();
            }
        }

        private boolean waitPollingDelay() {
            if (!isCurrentWorker()) return false;
            try {
                synchronized (pollEvent) {
                    if (receivedAppList) {
                        // If we've already reported an app list successfully,
                        // wait the full polling period
                        pollEvent.wait(APPLIST_POLLING_PERIOD_MS);
                    }
                    else {
                        // If we've failed to get an app list so far, retry much earlier
                        pollEvent.wait(APPLIST_FAILED_POLLING_RETRY_MS);
                    }
                }
            } catch (InterruptedException e) {
                return false;
            }

            return isCurrentWorker();
        }

        private boolean isCurrentWorker() {
            return thread == Thread.currentThread() && !Thread.currentThread().isInterrupted();
        }

        private synchronized boolean saveAppList(String appList) {
            // stop()/start() may replace the worker while its HTTP response is in flight.
            // Commit the cache and current library together before releasing this generation.
            if (!isCurrentWorker()) return false;
            try (OutputStream cacheOut = CacheHelper.openCacheFileForOutput(
                    getCacheDir(), "applist", computer.uuid)) {
                CacheHelper.writeStringToOutputStream(cacheOut, appList);
            } catch (IOException e) {
                e.printStackTrace();
            }
            computer.rawAppList = appList;
            receivedAppList = true;
            return true;
        }

        private PollingTuple getPollingTuple(ComputerDetails details) {
            synchronized (pollingTuples) {
                for (PollingTuple tuple : pollingTuples) {
                    if (details.uuid.equals(tuple.computer.uuid)) {
                        return tuple;
                    }
                }
            }

            return null;
        }

        public synchronized void start() {
            if (thread != null) {
                return;
            }
            thread = new Thread() {
                @Override
                public void run() {
                    int emptyAppListResponses = 0;
                    do {
                        if (!isCurrentWorker()) return;
                        // Can't poll if it's not online or paired
                        if (computer.state != ComputerDetails.State.ONLINE ||
                                computer.pairState != PairingManager.PairState.PAIRED) {
                            ComputerManagerListener currentListener = listener;
                            if (currentListener != null) {
                                currentListener.notifyComputerUpdated(computer);
                            }
                            continue;
                        }

                        // Can't poll if there's no UUID yet
                        if (computer.uuid == null) {
                            continue;
                        }

                        PollingTuple tuple = getPollingTuple(computer);

                        try {
                            NvHTTP http = new NvHTTP(ServerHelper.getCurrentAddressFromComputer(computer), computer.httpsPort, idManager.getUniqueId(),
                                    computer.serverCert, PlatformBinding.getCryptoProvider(ComputerManagerService.this));
                            synchronized (ApplistPoller.this) {
                                if (!isCurrentWorker()) return;
                                activeRequest = http;
                            }

                            String appList;
                            try {
                                if (tuple != null) {
                                    // Never hold the poller's lifecycle lock across network I/O.
                                    synchronized (tuple.networkLock) {
                                        if (!isCurrentWorker()) return;
                                        appList = http.getAppListRaw();
                                    }
                                } else {
                                    if (!isCurrentWorker()) return;
                                    appList = http.getAppListRaw();
                                }
                            } finally {
                                synchronized (ApplistPoller.this) {
                                    if (activeRequest == http) activeRequest = null;
                                }
                            }
                            if (!isCurrentWorker()) return;

                            List<NvApp> list = NvHTTP.getAppListByReader(new StringReader(appList));
                            if (list.isEmpty()) {
                                LimeLog.warning("Empty app list received from "+computer.uuid);

                                // The app list might actually be empty, so if we get an empty response a few times
                                // in a row, we'll go ahead and believe it.
                                emptyAppListResponses++;
                            }
                            if (!appList.isEmpty() &&
                                    (!list.isEmpty() || emptyAppListResponses >= EMPTY_LIST_THRESHOLD)) {
                                if (!saveAppList(appList)) return;

                                // Reset empty count if it wasn't empty this time
                                if (!list.isEmpty()) {
                                    emptyAppListResponses = 0;
                                }

                                // Notify that the app list has been updated
                                // outside the lifecycle lock, and only for the current worker.
                                ComputerManagerListener currentListener = listener;
                                if (currentListener != null && isCurrentWorker()) {
                                    currentListener.notifyAppListUpdated(computer, true);
                                }
                                // Launcher exports follow added, renamed and removed games.
                                FrontendSync.appListChanged(ComputerManagerService.this, computer);
                            }
                            else if (appList.isEmpty()) {
                                LimeLog.warning("Null app list received from "+computer.uuid);
                                ComputerManagerListener currentListener = listener;
                                if (currentListener != null && isCurrentWorker()) {
                                    currentListener.notifyAppListUpdated(computer, false);
                                }
                            }
                        } catch (IOException | XmlPullParserException e) {
                            if (!isCurrentWorker()) return;
                            e.printStackTrace();
                            ComputerManagerListener currentListener = listener;
                            if (currentListener != null && isCurrentWorker()) {
                                currentListener.notifyAppListUpdated(computer, false);
                            }
                        }
                    } while (waitPollingDelay());
                }
            };
            thread.setName("App list polling thread for " + computer.name);
            thread.start();
        }

        public synchronized void stop() {
            if (thread != null) {
                Thread stopping = thread;
                thread = null;
                if (activeRequest != null) {
                    activeRequest.cancelPendingRequests();
                    activeRequest = null;
                }
                // Cancel without joining the network worker on the UI thread.
                stopping.interrupt();
            }
        }
    }
}

class PollingTuple {
    public Thread thread;
    public final ComputerDetails computer;
    public final Object networkLock;
    public long lastSuccessfulPollMs;
    /** Set by removeComputer before it deletes the PC: a poll still running saves and shows nothing. */
    public volatile boolean removed;

    public PollingTuple(ComputerDetails computer, Thread thread) {
        this.computer = computer;
        this.thread = thread;
        this.networkLock = new Object();
    }
}

class ReachabilityTuple {
    public final String reachableAddress;
    public final ComputerDetails computer;

    public ReachabilityTuple(ComputerDetails computer, String reachableAddress) {
        this.computer = computer;
        this.reachableAddress = reachableAddress;
    }
}
