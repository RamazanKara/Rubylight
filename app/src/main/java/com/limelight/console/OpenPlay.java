package com.limelight.console;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.os.Bundle;
import android.os.Handler;
import android.preference.PreferenceManager;
import android.widget.Toast;

import androidx.annotation.VisibleForTesting;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.PreferencesRecentsStore;
import com.limelight.nativeapi.RecentsStore;
import com.limelight.utils.LaunchSource;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * "When Nightjar opens": whether this start of the app goes into a game, and the guard against a start that comes to
 * nothing. The guard is armed when a countdown ends and cleared only by a stream that lasted or by the player's own
 * doing, so a failure, a crash or a kill during a start all skip the next one, once. {@link OpenPlayModel} decides.
 */
public final class OpenPlay {
    /** The setting: {@code pcs}, {@code last} or {@code chosen}. */
    public static final String SETTING = "list_open_on_start";
    private static final String DEFAULT = "pcs";
    private static final String FILE = "open_play";
    private static final String ARMED = "armed";
    private static final String GAME = "game";
    /** In {@link #GAME}'s saved game, beside its app id. */
    private static final String STABLE_ID = "stable_id";

    /** Screens created in this process: only the first can be a cold start. */
    private static final AtomicInteger screens = new AtomicInteger();

    private OpenPlay() { }

    public static void install(Application application) {
        application.registerActivityLifecycleCallbacks(new Application.ActivityLifecycleCallbacks() {
            @Override public void onActivityCreated(Activity activity, Bundle state) { screens.incrementAndGet(); }
            @Override public void onActivityStarted(Activity activity) { }
            @Override public void onActivityResumed(Activity activity) { }
            @Override public void onActivityPaused(Activity activity) { }
            @Override public void onActivityStopped(Activity activity) { }
            @Override public void onActivitySaveInstanceState(Activity activity, Bundle state) { }
            @Override public void onActivityDestroyed(Activity activity) { }
        });
    }

    private static SharedPreferences state(Context context) {
        return context.getApplicationContext().getSharedPreferences(FILE, Context.MODE_PRIVATE);
    }

    /** One game to open. */
    private static final class Game {
        final String hostUuid;
        final String hostName;
        final int appId;
        final String title;
        /** What the host calls the game across id changes; empty when it gave nothing or for an older save. */
        final String stableId;

        Game(String hostUuid, String hostName, int appId, String title, String stableId) {
            this.hostUuid = hostUuid;
            this.hostName = hostName;
            this.appId = appId;
            this.title = title;
            this.stableId = stableId;
        }

        Intent page(Context context) {
            return new Intent(context, GameDetailActivity.class)
                    .putExtra(GameDetailActivity.EXTRA_HOST_UUID, hostUuid)
                    .putExtra(GameDetailActivity.EXTRA_APP_ID, appId)
                    .putExtra(GameDetailActivity.EXTRA_TITLE, title)
                    .putExtra(GameDetailActivity.EXTRA_HOST_NAME, hostName)
                    .putExtra(GameDetailActivity.EXTRA_STABLE_ID, stableId)
                    .putExtra(GameDetailActivity.EXTRA_OPEN_PLAY, true);
        }
    }

    private static Game chosen(Context context) {
        String saved = state(context).getString(GAME, null);
        if (saved == null) return null;
        try {
            JSONObject json = new JSONObject(saved);
            return new Game(json.getString("host"), json.optString("host_name"), json.getInt("app"), json.optString("title"),
                    json.optString(STABLE_ID));
        } catch (JSONException e) {
            LimeLog.warning("Automatic start: the chosen game is unreadable: " + e);
            return null;
        }
    }

    /** The chosen game's title for Settings' "A chosen game" entry; null while no game is chosen. */
    public static String chosenTitle(Context context) {
        Game game = chosen(context);
        if (game == null) return null;
        return game.title == null || game.title.isEmpty() ? game.hostName : game.title;
    }

    private static Game last(Context context) {
        List<RecentsStore.Recent> recents = new PreferencesRecentsStore(context).load();
        if (recents.isEmpty()) return null;
        RecentsStore.Recent recent = recents.get(0);
        return new Game(recent.hostUuid, recent.hostName, recent.appId, recent.title, recent.stableId);
    }

    /** The launcher's start, not a shortcut's, a link's or an app's that asks for something else. */
    private static boolean fromLauncher(Intent intent) {
        return intent != null && Intent.ACTION_MAIN.equals(intent.getAction())
                && (intent.hasCategory(Intent.CATEGORY_LAUNCHER) || intent.hasCategory(Intent.CATEGORY_LEANBACK_LAUNCHER));
    }

    /** The app's launch screen is being created: the game page to open over it, or null to carry on as usual. */
    public static Intent atEntry(Activity activity, Bundle saved) {
        return atEntry(activity, activity.getIntent(), saved != null, screens.get() <= 1);
    }

    @VisibleForTesting
    static Intent atEntry(Context context, Intent intent, boolean restored, boolean firstScreen) {
        boolean fromRecents = intent != null && (intent.getFlags() & Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY) != 0;
        String opens = PreferenceManager.getDefaultSharedPreferences(context).getString(SETTING, DEFAULT);
        Game last = last(context);
        Game chosen = chosen(context);
        OpenPlayModel.Decision decision = OpenPlayModel.decide(firstScreen, fromLauncher(intent), restored, fromRecents, opens,
                last != null, chosen != null, state(context).getBoolean(ARMED, false));
        if (decision.disarm) disarm(context);
        Game game = "chosen".equals(decision.game) ? chosen : "last".equals(decision.game) ? last : null;
        if (game == null) return null;
        switch (decision.action) {
            case "countdown":
                return game.page(context);
            case "skip":
                Toast.makeText(context, context.getString(R.string.open_play_skipped, game.title), Toast.LENGTH_LONG).show();
                return null;
            default:
                return null;
        }
    }

    /** The countdown ended: until something clears this, the start did not work out. */
    static void arm(Context context) {
        // On disk before anything can go wrong: a crash in the start is what this is for.
        if (!state(context).edit().putBoolean(ARMED, true).commit()) {
            LimeLog.warning("Automatic start: the guard could not be saved");
        }
    }

    static void disarm(Context context) {
        state(context).edit().putBoolean(ARMED, false).apply();
    }

    @VisibleForTesting
    static boolean armed(Context context) {
        return state(context).getBoolean(ARMED, false);
    }

    /** Whether this stream's intent came from an automatic start, directly or by reconnecting. */
    public static boolean isAutoStart(Intent intent) {
        return LaunchSource.of(intent).endsWith(LaunchSource.OPEN_PLAY);
    }

    /** A stream of an automatic start ended or ran this far: clears the guard when the model says that counts. */
    public static void streamEnded(Context context, long connectedMs, boolean leftByPlayer) {
        if (OpenPlayModel.streamEndDisarms(connectedMs, leftByPlayer)) disarm(context);
    }

    /** A stream of an automatic start is up: the guard clears once it lasted. Returns what to remove when it stops first. */
    public static Runnable settleAfter(Context context, Handler handler) {
        Context app = context.getApplicationContext();
        long settled = OpenPlayModel.timing()[1];
        Runnable settle = () -> streamEnded(app, settled, false);
        handler.postDelayed(settle, settled);
        return settle;
    }

    /** Whether this game is the chosen one, for its menu's wording. */
    private static boolean isChosen(Context context, String hostUuid, int appId) {
        Game game = chosen(context);
        return game != null && game.hostUuid.equals(hostUuid) && game.appId == appId;
    }

    /** The game menu's row. */
    public static String menuLabel(Context context, String hostUuid, int appId) {
        return context.getString(isChosen(context, hostUuid, appId) ? R.string.open_play_menu_stop : R.string.open_play_menu);
    }

    /** The game menu's choice, or taking it back: the setting follows, so the choice does what it says. */
    public static void toggle(Context context, String hostUuid, String hostName, int appId, String stableId, String title) {
        SharedPreferences settings = PreferenceManager.getDefaultSharedPreferences(context);
        if (isChosen(context, hostUuid, appId)) {
            state(context).edit().remove(GAME).apply();
            if ("chosen".equals(settings.getString(SETTING, DEFAULT))) settings.edit().putString(SETTING, DEFAULT).apply();
            return;
        }
        try {
            String game = new JSONObject().put("host", hostUuid).put("host_name", hostName).put("app", appId)
                    .put("title", title).put(STABLE_ID, stableId).toString();
            state(context).edit().putString(GAME, game).apply();
        } catch (JSONException impossible) {
            throw new IllegalStateException(impossible);
        }
        settings.edit().putString(SETTING, "chosen").apply();
        Toast.makeText(context, context.getString(R.string.open_play_chosen, title), Toast.LENGTH_SHORT).show();
    }

    /** Whether a game's entry on the host is the one saved under this stable id. */
    static boolean sameGame(ConsoleSnapshot.Source source, String stableId) {
        return stableId.equalsIgnoreCase(source.installId) || stableId.equalsIgnoreCase(source.appUuid);
    }

    /** The app id the host lists a saved game under now; {@code appId} when the list does not tell. */
    static int listedAs(ConsoleSnapshot.Host host, String stableId, int appId) {
        if (host != null && !stableId.isEmpty()) {
            for (ConsoleSnapshot.Source source : host.sources) {
                if (sameGame(source, stableId)) return source.appId;
            }
        }
        return appId;
    }

    /** A removed PC's game cannot be opened any more. */
    public static void forgetHost(Context context, String hostUuid) {
        Game game = chosen(context);
        if (game != null && game.hostUuid.equals(hostUuid)) state(context).edit().remove(GAME).apply();
    }
}
