package com.limelight.preferences;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.ui.ActionSheet;

import java.util.Map;

/**
 * The stream profile in use and the chooser for it, shared by the library's menu and Home's game page. Choosing
 * one makes it the active profile, as the Profiles page does; a profile kept for this screen, or "Choose by network"
 * for this network, still decides, so its Profiles page opens instead.
 */
public final class StreamProfilePicker {
    private StreamProfilePicker() {
    }

    /** The profile the next stream uses, as the entry names it; null when the player has no profiles. */
    public static String inUse(Context context) {
        StreamProfiles.Snapshot profiles = StreamProfiles.load(context);
        if (profiles.names.isEmpty()) return null;
        StreamLayers.Input input = new StreamLayers.Input();
        input.profiles = profiles;
        StreamNetwork.fill(context, input);
        if (byNetwork(profiles, input)) {
            StreamLayers.Result result = StreamLayers.resolve(input);
            return result.profileName != null ? result.profileName : context.getString(R.string.profiles_none);
        }
        return profiles.exists(profiles.activeId) ? profiles.name(profiles.activeId)
                : context.getString(R.string.profiles_none);
    }

    /**
     * Opens what the profile entry opens: the Profiles page when "Choose by network" decides for this network,
     * else the list of profiles for the caller to show. {@code onChosen} runs after a profile is picked from that
     * list. Null when there are no profiles, or when the Profiles page was opened.
     */
    public static ActionSheet chooser(Activity activity, Runnable onChosen) {
        StreamProfiles.Snapshot profiles = StreamProfiles.load(activity);
        if (profiles.names.isEmpty()) return null;
        StreamLayers.Input input = new StreamLayers.Input();
        input.profiles = profiles;
        StreamNetwork.fill(activity, input);
        if (byNetwork(profiles, input)) {
            activity.startActivity(new Intent(activity, StreamProfilesActivity.class));
            return null;
        }
        ActionSheet picker = new ActionSheet(activity, activity.getString(R.string.profiles_subtitle));
        String current = profiles.exists(profiles.activeId) ? profiles.activeId : null;
        addChoice(picker, activity, null, activity.getString(R.string.profiles_none), current == null, onChosen);
        for (Map.Entry<String, String> entry : profiles.names.entrySet()) {
            addChoice(picker, activity, entry.getKey(), entry.getValue(), entry.getKey().equals(current), onChosen);
        }
        return picker;
    }

    /** A profile kept for this screen, or "Choose by network" for this network, decides instead of the player's pick. */
    private static boolean byNetwork(StreamProfiles.Snapshot profiles, StreamLayers.Input input) {
        return StreamLayers.screenProfile(profiles, input.screen) != null
                || profiles.auto && profiles.rule(input.network) != null;
    }

    private static void addChoice(ActionSheet picker, Activity activity, String id, String name, boolean inUse,
                                  Runnable onChosen) {
        MaterialButton row = inUse
                ? picker.addAction(R.drawable.ic_folder, name, activity.getString(R.string.profiles_marker_in_use))
                : picker.addAction(R.drawable.ic_folder, name);
        row.setOnClickListener(v -> {
            StreamProfiles.setActive(activity, id);
            picker.dismiss();
            if (onChosen != null) onChosen.run();
        });
    }
}
