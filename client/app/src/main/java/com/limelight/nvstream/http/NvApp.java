package com.limelight.nvstream.http;

import com.limelight.LimeLog;

import java.nio.charset.StandardCharsets;
import java.util.UUID;

public class NvApp {
    public enum Role { STREAM, REMOTE_MONITOR, INPUT_ONLY }
    public enum Control { NONE, RESUME, DISCONNECT_MONITOR, DISCONNECT_INPUT, TERMINATE, MONITOR, INPUT, RUNNING_GAME }

    // Apollo src/process.h. Apollo lists these apps while its input-only mode is on, and Vibepollo
    // still honors them at launch. Its Virtual Display and fallback Desktop apps are ordinary streams.
    static final String APOLLO_REMOTE_INPUT_UUID = "8CB5C136-DA67-4F99-B4A1-F9CD35005CF4";
    static final String APOLLO_TERMINATE_UUID = "E16CBE1B-295D-4632-9A76-EC4180C857D3";

    private String appName = "";
    private int appId;
    private boolean initialized;
    private boolean hdrSupported;
    private String appUuid = "";
    private int hostIndex = Integer.MAX_VALUE;
    private String artVersion = "";
    private boolean monitorResume;
    private boolean controlTiles = true;
    private boolean apolloApps = true;
    private boolean smallStreamInputOnly;
    
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
     * Which reserved identities this app's host gives a meaning: control tiles where the host may
     * publish them, and Apollo's Remote Input and Terminate apps on Apollo and Vibepollo. Elsewhere
     * they are ordinary apps. A host not yet known keeps both, as before.
     */
    public void setHostCapabilities(HostCapabilities caps) {
        controlTiles = caps.mayHave(HostCapabilities.Feature.REMOTE_SESSION_TILES);
        HostCapabilities.Family family = caps.family();
        apolloApps = family == HostCapabilities.Family.APOLLO || family == HostCapabilities.Family.VIBEPOLLO ||
                family == HostCapabilities.Family.UNKNOWN;
    }

    public Control getControl() {
        if (apolloApps && APOLLO_REMOTE_INPUT_UUID.equalsIgnoreCase(appUuid)) {
            return Control.INPUT;
        }
        if (apolloApps && APOLLO_TERMINATE_UUID.equalsIgnoreCase(appUuid)) {
            return Control.TERMINATE;
        }
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
        Control control = getControl();
        if (smallStreamInputOnly && control == Control.NONE) {
            return Role.INPUT_ONLY;
        }
        switch (control) {
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

    /**
     * Joins this game as Input-only on a host without its own Input-only mode: the host streams
     * it as usual at a small size, and this device shows none of it. Control tiles ignore it.
     */
    public void setSmallStreamInputOnly(boolean smallStreamInputOnly) {
        this.smallStreamInputOnly = smallStreamInputOnly;
    }

    public boolean isSmallStreamInputOnly() {
        return smallStreamInputOnly && getControl() == Control.NONE;
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

    /**
     * Whether this is the game the host reports as running. A UUID decides when both sides have
     * one: Apollo's input-only mode reports the running game's UUID with game ID 0.
     */
    public boolean matchesRunningApp(int runningId, String runningUuid) {
        if (!appUuid.isEmpty() && runningUuid != null && !runningUuid.isEmpty()) {
            return appUuid.equals(runningUuid);
        }
        return runningId != 0 && appId == runningId;
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
