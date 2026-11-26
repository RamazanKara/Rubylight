package com.limelight.preferences;

import android.content.Context;
import android.content.Intent;
import android.hardware.display.DisplayManager;
import android.os.Build;
import android.view.Display;

import com.limelight.R;

/**
 * The screen a stream starts on, as a profile kept for a screen sees it (nightjar#163): the device's own screen (both
 * panels of a foldable), or a connected display by the name Android gives it. Which profile a screen calls up is
 * decided by {@code nightjar_policy::display_profile}, as on desktop (see {@link StreamLayers#screenProfile}).
 */
public final class StreamScreen {
    /** The device's own screen. */
    public static final String BUILT_IN = "builtin";
    static final String EXTERNAL = "external:";

    /** The identity profiles are kept by: {@link #BUILT_IN}, or {@code external:} and the display's name. */
    public final String id;
    /** The connected display's name; empty for the device's own screen, which {@link #label} names. */
    public final String name;

    StreamScreen(String id, String name) {
        this.id = id;
        this.name = name == null ? "" : name;
    }

    static StreamScreen builtIn() {
        return new StreamScreen(BUILT_IN, "");
    }

    static StreamScreen external(String name) {
        String trimmed = name == null ? "" : name.trim();
        return trimmed.isEmpty() ? null : new StreamScreen(EXTERNAL + trimmed, trimmed);
    }

    private static final String EXTRA_ID = "StreamScreen";
    private static final String EXTRA_NAME = "StreamScreenName";

    /** Keeps the screen in a stream's intent, so its restarts and its second stream pick as it did. */
    public void putInto(Intent intent) {
        intent.putExtra(EXTRA_ID, id).putExtra(EXTRA_NAME, name);
    }

    /** The screen {@link #putInto} kept in the intent; null for none. */
    public static StreamScreen from(Intent intent) {
        String id = intent.getStringExtra(EXTRA_ID);
        return id == null || id.trim().isEmpty() ? null : new StreamScreen(id, intent.getStringExtra(EXTRA_NAME));
    }

    /** The screen's name as the Profiles page shows it. */
    public String label(Context context) {
        return BUILT_IN.equals(id) || name.isEmpty() ? context.getString(R.string.profiles_screen_built_in) : name;
    }

    /**
     * The screen a stream started now shows on: the connected display the app is on, else a connected display
     * (one that mirrors this screen, as a handheld's or phone's HDMI or USB-C output does), else the device's own.
     * Null when Android can't say; the network choice and the profile in use then decide.
     */
    public static StreamScreen of(Context context) {
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                Display shown = null;
                try {
                    shown = context.getDisplay();
                } catch (UnsupportedOperationException e) {
                    // Not a visual context (the application's): look at the connected displays below.
                }
                if (shown != null && shown.getDisplayId() != Display.DEFAULT_DISPLAY) {
                    StreamScreen screen = external(shown.getName());
                    if (screen != null) return screen;
                }
            }
            DisplayManager manager = (DisplayManager) context.getSystemService(Context.DISPLAY_SERVICE);
            if (manager != null) {
                for (Display display : manager.getDisplays(DisplayManager.DISPLAY_CATEGORY_PRESENTATION)) {
                    // As DualScreen.secondDisplay: a display the app can't show on is not where a stream goes.
                    if (display.getDisplayId() == Display.DEFAULT_DISPLAY || !display.isValid()
                            || (display.getFlags() & Display.FLAG_PRIVATE) != 0) continue;
                    StreamScreen screen = external(display.getName());
                    if (screen != null) return screen;
                }
            }
            return builtIn();
        } catch (RuntimeException e) {
            return null;
        }
    }
}
