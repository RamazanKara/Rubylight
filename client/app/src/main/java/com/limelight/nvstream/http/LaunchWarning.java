/**
 * What a host's launch warning shows. The shared Rust parser (client/protocol/client/src/http/
 * launch_warning.rs) has already checked the reply and cleaned the code and message.
 */
package com.limelight.nvstream.http;

public final class LaunchWarning {
    public static final String VIRTUAL_DISPLAY_UNAVAILABLE = "virtual_display_unavailable";
    public static final String VIRTUAL_DISPLAY_DISABLED_BY_CLIENT = "virtual_display_disabled_by_client";

    private LaunchWarning() {}

    /**
     * The PC's own text when it sent one, else this app's text for a known code; null for no
     * warning or an unknown code without text.
     */
    public static String text(String code, String message, String unavailable, String disabledByClient) {
        if (code == null || code.isEmpty()) {
            return null;
        }
        if (message != null && !message.isEmpty()) {
            return message;
        }
        switch (code) {
            case VIRTUAL_DISPLAY_UNAVAILABLE: return unavailable;
            case VIRTUAL_DISPLAY_DISABLED_BY_CLIENT: return disabledByClient;
            default: return null;
        }
    }
}
