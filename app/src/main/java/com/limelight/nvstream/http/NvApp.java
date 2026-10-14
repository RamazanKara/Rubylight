package com.limelight.nvstream.http;

import com.limelight.LimeLog;

import java.nio.charset.StandardCharsets;
import java.util.UUID;

public class NvApp {
    public enum Role { STREAM, REMOTE_MONITOR, INPUT_ONLY }
    public enum Control { NONE, RESUME, DISCONNECT_MONITOR, DISCONNECT_INPUT, TERMINATE, MONITOR, INPUT, RUNNING_GAME }

    private String appName = "";
    private int appId;
    private boolean initialized;
    private boolean hdrSupported;
    private String appUuid = "";
    private int hostIndex = Integer.MAX_VALUE;
    private String artVersion = "";
    private boolean monitorResume;
    private boolean controlTiles = true;
    
    public NvApp() {}
    
    public NvApp(String appName) {
        this.appName = appName;
    }
    
    public NvApp(String appName, int appId, boolean hdrSupported) {
        this.appName = appName;
        this.appId = appId;
        this.hdrSupported = hdrSupported;
        this.initialized = true;
    }
    
    public void setAppName(String appName) {
        this.appName = appName;
    }
    
    public void setAppId(String appId) {
        try {
            this.appId = Integer.parseInt(appId);
            this.initialized = true;
        } catch (NumberFormatException e) {
            LimeLog.warning("Malformed app ID: "+appId);
        }
    }
    
    public void setAppId(int appId) {
        this.appId = appId;
        this.initialized = true;
    }

    public void setHdrSupported(boolean hdrSupported) {
        this.hdrSupported = hdrSupported;
    }

    public void setAppUuid(String appUuid) {
        this.appUuid = appUuid == null ? "" : appUuid.trim();
    }

    public String getAppUuid() {
        return appUuid;
    }

    /**
     * Whether this app's host publishes control tiles. Elsewhere the reserved IDs and UUIDs are
     * ordinary apps. A host not yet known keeps them, as before.
     */
    public void setControlTiles(boolean controlTiles) {
        this.controlTiles = controlTiles;
    }

    public Control getControl() {
        if (!controlTiles) {
            return Control.NONE;
        }
        // Rubylight rust/core/src/remote.rs and Vibepollo remote_session.h keep these identities
        // stable across catalogues.
        for (int offset = 1; offset <= 7; offset++) {
            if (appId == 2147483500 + offset || appId == 2147483600 + offset ||
                    ((offset == 1 || offset == 4 || offset == 5 || offset == 6) && appId == 2147483510 + offset) ||
                    appUuid.equals("9a1c5a25-58fe-40e0-b9aa-7d3f0000000" + offset)) {
                return Control.values()[offset];
            }
        }
        return Control.NONE;
    }

    public Role getRole() {
        switch (getControl()) {
            case MONITOR:
            case DISCONNECT_MONITOR:
                return Role.REMOTE_MONITOR;
            case INPUT:
            case DISCONNECT_INPUT:
                return Role.INPUT_ONLY;
            case RESUME:
                return monitorResume ? Role.REMOTE_MONITOR : Role.STREAM;
            default:
                return Role.STREAM;
        }
    }

    public void setMonitorResume(boolean monitorResume) {
        this.monitorResume = monitorResume;
    }

    public boolean isControlAction() {
        Control control = getControl();
        return control == Control.DISCONNECT_MONITOR || control == Control.DISCONNECT_INPUT ||
                control == Control.TERMINATE;
    }

    public void setHostIndex(String index) {
        try {
            int value = Integer.parseInt(index.trim());
            hostIndex = value >= 0 ? value : Integer.MAX_VALUE;
        } catch (NumberFormatException e) {
            hostIndex = Integer.MAX_VALUE;
        }
    }

    public int getHostIndex() {
        return hostIndex;
    }

    public void setArtVersion(String artVersion) {
        this.artVersion = artVersion;
    }

    public String getArtVersion() {
        return artVersion;
    }

    public String getAssetCacheKey() {
        if (appUuid.isEmpty() && artVersion.isEmpty()) {
            return Integer.toString(appId);
        }
        // Host artwork versions are opaque and must not become filesystem paths.
        return appId + "-" + UUID.nameUUIDFromBytes(
                (appUuid + "\0" + artVersion).getBytes(StandardCharsets.UTF_8));
    }

    public boolean matchesRunningApp(int runningId, String runningUuid) {
        if (runningId == 0) {
            return false;
        }
        return !appUuid.isEmpty() && runningUuid != null && !runningUuid.isEmpty() ?
                appUuid.equals(runningUuid) : appId == runningId;
    }

    public int compareForDisplay(NvApp other) {
        int order = Integer.compare(hostIndex, other.hostIndex);
        return order != 0 ? order : appName.compareToIgnoreCase(other.appName);
    }
    
    public String getAppName() {
        return this.appName;
    }
    
    public int getAppId() {
        return this.appId;
    }

    public boolean isHdrSupported() {
        return this.hdrSupported;
    }
    
    public boolean isInitialized() {
        return this.initialized;
    }

    @Override
    public String toString() {
        StringBuilder str = new StringBuilder();
        str.append("Name: ").append(appName).append("\n");
        str.append("HDR Supported: ").append(hdrSupported ? "Yes" : "Unknown").append("\n");
        str.append("ID: ").append(appId).append("\n");
        return str.toString();
    }
}
