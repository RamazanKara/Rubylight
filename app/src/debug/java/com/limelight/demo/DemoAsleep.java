package com.limelight.demo;

import android.content.Context;
import android.os.Handler;
import android.os.Looper;

import com.limelight.computers.ComputerManagerListener;
import com.limelight.nativeapi.RecentsStore;
import com.limelight.nvstream.http.ComputerDetails;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * "--ez asleep true" on the launcher: the offline Laptop has the native API and a library it saved while it was
 * awake, two games of which Living-room PC has too, so Home shows a PC that is away with its games, the "which PC?"
 * question and Wake PC and play. Waking it sends nothing: the Laptop answers {@link #WAKE_MS} after the press.
 * Without the extra the demo is as before, and a wake press still sends nothing (the Laptop just never answers).
 */
final class DemoAsleep {
    static final String LAPTOP_UUID = "d3e00000-0000-4000-8000-000000000003";
    static final long WAKE_MS = 3_000;
    /** The games both PCs list, by Living-room PC's app id; the Laptop lists them under the same game id. */
    static final int[] SHARED = {3, 4};
    /** The Laptop's own games; app ids 11 and up. */
    static final String[] LAPTOP_ONLY = {"Lunar Base", "Kart Party", "Deep Dungeon"};

    /** Unix seconds; later than any game of Living-room PC. */
    private static final long LAST_PLAYED = 1_761_400_000L;

    /** Set by the launcher before the screens open. */
    static boolean enabled;

    private static ComputerDetails laptop;
    private static final List<ComputerManagerListener> listeners = new CopyOnWriteArrayList<>();
    private static final Handler handler = new Handler(Looper.getMainLooper());

    private DemoAsleep() {
    }

    /** Called by the launcher: writes or removes the Laptop's saved library and puts the Laptop back to sleep. */
    static void prepare(Context context, boolean on) throws IOException {
        enabled = on;
        handler.removeCallbacksAndMessages(null);
        if (laptop != null) {
            laptop.state = ComputerDetails.State.OFFLINE;
            laptop.caps = on ? DemoFixtures.hostCapabilities() : com.limelight.nvstream.http.HostCapabilities.UNKNOWN;
        }
        File directory = new File(context.getFilesDir(), "console-library");
        File saved = new File(directory, LAPTOP_UUID.replaceAll("[^A-Za-z0-9_-]", "_") + ".json");
        if (!on) {
            saved.delete();
            return;
        }
        if (!directory.isDirectory() && !directory.mkdirs()) throw new IOException("Cannot create " + directory);
        String body = "{\"fetched_ms\":" + (System.currentTimeMillis() - 3_600_000L) + ",\"library\":" + library() + "}";
        try (FileOutputStream out = new FileOutputStream(saved)) {
            out.write(body.getBytes(StandardCharsets.UTF_8));
        }
    }

    /** The Laptop's library, as the native API's "ok" body: the shared games, then its own. */
    static String library() {
        StringBuilder games = new StringBuilder();
        for (int app : SHARED) {
            String store = DemoConsole.LIBRARY[app - 1][1];
            game(games, "laptop-" + app, app, DemoConsole.LIBRARY[app - 1][0], store, store + ":" + app, null, "demo-" + app);
        }
        for (int i = 0; i < LAPTOP_ONLY.length; i++) {
            // Lunar Base was played last of all, so a TV shows it first in Recently played.
            game(games, "laptop-" + (11 + i), 11 + i, LAPTOP_ONLY[i], null, null, i == 0 ? LAST_PLAYED : null,
                    "laptop-" + (11 + i));
        }
        return "{\"revision\":\"demo-laptop\",\"games\":[" + games + "]}";
    }

    private static void game(StringBuilder games, String installId, int appId, String title, String store, String gameId,
                             Long lastPlayed, String cover) {
        if (games.length() > 0) games.append(',');
        games.append("{\"install_id\":\"").append(installId).append("\",\"app_id\":").append(appId)
                .append(",\"title\":\"").append(title).append("\",\"game_id\":")
                .append(gameId == null ? "null" : "\"" + gameId + "\"")
                .append(store == null ? ",\"store\":null" : ",\"store\":{\"source\":\"" + store + "\",\"id\":\"" + appId + "\"}")
                .append(",\"installed\":true,\"running\":false,\"added\":true,")
                .append(lastPlayed == null ? "" : "\"last_played\":" + lastPlayed + ",")
                .append("\"artwork\":{\"cover\":\"/api/native/v1/cover/").append(cover)
                .append("\",\"version\":\"\"}}");
    }

    /** Lunar Base on the Laptop leads Continue playing on a phone, so the first screen shows a card that is away. */
    static void recordRecent(RecentsStore recents) {
        // Recents are kept between runs: a run without the Laptop drops the one an earlier run left.
        if (enabled) recents.record(new RecentsStore.Recent(LAPTOP_UUID, "Laptop", 11, LAPTOP_ONLY[0], 3_000L));
        else recents.forgetHost(LAPTOP_UUID);
    }

    /** Gives the service's Laptop the native API, so its saved library is read; the demo's other PCs stay as they are. */
    static List<ComputerDetails> apply(List<ComputerDetails> computers) {
        for (ComputerDetails computer : computers) {
            if (!LAPTOP_UUID.equals(computer.uuid)) continue;
            laptop = computer;
            if (enabled) computer.caps = DemoFixtures.hostCapabilities();
        }
        return computers;
    }

    static void listen(ComputerManagerListener listener) {
        listeners.add(listener);
    }

    static void unlisten(ComputerManagerListener listener) {
        listeners.remove(listener);
    }

    /** What the demo's screens do instead of sending a wake packet. */
    static void wake(ComputerDetails computer) {
        if (!enabled || !LAPTOP_UUID.equals(computer.uuid)) return;
        handler.postDelayed(() -> {
            if (laptop == null || !enabled) return;
            laptop.state = ComputerDetails.State.ONLINE;
            for (ComputerManagerListener listener : listeners) listener.notifyComputerUpdated(laptop);
        }, WAKE_MS);
    }
}
