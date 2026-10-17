package com.limelight.binding.video;

/** Reports display policy changes without changing votes or treating a selected policy as capability. */
public final class DisplayRefreshDiagnostics {
    private String previous;

    /** Logical display/mode IDs only; no display names, serial numbers or hardware identifiers. */
    public String update(int api, int displayId, int modeId, boolean arrSupported,
                         boolean requested, boolean automaticPolicy, int windowModeId,
                         float windowRefreshRate, Float appSurfaceFrameRate) {
        String state = "Display refresh policy: api=" + api + ", display=" + displayId +
                ", mode=" + (api >= 23 ? Integer.toString(modeId) : "unavailable") +
                ", arrSupported=" + (api >= 36 ? Boolean.toString(arrSupported) : "unavailable") +
                ", selected=" + (!requested ? "disabled" : automaticPolicy ? "ARR" : "max Hz") +
                ", windowMode=" + (api >= 23 ? Integer.toString(windowModeId) : "unavailable") +
                ", windowHz=" + windowRefreshRate +
                ", appSurfaceHz=" + (appSurfaceFrameRate == null ? "unavailable" : appSurfaceFrameRate);
        if (state.equals(previous)) return null;
        previous = state;
        return state;
    }
}
