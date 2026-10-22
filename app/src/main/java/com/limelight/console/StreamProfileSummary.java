package com.limelight.console;

import android.content.Context;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.preferences.StreamLayersText;

import java.util.Locale;

/**
 * The stream settings a game would start with on a host, for the detail screen: the values the existing
 * layers resolve (main settings, a profile, the host's and the game's own values) and which layers apply.
 */
public final class StreamProfileSummary {
    public final String values;
    public final String layers;

    private StreamProfileSummary(String values, String layers) {
        this.values = values;
        this.layers = layers;
    }

    public static StreamProfileSummary of(Context context, ComputerDetails host, String appUuid, int appId) {
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(context, host.uuid, host.caps,
                appUuid, appId, null);
        return new StreamProfileSummary(values(context, config.width, config.height, config.fps, config.bitrate),
                config.layers == null ? null : StreamLayersText.applied(context, config.layers));
    }

    static String values(Context context, int width, int height, int fps, int bitrateKbps) {
        String mbps = bitrateKbps % 1000 == 0 ? Integer.toString(bitrateKbps / 1000)
                : String.format(Locale.getDefault(), "%.1f", bitrateKbps / 1000f);
        return context.getString(R.string.console_profile_values, width, height, fps, mbps);
    }

    /** The second line: the layers in force, or a note that the main settings apply. */
    public String layersOrMain(Context context) {
        return layers != null ? layers : context.getString(R.string.console_profile_main);
    }
}
