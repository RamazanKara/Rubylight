package com.limelight.preferences;

import android.content.Context;

import com.limelight.R;

import java.util.ArrayList;
import java.util.List;

/** What the stream settings layers say to the player. */
public final class StreamLayersText {
    private StreamLayersText() {}

    public static String network(Context context, StreamLayers.Network network) {
        switch (network) {
            case MOBILE: return context.getString(R.string.profiles_network_mobile);
            case HOME_WIFI: return context.getString(R.string.profiles_network_home_wifi);
            case WIFI: return context.getString(R.string.profiles_network_wifi);
            default: return context.getString(R.string.profiles_network_other);
        }
    }

    /** "profile Hotel, chosen for Mobile data", or "profile Hotel"; null when no profile applies. */
    public static String profile(Context context, StreamLayers.Result result) {
        if (result.profileName == null) return null;
        return result.reason == StreamLayers.Reason.NETWORK ?
                context.getString(R.string.layers_part_profile_network, result.profileName, network(context, result.network)) :
                context.getString(R.string.layers_part_profile, result.profileName);
    }

    /** The layers in force, e.g. "Settings: profile Hotel + this PC"; null for the main settings alone. */
    public static String applied(Context context, StreamLayers.Result result) {
        List<String> parts = new ArrayList<>();
        String profile = profile(context, result);
        if (profile != null) parts.add(profile);
        if (result.pcApplied) parts.add(context.getString(R.string.layers_part_pc));
        if (result.gameApplied) parts.add(context.getString(R.string.layers_part_game));
        if (result.bitrateLimited) parts.add(context.getString(R.string.layers_part_mobile_limit));
        return parts.isEmpty() ? null : context.getString(R.string.layers_applied, String.join(" + ", parts));
    }

    /** One message per problem, each saying what applies instead. */
    public static List<String> problems(Context context, StreamLayers.Result result) {
        List<String> messages = new ArrayList<>();
        for (StreamLayers.Problem problem : result.problems) {
            switch (problem) {
                case PROFILE_MISSING:
                    messages.add(result.reason == StreamLayers.Reason.NETWORK ?
                            context.getString(R.string.layers_problem_profile_missing_network, network(context, result.network)) :
                            context.getString(R.string.layers_problem_profile_missing));
                    break;
                case PROFILE_INVALID:
                    messages.add(context.getString(R.string.layers_problem_profile_invalid, result.invalidProfileName));
                    break;
                case PC_INVALID:
                    messages.add(context.getString(R.string.layers_problem_pc_invalid));
                    break;
                case GAME_INVALID:
                    messages.add(context.getString(R.string.layers_problem_game_invalid));
                    break;
                default:
                    messages.add(context.getString(R.string.layers_problem_home_wifi_unknown));
                    break;
            }
        }
        return messages;
    }

    /** Where a value not set on this screen comes from, e.g. "from profile Hotel". */
    public static String source(Context context, StreamLayers.Result result, StreamLayers.Source source) {
        switch (source) {
            case PROFILE: return context.getString(R.string.layers_from_profile, result.profileName);
            case PC: return context.getString(R.string.layers_from_pc);
            case GAME: return context.getString(R.string.layers_from_game);
            case MOBILE_LIMIT: return context.getString(R.string.layers_from_mobile_limit);
            default: return context.getString(R.string.layers_from_main);
        }
    }
}
