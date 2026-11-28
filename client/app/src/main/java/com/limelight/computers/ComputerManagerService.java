package com.limelight.computers;

import android.app.Service;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.SharedPreferences;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.Binder;
import android.os.IBinder;
import android.preference.PreferenceManager;
import android.util.Base64;

import androidx.annotation.VisibleForTesting;

import com.limelight.AppView;
import com.limelight.LimeLog;
import com.limelight.binding.PlatformBinding;
import com.limelight.discovery.DiscoveryService;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.HiddenStore;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.JniNativeApiTransport;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.InviteLink;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingLink;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarHostsJni;
import com.limelight.nvstream.mdns.MdnsComputer;
import com.limelight.nvstream.mdns.MdnsDiscoveryListener;
import com.limelight.nvstream.wol.WakeRelayKeys;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.nvstream.wol.WakeTargetsPoll;
import com.limelight.preferences.AppQuitOnLeave;
import com.limelight.preferences.AppStreamSettings;
import com.limelight.preferences.AutoBitrateChoice;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.preferences.HostStreamProfile;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.utils.CacheHelper;
import com.limelight.utils.FrontendExportStore;
import com.limelight.utils.FrontendSync;
import com.limelight.utils.NetHelper;
import com.limelight.utils.RedactedLog;
import com.limelight.utils.ServerHelper;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.security.cert.CertificateEncodingException;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.RejectedExecutionException;

/**
 * Android foreground/bound service hosting tracked PCs, mDNS binding, network callbacks,
 * VPN-bound STUN lookup, and Android cleanup cascades backed by Rust {@code HostManager}.
 */
public class ComputerManagerService extends Service {
    private static final int MDNS_QUERY_PERIOD_MS = 1000;

    private final ComputerManagerBinder binder = new ComputerManagerBinder();

    private IdentityManager idManager;
    private long hostManagerHandle;
    private Thread eventThread;
    private WakeTargetsPoll wakeTargets;
    private ExecutorService wakeRequests;
    private ExecutorService internetPlayCalls;
    private ExecutorService networkChanges;
    private ExecutorService vpnStunCalls;
    private volatile Network vpnNetwork;
    private final SharedPreferences.OnSharedPreferenceChangeListener internetPlaySetting =
            (preferences, key) -> {
                if (PreferenceConfiguration.INTERNET_PLAY_PREF_STRING.equals(key)) {
                    applyInternetPlay(preferences);
                }
            };
    private volatile ComputerManagerListener listener;
    private volatile boolean pollingActive;
    private volatile boolean destroyed;
    private boolean discoveryServiceBound;

    private ConnectivityManager.NetworkCallback networkCallback;
    private volatile DiscoveryService.DiscoveryBinder discoveryBinder;
    private final ServiceConnection discoveryServiceConnection = new ServiceConnection() {
        public void onServiceConnected(ComponentName className, IBinder binder) {
            synchronized (discoveryServiceConnection) {
                if (destroyed) return;
                DiscoveryService.DiscoveryBinder privateBinder = (DiscoveryService.DiscoveryBinder) binder;
                privateBinder.setListener(createDiscoveryListener());
                discoveryBinder = privateBinder;
                if (pollingActive) privateBinder.startDiscovery(MDNS_QUERY_PERIOD_MS);
                discoveryServiceConnection.notifyAll();
            }
        }

        public void onServiceDisconnected(ComponentName className) {
            discoveryBinder = null;
        }
    };

    public ComputerManagerService() {
    }

    @VisibleForTesting
    boolean pollComputer(ComputerDetails details) {
        if (details == null || hostManagerHandle == 0) return false;
        RedactedLog.addComputerSecrets(details);
        details.anotherCertificate = false;
        String activeAddr = details.activeAddress != null ? details.activeAddress.address : "";
        int activePort = details.activeAddress != null ? details.activeAddress.port : 0;
        String raw = NightjarHostsJni.hostManagerPollDetails(
                hostManagerHandle,
                detailsToDto(details, InternetPlay.reply(this, details.uuid)).toString(),
                details.state.name(),
                activeAddr,
                activePort);
        if (raw == null || raw.isEmpty()) return false;
        try {
            Json.Obj res = Json.parseObject(raw);
            boolean ok = res.optBoolean("ok", false);
            boolean shouldClearCert = res.optBoolean("should_clear_cert", false);
            Json.Obj snap = res.optObject("snapshot");
            if (snap != null) {
                applySnapshot(details, snap, ok);
                RedactedLog.addComputerSecrets(details);
            }
            if (shouldClearCert) {
                details.serverCert = null;
                if (details.uuid != null) { InternetPlay.forget(this, details.uuid); com.limelight.nativeapi.HubSyncController.forget(this, details.uuid); }
            }
            return ok;
        } catch (Json.SyntaxException e) {
            return false;
        }
    }

    public class ComputerManagerBinder extends Binder {
        public boolean isPersistent() { return true; }

        public void startPolling(ComputerManagerListener listener) {
            if (destroyed) return;
            pollingActive = true;
            ComputerManagerService.this.listener = listener;
            DiscoveryService.DiscoveryBinder discovery = discoveryBinder;
            if (discovery != null) discovery.startDiscovery(MDNS_QUERY_PERIOD_MS);
            if (hostManagerHandle != 0) NightjarHostsJni.hostManagerStartPolling(hostManagerHandle);
        }

        public boolean waitForReady() {
            synchronized (discoveryServiceConnection) {
                try {
                    while (discoveryBinder == null && discoveryServiceBound && !destroyed) {
                        discoveryServiceConnection.wait(1000);
                    }
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return false;
                }
                return discoveryBinder != null && !destroyed;
            }
        }

        public void waitForPollingStopped() {
            if (hostManagerHandle != 0) NightjarHostsJni.hostManagerWaitForPollingStopped(hostManagerHandle);
        }

        public boolean addComputerBlocking(ComputerDetails fakeDetails) throws InterruptedException {
            return ComputerManagerService.this.addComputerBlocking(fakeDetails);
        }

        public LinkAdd addComputerForLink(ComputerDetails fakeDetails, PairingLink link) throws InterruptedException {
            return ComputerManagerService.this.addComputerForLink(fakeDetails, link);
        }

        public void removeComputer(ComputerDetails computer) {
            ComputerManagerService.this.removeComputer(computer);
        }

        public boolean addInvitedComputer(ComputerDetails details, String reply) {
            return ComputerManagerService.this.addInvitedComputer(details, reply);
        }

        public boolean inviteClaimsKnownPc(InviteLink invite) {
            return ComputerManagerService.this.inviteClaimsKnownPc(invite);
        }

        public void renameComputer(String uuid, String alias) {
            ComputerManagerService.this.renameComputer(uuid, alias);
        }

        public void pinAddress(String uuid, ComputerDetails.AddressTuple address) {
            ComputerManagerService.this.pinAddress(uuid, address);
        }

        public void forgetCertificate(String uuid) {
            ComputerManagerService.this.forgetCertificate(uuid);
        }

        public void setServerCertificate(String uuid, byte[] certDer) {
            if (hostManagerHandle != 0 && uuid != null) {
                NightjarHostsJni.hostManagerSetServerCertificate(hostManagerHandle, uuid, certDer);
            }
        }

        public long hostManagerHandle() { return hostManagerHandle; }

        public void stopPolling() { ComputerManagerService.this.onUnbind(null); }

        public ApplistPoller createAppListPoller(ComputerDetails computer) { return new ApplistPoller(computer); }

        public String getUniqueId() { return idManager.getUniqueId(); }

        public ComputerDetails getComputer(String uuid) {
            if (hostManagerHandle == 0 || uuid == null) return null;
            String raw = NightjarHostsJni.hostManagerGetSnapshot(hostManagerHandle, uuid);
            if (raw == null || raw.isEmpty()) return null;
            try {
                return snapshotToDetails(Json.parseObject(raw));
            } catch (Json.SyntaxException e) {
                return null;
            }
        }

        /** All saved PCs with their current in-memory snapshots (call off the UI thread). */
        public java.util.List<ComputerDetails> getSavedComputers() {
            if (hostManagerHandle == 0) return java.util.Collections.emptyList();
            ComputerDatabaseManager db = new ComputerDatabaseManager(ComputerManagerService.this);
            java.util.List<ComputerDetails> saved;
            try {
                saved = db.getAllComputers();
            } finally {
                db.close();
            }
            java.util.List<ComputerDetails> out = new java.util.ArrayList<>(saved.size());
            for (ComputerDetails entry : saved) {
                ComputerDetails live = entry.uuid == null ? null : getComputer(entry.uuid);
                out.add(live != null ? live : entry);
            }
            return out;
        }

        public void invalidateStateForComputer(String uuid) {
            if (hostManagerHandle != 0 && uuid != null) {
                NightjarHostsJni.hostManagerInvalidateState(hostManagerHandle, uuid);
            }
        }
    }

    @Override
    public boolean onUnbind(Intent intent) {
        DiscoveryService.DiscoveryBinder discovery = discoveryBinder;
        if (discovery != null) discovery.stopDiscovery();
        pollingActive = false;
        if (hostManagerHandle != 0) NightjarHostsJni.hostManagerStopPolling(hostManagerHandle);
        listener = null;
        return false;
    }

    private MdnsDiscoveryListener createDiscoveryListener() {
        return new MdnsDiscoveryListener() {
            @Override
            public boolean notifyComputerAdded(MdnsComputer computer) {
                ComputerDetails details = new ComputerDetails();
                if (computer.getLocalAddress() != null) {
                    details.localAddress = new ComputerDetails.AddressTuple(
                            computer.getLocalAddress().getHostAddress(), computer.getPort());
                }
                if (computer.getIpv6Address() != null) {
                    details.ipv6Address = new ComputerDetails.AddressTuple(
                            computer.getIpv6Address().getHostAddress(), computer.getPort());
                }
                try {
                    return addComputerBlocking(details);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return false;
                }
            }

            @Override
            public void notifyDiscoveryFailure(Exception e) {
                LimeLog.severe("mDNS discovery failed: " + e);
            }
        };
    }

    public boolean addComputerBlocking(ComputerDetails fakeDetails) throws InterruptedException {
        if (Thread.interrupted()) throw new InterruptedException();
        if (hostManagerHandle == 0 || fakeDetails == null) return false;
        String raw = NightjarHostsJni.hostManagerAddComputerBlocking(
                hostManagerHandle, detailsToDto(fakeDetails, null).toString());
        if (Thread.interrupted()) throw new InterruptedException();
        if (raw == null || raw.isEmpty()) return false;
        try {
            applySnapshot(fakeDetails, Json.parseObject(raw), true);
            return fakeDetails.state == ComputerDetails.State.ONLINE;
        } catch (Json.SyntaxException e) {
            return false;
        }
    }

    public boolean addInvitedComputer(ComputerDetails details, String reply) {
        if (hostManagerHandle == 0 || details == null) return false;
        boolean added = NightjarHostsJni.hostManagerAddInvitedComputer(
                hostManagerHandle, detailsToDto(details, reply).toString(), reply == null ? "" : reply);
        if (added && reply != null) InternetPlay.save(this, details.uuid, reply);
        return added;
    }

    public boolean inviteClaimsKnownPc(InviteLink invite) {
        if (hostManagerHandle == 0 || invite == null || invite.uniqueId == null) return false;
        return NightjarHostsJni.hostManagerInviteClaimsKnownPc(
                hostManagerHandle, invite.uniqueId, invite.certSha256Bytes());
    }

    public enum LinkAdd { ADDED, UNREACHABLE, CERTIFICATE_MISMATCH }

    public LinkAdd addComputerForLink(ComputerDetails fakeDetails, PairingLink link) throws InterruptedException {
        if (Thread.interrupted()) throw new InterruptedException();
        if (hostManagerHandle == 0 || fakeDetails == null || link == null) return LinkAdd.UNREACHABLE;
        String raw = NightjarHostsJni.hostManagerAddComputerForLink(
                hostManagerHandle, detailsToDto(fakeDetails, null).toString(), link.certSha256());
        if (Thread.interrupted()) throw new InterruptedException();
        if (raw == null || raw.isEmpty()) return LinkAdd.UNREACHABLE;
        try {
            Json.Obj res = Json.parseObject(raw);
            int outcome = res.optInt("outcome", 1);
            if (outcome == 2) return LinkAdd.CERTIFICATE_MISMATCH;
            if (outcome != 0) return LinkAdd.UNREACHABLE;
            Json.Obj snap = res.optObject("snapshot");
            if (snap != null) applySnapshot(fakeDetails, snap, true);
            return LinkAdd.ADDED;
        } catch (Json.SyntaxException e) {
            return LinkAdd.UNREACHABLE;
        }
    }

    public void pinAddress(String uuid, ComputerDetails.AddressTuple address) {
        if (getBaseContext() != null) PinnedAddresses.set(this, uuid, address);
        if (hostManagerHandle != 0 && uuid != null) {
            NightjarHostsJni.hostManagerPinAddress(
                    hostManagerHandle, uuid, address != null ? address.address : "", address != null ? address.port : 0);
        }
    }

    public void forgetCertificate(String uuid) {
        if (uuid == null) return;
        if (hostManagerHandle != 0) NightjarHostsJni.hostManagerForgetCertificate(hostManagerHandle, uuid);
        if (getBaseContext() != null) { InternetPlay.forget(this, uuid); com.limelight.nativeapi.HubSyncController.forget(this, uuid); }
    }

    public void renameComputer(String uuid, String alias) {
        if (getBaseContext() != null) HostNames.set(this, uuid, alias);
        if (hostManagerHandle != 0 && uuid != null) {
            NightjarHostsJni.hostManagerRenameComputer(hostManagerHandle, uuid, alias == null ? "" : alias);
        }
    }

    public void removeComputer(ComputerDetails computer) {
        if (computer == null || computer.uuid == null) return;
        if (hostManagerHandle != 0) NightjarHostsJni.hostManagerRemoveComputer(hostManagerHandle, computer.uuid);
        wakeTargets.forget(computer.uuid);
        com.limelight.nativeapi.HubSyncController.forget(this, computer.uuid);
        HostStreamProfile.reset(this, computer.uuid);
        HostDisplayChoice.forget(this, computer.uuid);
        HostCapabilityCache.forget(this, computer.uuid);
        InternetPlay.forget(this, computer.uuid);
        ConsoleRepository.forgetHost(this, computer.uuid);
        HostNotices.forget(this, computer.uuid);
        HostNames.forget(this, computer.uuid);
        PinnedAddresses.set(this, computer.uuid, null);
        AppQuitOnLeave.forget(this, computer.uuid);
        AppStreamSettings.forget(this, computer.uuid);
        AutoBitrateChoice.forget(this, computer.uuid);
        getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, Context.MODE_PRIVATE).edit()
                .remove(computer.uuid).remove(HiddenStore.Preferences.idsKey(computer.uuid)).apply();
        FrontendExportStore.forgetHost(this, computer.uuid);
        CacheHelper.deleteCacheFile(getCacheDir(), "applist", computer.uuid);
        new DiskAssetLoader(this).deleteAssetsForComputer(computer.uuid);
        DiscoveryService.DiscoveryBinder discovery = discoveryBinder;
        if (discovery != null) discovery.forgetAnsweredComputers();
    }

    private void applyInternetPlay(SharedPreferences preferences) {
        final boolean on = preferences.getBoolean(PreferenceConfiguration.INTERNET_PLAY_PREF_STRING, true);
        try {
            internetPlayCalls.execute(() -> InternetPlay.setEnabled(on));
        } catch (RejectedExecutionException ignored) {
        }
    }

    @Override
    public IBinder onBind(Intent intent) { return binder; }

    NativeApiClient nativeClient(ComputerDetails computer) {
        HostEndpoint endpoint = HostEndpoint.of(computer, idManager.getUniqueId(),
                PlatformBinding.getCryptoProvider(this));
        return endpoint == null ? null : new NativeApiClient(new JniNativeApiTransport(), endpoint);
    }

    @Override
    public void onCreate() {
        super.onCreate();
        discoveryServiceBound = bindService(new Intent(this, DiscoveryService.class),
                discoveryServiceConnection, Service.BIND_AUTO_CREATE);
        try {
            new ComputerDatabaseManager(this).close();
        } catch (Exception ignored) {
        }
        idManager = new IdentityManager(this);
        String certPem = "";
        String keyPem = "";
        LimelightCryptoProvider crypto = PlatformBinding.getCryptoProvider(this);
        if (crypto != null) {
            try {
                if (crypto.getClientCertificate() != null) certPem = pem("CERTIFICATE", crypto.getClientCertificate().getEncoded());
                if (crypto.getClientPrivateKey() != null) keyPem = pem("PRIVATE KEY", crypto.getClientPrivateKey().getEncoded());
            } catch (CertificateEncodingException ignored) {
            }
        }
        hostManagerHandle = NightjarHostsJni.hostManagerOpen(
                getFilesDir().getAbsolutePath(), idManager.getUniqueId(), certPem, keyPem);
        InternetPlay.migrateLegacyToManager(this, hostManagerHandle);

        wakeRequests = Executors.newSingleThreadExecutor(task -> new Thread(task, "Wake targets"));
        wakeTargets = new WakeTargetsPoll(new WakeTargets(this), new WakeRelayKeys(this), this::nativeClient,
                wakeRequests, () -> hostManagerHandle == 0 ? 0L : NightjarHostsJni.hostManagerNowMs(hostManagerHandle));
        internetPlayCalls = Executors.newSingleThreadExecutor(task -> new Thread(task, "Internet play setting"));
        networkChanges = Executors.newSingleThreadExecutor(task -> new Thread(task, "Network changes"));
        vpnStunCalls = Executors.newSingleThreadExecutor(task -> new Thread(task, "VPN STUN"));

        SharedPreferences preferences = PreferenceManager.getDefaultSharedPreferences(this);
        preferences.registerOnSharedPreferenceChangeListener(internetPlaySetting);
        applyInternetPlay(preferences);

        eventThread = new Thread(this::pumpHostEvents, "HostManager-events");
        eventThread.start();

        ConnectivityManager connMgr = (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
        if (connMgr != null) {
            networkCallback = new ConnectivityManager.NetworkCallback() {
                @Override public void onAvailable(Network network) {
                    updateVpnState(connMgr, network, connMgr.getNetworkCapabilities(network));
                    long atMs = hostManagerHandle == 0 ? 0L : NightjarHostsJni.hostManagerNowMs(hostManagerHandle);
                    applyNetworkChange(true, atMs);
                }
                @Override public void onCapabilitiesChanged(Network network, NetworkCapabilities caps) {
                    updateVpnState(connMgr, network, caps);
                }
                @Override public void onLost(Network network) {
                    if (network != null && network.equals(vpnNetwork)) {
                        vpnNetwork = null;
                        if (hostManagerHandle != 0) NightjarHostsJni.hostManagerSetVpnActive(hostManagerHandle, false);
                    }
                    long atMs = hostManagerHandle == 0 ? 0L : NightjarHostsJni.hostManagerNowMs(hostManagerHandle);
                    applyNetworkChange(false, atMs);
                }
            };
            connMgr.registerDefaultNetworkCallback(networkCallback);
        }
    }

    private void updateVpnState(ConnectivityManager connMgr, Network network, NetworkCapabilities caps) {
        boolean isVpn = caps != null && caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN);
        vpnNetwork = isVpn ? network : null;
        if (hostManagerHandle != 0) NightjarHostsJni.hostManagerSetVpnActive(hostManagerHandle, isVpn);
    }

    private void pumpHostEvents() {
        while (!destroyed && hostManagerHandle != 0) {
            String raw = NightjarHostsJni.hostManagerNextEvent(hostManagerHandle, 250);
            if (raw == null || destroyed) continue;
            try {
                Json.Obj snap = Json.parseObject(raw);
                ComputerDetails details = snapshotToDetails(snap);
                if (details == null) continue;
                RedactedLog.addComputerSecrets(details);
                if (details.uuid != null && details.caps != null) HostCapabilityCache.save(this, details.uuid, details.caps);
                if (snap.optBoolean("needs_vpn_stun", false) && details.uuid != null) {
                    scheduleVpnStun(details.uuid, details.externalPort);
                }
                if (details.state == ComputerDetails.State.ONLINE) wakeTargets.polled(details);
                com.limelight.nativeapi.HubSyncController.onPumpSnapshot(this, details, this::nativeClient);
                ComputerManagerListener currentListener = listener;
                if (currentListener != null && !destroyed) currentListener.notifyComputerUpdated(details);
            } catch (Json.SyntaxException ignored) {
            }
        }
    }

    private void scheduleVpnStun(final String uuid, final int externalPort) {
        if (destroyed) return;
        final ConnectivityManager connMgr = (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
        if (connMgr == null) return;
        try {
            vpnStunCalls.execute(() -> {
                if (destroyed || hostManagerHandle == 0) return;
                Network lan = stunNetwork(connMgr);
                if (lan == null) {
                    LimeLog.info("STUN: no non-VPN network to ask on");
                    return;
                }
                try {
                    String ip = StunClient.query("stun.moonlight-stream.org", 3478, 3000,
                            lan::getAllByName, lan::bindSocket);
                    if (ip != null && !ip.isEmpty() && hostManagerHandle != 0 && !destroyed) {
                        int port = externalPort > 0 ? externalPort : NvHTTP.DEFAULT_HTTP_PORT;
                        NightjarHostsJni.hostManagerSetRemoteEndpoint(hostManagerHandle, uuid, ip, port);
                    }
                } catch (IOException e) {
                    LimeLog.warning("STUN on the non-VPN network failed: " + e);
                }
            });
        } catch (RejectedExecutionException ignored) {
        }
    }

    @VisibleForTesting
    static Network stunNetwork(ConnectivityManager connMgr) {
        for (Network net : connMgr.getAllNetworks()) {
            NetworkCapabilities netCaps = connMgr.getNetworkCapabilities(net);
            if (netCaps != null && !netCaps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)
                    && !netCaps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) {
                return net;
            }
        }
        return null;
    }

    void resetStateForNetworkChange(boolean available, long atMs) {
        if (hostManagerHandle != 0 && !destroyed) {
            NightjarHostsJni.hostManagerResetNetworkState(hostManagerHandle, available, atMs);
        }
    }

    private void applyNetworkChange(boolean available, long atMs) {
        try {
            networkChanges.execute(() -> resetStateForNetworkChange(available, atMs));
        } catch (RejectedExecutionException ignored) {
        }
    }

    @Override
    public void onDestroy() {
        synchronized (discoveryServiceConnection) {
            destroyed = true;
            discoveryServiceConnection.notifyAll();
        }
        onUnbind(null);
        ConnectivityManager connMgr = (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
        if (connMgr != null && networkCallback != null) {
            try {
                connMgr.unregisterNetworkCallback(networkCallback);
            } catch (IllegalArgumentException ignored) {
            }
        }
        if (discoveryServiceBound) {
            unbindService(discoveryServiceConnection);
            discoveryServiceBound = false;
        }
        discoveryBinder = null;
        wakeRequests.shutdownNow();
        wakeTargets.shutdown();
        networkChanges.shutdownNow();
        vpnStunCalls.shutdownNow();
        PreferenceManager.getDefaultSharedPreferences(this).unregisterOnSharedPreferenceChangeListener(internetPlaySetting);
        internetPlayCalls.shutdown();
        long handle = hostManagerHandle;
        hostManagerHandle = 0;
        if (handle != 0) NightjarHostsJni.hostManagerDestroy(handle);
        if (eventThread != null) {
            eventThread.interrupt();
            try { eventThread.join(1000); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
            eventThread = null;
        }
    }

    private ComputerDetails snapshotToDetails(Json.Obj snap) {
        ComputerDetails details = new ComputerDetails();
        applySnapshot(details, snap, true);
        return details;
    }

    private void applySnapshot(ComputerDetails target, Json.Obj snap, boolean applyReachState) {
        String uid = optNull(snap, "uuid");
        if (uid != null && !uid.isEmpty()) target.uuid = uid;
        String name = optNull(snap, "name");
        if (name != null && !name.isEmpty()) target.name = name;
        target.alias = optNull(snap, "alias");
        target.macAddress = optNull(snap, "mac");
        int https = snap.optInt("https_port", 0);
        if (https != 0) target.httpsPort = https;
        int ext = snap.optInt("external_port", 0);
        if (ext != 0) target.externalPort = ext;
        target.localAddress = toTuple(snap.optObject("local"));
        target.manualAddress = toTuple(snap.optObject("manual"));
        target.ipv6Address = toTuple(snap.optObject("ipv6"));
        target.remoteAddress = toTuple(snap.optObject("remote"));
        target.pinnedAddress = toTuple(snap.optObject("pinned"));
        target.activeAddress = toTuple(snap.optObject("active"));
        Json.Obj si = snap.optObject("server_info");
        boolean online = "ONLINE".equals(snap.optString("state", "UNKNOWN"));
        // Before a host has answered /serverinfo, leave pairState null so saved paired hosts wait for
        // their first answer (or appear as offline paired cards if unreachable) instead of looking settled/unpaired.
        target.pairState = (si != null || online)
                ? (snap.optBoolean("paired", false) ? PairingManager.PairState.PAIRED : PairingManager.PairState.NOT_PAIRED)
                : null;
        target.anotherCertificate = snap.optBoolean("another_certificate", false);
        target.runningGameId = snap.optInt("running_game_id", 0);
        target.runningGameUuid = optNull(snap, "running_game_uuid");
        target.unreachable = optNull(snap, "unreachable");
        target.internetPath = optNull(snap, "internet_path");
        String certB64 = optNull(snap, "server_cert_b64");
        if (certB64 != null) target.serverCert = decodeCert(certB64);
        if (si != null) {
            int port = target.activeAddress != null ? target.activeAddress.port : NvHTTP.DEFAULT_HTTP_PORT;
            NvHTTP.applyServerInfoSnapshot(target, si, port);
        } else {
            String capsRaw = optNull(snap, "capabilities_json");
            if (capsRaw != null && !capsRaw.isEmpty()) {
                target.caps = HostCapabilities.fromJson(capsRaw);
            } else if (target.uuid != null && (target.caps == null || target.caps == HostCapabilities.UNKNOWN)) {
                target.caps = HostCapabilityCache.load(this, target.uuid);
            }
        }
        if (applyReachState) {
            try {
                target.state = ComputerDetails.State.valueOf(snap.optString("state", "UNKNOWN"));
            } catch (IllegalArgumentException ignored) {
            }
        }
    }

    private static Json.Obj detailsToDto(ComputerDetails d, String internetPlay) {
        Json.Obj dto = Json.obj()
                .put("uuid", d.uuid == null ? "" : d.uuid)
                .put("name", d.name == null ? "" : d.name)
                .put("alias", d.alias)
                .put("https_port", d.httpsPort)
                .put("mac", d.macAddress)
                .put("local", fromTuple(d.localAddress))
                .put("manual", fromTuple(d.manualAddress))
                .put("ipv6", fromTuple(d.ipv6Address))
                .put("remote", fromTuple(d.remoteAddress))
                .put("pinned", fromTuple(d.pinnedAddress));
        if (internetPlay != null) dto.put("internet_play", internetPlay);
        if (d.serverCert != null) {
            try {
                dto.put("server_cert_b64", Base64.encodeToString(d.serverCert.getEncoded(), Base64.NO_WRAP));
            } catch (CertificateEncodingException ignored) {
            }
        }
        return dto;
    }

    private static Json.Obj fromTuple(ComputerDetails.AddressTuple t) {
        return t == null ? null : Json.obj().put("address", t.address).put("port", t.port);
    }

    private static ComputerDetails.AddressTuple toTuple(Json.Obj o) {
        if (o == null) return null;
        String addr = optNull(o, "address");
        int port = o.optInt("port", 0);
        return addr == null || addr.isEmpty() || port <= 0 ? null : new ComputerDetails.AddressTuple(addr, port);
    }

    private static String optNull(Json.Obj o, String key) {
        return !o.has(key) || o.isNull(key) ? null : o.optString(key, null);
    }

    private static String pem(String label, byte[] der) {
        String b64 = Base64.encodeToString(der, Base64.NO_WRAP);
        StringBuilder sb = new StringBuilder("-----BEGIN ").append(label).append("-----\n");
        for (int i = 0; i < b64.length(); i += 64) {
            sb.append(b64, i, Math.min(i + 64, b64.length())).append('\n');
        }
        return sb.append("-----END ").append(label).append("-----\n").toString();
    }

    private static X509Certificate decodeCert(String b64) {
        try {
            byte[] der = Base64.decode(b64, Base64.DEFAULT);
            return (X509Certificate) CertificateFactory.getInstance("X.509")
                    .generateCertificate(new ByteArrayInputStream(der));
        } catch (IllegalArgumentException | CertificateException e) {
            return null;
        }
    }

    public class ApplistPoller {
        private Thread thread;
        private final ComputerDetails computer;
        private long pollerHandle;
        private boolean startFailedNoEndpoint;

        public ApplistPoller(ComputerDetails computer) { this.computer = computer; }

        /** True when {@link #start()} returned early because the host had no reachable address or cert yet. */
        public synchronized boolean needsRestart() { return startFailedNoEndpoint && thread == null; }

        public void pollNow() {
            long h = pollerHandle;
            if (h != 0) NightjarHostsJni.applistPollerPollNow(h);
        }

        private boolean isCurrentWorker() { return thread == Thread.currentThread() && !Thread.currentThread().isInterrupted(); }

        private synchronized boolean saveAppList(String appList) {
            if (!isCurrentWorker()) return false;
            try (OutputStream out = CacheHelper.openCacheFileForOutput(getCacheDir(), "applist", computer.uuid)) {
                CacheHelper.writeStringToOutputStream(out, appList);
            } catch (IOException e) {
                LimeLog.warning("Applist save failed: " + e.getMessage());
            }
            computer.rawAppList = appList;
            return true;
        }

        public synchronized void start() {
            if (thread != null || hostManagerHandle == 0) return;
            ComputerDetails.AddressTuple addr;
            try {
                addr = ServerHelper.getCurrentAddressFromComputer(computer);
            } catch (IOException e) {
                startFailedNoEndpoint = true;
                return;
            }
            byte[] cert = null;
            if (computer.serverCert != null) {
                try { cert = computer.serverCert.getEncoded(); } catch (CertificateEncodingException ignored) { }
            }
            if (addr == null || cert == null) {
                startFailedNoEndpoint = true;
                return;
            }
            startFailedNoEndpoint = false;
            final long h = NightjarHostsJni.applistPollerStart(
                    hostManagerHandle, addr.address, addr.port, computer.httpsPort, cert);
            if (h == 0) {
                startFailedNoEndpoint = true;
                return;
            }
            pollerHandle = h;
            thread = new Thread(() -> {
                try {
                    while (isCurrentWorker()) {
                        String rawXml = NightjarHostsJni.applistPollerNextUpdate(h);
                        if (rawXml == null || !isCurrentWorker()) break;
                        if (!saveAppList(rawXml)) break;
                        ComputerDetails latest = binder.getComputer(computer.uuid);
                        ComputerDetails delivered = latest != null ? latest : computer;
                        delivered.rawAppList = rawXml;
                        ComputerManagerListener currentListener = listener;
                        if (currentListener != null && isCurrentWorker()) currentListener.notifyAppListUpdated(delivered, true);
                        FrontendSync.appListChanged(ComputerManagerService.this, delivered);
                    }
                } finally {
                    synchronized (ApplistPoller.this) {
                        if (thread == Thread.currentThread()) {
                            thread = null;
                        }
                        if (pollerHandle == h) {
                            pollerHandle = 0;
                            NightjarHostsJni.applistPollerStop(h);
                        }
                    }
                }
            }, "App list polling thread for " + computer.name);
            thread.start();
        }

        public synchronized void stop() {
            long h = pollerHandle;
            pollerHandle = 0;
            if (h != 0) NightjarHostsJni.applistPollerStop(h);
            if (thread != null) {
                Thread stopping = thread;
                thread = null;
                stopping.interrupt();
            }
        }
    }
}
