package com.limelight.demo;

import android.content.Context;

import com.limelight.nativeapi.CachedAppListSource;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.FavouritesStore;
import com.limelight.nativeapi.FileLibraryCache;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeApiTransport;
import com.limelight.nativeapi.PreferencesFavouritesStore;
import com.limelight.nativeapi.PreferencesRecentsStore;
import com.limelight.nativeapi.RecentsStore;

import java.io.File;
import java.util.concurrent.Executors;

/**
 * The native API of the demo hosts, answered from fixtures so Home and the game detail run without a host:
 * the library is {@link #LIBRARY} and the games not added, Racing Game runs, and this device has host control unless a capture
 * asks for the classic Quit with {@code --ez control false}.
 */
final class DemoConsole {
    /** Set by the launcher before the screens open. */
    static boolean hostControl = true;
    /** Racing Game runs on the host; "--es session idle" shows every game as ready to play. */
    static boolean racingRuns = true;

    /** App ids starred on start: "--es stars 3,4" shows the Favourites row. */
    static int[] starred = new int[0];

    private DemoConsole() {
    }

    static ConsoleRepository repository(Context context) {
        PreferencesRecentsStore recents = new PreferencesRecentsStore(context);
        // Open World and Space Sim were played before; Racing Game is the running one.
        recents.record(new RecentsStore.Recent(DemoFixtures.PC_UUID, "Living-room PC", 3, "Open World", 1_000L));
        recents.record(new RecentsStore.Recent(DemoFixtures.PC_UUID, "Living-room PC", 4, "Space Sim", 2_000L));
        PreferencesFavouritesStore favourites = new PreferencesFavouritesStore(context);
        for (int app : starred) {
            favourites.set(new FavouritesStore.Favourite(DemoFixtures.PC_UUID, "Living-room PC", app,
                    titleOf(app)), true);
        }
        return new ConsoleRepository(new Transport(),
                computer -> new HostEndpoint(computer.uuid, DemoComputerManagerService.SETTINGS_ADDRESS, 47989, 47984,
                        "", "nightjar-demo", new byte[0], new byte[0]),
                new FileLibraryCache(new File(context.getFilesDir(), "console-library")), recents, favourites,
                new CachedAppListSource(context.getCacheDir()), Executors.newCachedThreadPool(), System::currentTimeMillis);
    }

    /** The native API client the demo stream menu's Radeon panel reads and writes through. */
    static NativeApiClient client() {
        return new NativeApiClient(new Transport(), new HostEndpoint(DemoFixtures.PC_UUID,
                DemoComputerManagerService.SETTINGS_ADDRESS, 47989, 47984, "", "nightjar-demo", new byte[0], new byte[0]));
    }

    /**
     * The demo library: the ten classic demo games first (same ids), then more, 36 added across Steam, Epic, GOG,
     * Xbox and no store, six of them not installed, five played lately; then four the host found but has not added.
     * Covers cycle through the ten demo images, which are portrait, landscape and square.
     */
    static final String[][] LIBRARY = {
            // title, store, installed, last played (unix seconds)
            {"Desktop", null, "1", null},
            {"Racing Game", "steam", "1", null},
            {"Open World", "steam", "1", "1761300000"},
            {"Space Sim", "epic", "1", "1761200000"},
            {"Puzzle Night", "gog", "1", null},
            {"Retro Arcade", "steam", "1", null},
            {"Strategy", "microsoft", "1", null},
            {"Music Studio", null, "1", null},
            {"Ocean Explorer", "epic", "1", null},
            {"Skybound", "steam", "1", null},
            {"Ashfall", "steam", "1", null},
            {"Brightwater", "steam", "1", "1761100000"},
            {"Cinder Road", "steam", "0", null},
            {"Deep Field", "steam", "1", null},
            {"Ember Keep", "steam", "1", null},
            {"Frostline", "steam", "1", null},
            {"Glass Harbor", "steam", "0", null},
            {"Hollow Peak", "steam", "1", null},
            {"Iron Tide", "steam", "1", null},
            {"Jade Circuit", "steam", "1", null},
            {"Kite Season", "steam", "1", null},
            {"Lantern Row", "steam", "0", null},
            {"Moss and Stone", "steam", "1", null},
            {"Night Courier", "steam", "1", null},
            {"Orbit Garden", "steam", "1", null},
            {"Paper Fleet", "steam", "1", null},
            {"Quiet Engine", "steam", "0", null},
            {"Red Mesa", "steam", "1", null},
            {"Salt Flats Rally", "steam", "1", null},
            {"Tidebreaker", "epic", "1", "1761000000"},
            {"Under Ice", "epic", "0", null},
            {"Vector Bloom", "epic", "1", null},
            {"Wild Meridian", "gog", "1", "1760900000"},
            {"Xeno Farm", "gog", "0", null},
            {"Yard Sale Hero", "microsoft", "1", null},
            {"Zero Gravity Golf", "microsoft", "1", null},
    };
    /** Found by the host in a store, not added: title, store. */
    static final String[][] NOT_ADDED = {
            {"Bastion Shore", "steam"}, {"Comet Tail", "steam"}, {"Driftwood", "epic"}, {"Echo Valley", "gog"},
    };
    /** "--ez add false": the host lists games not added but cannot add one on request. */
    static boolean hostAdds = true;
    /** The games of {@link #NOT_ADDED} added during this run, by index, with the app id each got. */
    private static final java.util.Map<Integer, Integer> added = new java.util.concurrent.ConcurrentHashMap<>();

    /** The title of an app id of the demo library. */
    static String titleOf(int appId) {
        if (appId >= 1 && appId <= LIBRARY.length) return LIBRARY[appId - 1][0];
        for (java.util.Map.Entry<Integer, Integer> entry : added.entrySet()) {
            if (entry.getValue() == appId) return NOT_ADDED[entry.getKey()][0];
        }
        return "";
    }

    private static String storeName(String source) {
        switch (source) {
            case "steam": return "Steam";
            case "epic": return "Epic Games";
            case "gog": return "GOG";
            default: return "Xbox";
        }
    }

    private static void appendStore(StringBuilder json, String source, int id) {
        if (source == null) {
            json.append("\"store\":null,");
        } else {
            json.append("\"store\":{\"source\":\"").append(source).append("\",\"id\":\"").append(id)
                    .append("\",\"name\":\"").append(storeName(source)).append("\"},");
        }
    }

    private static String library(String argument) {
        StringBuilder games = new StringBuilder();
        for (int i = 0; i < LIBRARY.length; i++) {
            String[] game = LIBRARY[i];
            if (i > 0) games.append(',');
            games.append("{\"install_id\":\"demo-").append(i + 1).append("\",\"app_id\":").append(i + 1)
                    .append(",\"title\":\"").append(game[0]).append("\",\"game_id\":")
                    .append(game[1] == null ? "null" : "\"" + game[1] + ":" + (i + 1) + "\"").append(',');
            appendStore(games, game[1], i + 1);
            games.append("\"installed\":").append("1".equals(game[2]))
                    .append(",\"running\":").append(racingRuns && i == 1)
                    .append(",\"added\":true,\"last_played\":").append(game[3] == null ? "null" : game[3])
                    .append(",\"artwork\":{\"cover\":\"/api/native/v1/cover/demo-").append(i + 1)
                    .append("\",\"version\":\"\"}}");
        }
        boolean unadded = argument != null && argument.contains("include=unadded");
        for (int i = 0; i < NOT_ADDED.length; i++) {
            String[] game = NOT_ADDED[i];
            Integer appId = added.get(i);
            if (appId == null && !unadded) continue;
            games.append(",{\"title\":\"").append(game[0]).append("\",\"game_id\":\"").append(game[1]).append(":n")
                    .append(i).append("\",");
            appendStore(games, game[1], 900 + i);
            if (appId == null) {
                games.append("\"installed\":true,\"running\":false,\"added\":false}");
            } else {
                games.append("\"install_id\":\"demo-n").append(i).append("\",\"app_id\":").append(appId)
                        .append(",\"installed\":true,\"running\":false,\"added\":true}");
            }
        }
        return "{\"ok\":{\"revision\":\"demo-" + added.size() + "\",\"games\":[" + games + "]}}";
    }

    /** library_add: the game is added with the next app id after the library's; the reply is that one entry. */
    private static String add(String argument) throws NativeApiException {
        for (int i = 0; i < NOT_ADDED.length; i++) {
            if (argument != null && argument.contains("\"" + NOT_ADDED[i][1] + ":n" + i + "\"")) {
                added.putIfAbsent(i, LIBRARY.length + 1 + i);
                return "{\"ok\":{\"install_id\":\"demo-n" + i + "\",\"app_id\":" + added.get(i) + ",\"title\":\""
                        + NOT_ADDED[i][0] + "\",\"game_id\":\"" + NOT_ADDED[i][1] + ":n" + i + "\",\"added\":true}}";
            }
        }
        throw new NativeApiException(NativeApiException.Kind.UNAVAILABLE, "the demo has no such game");
    }

    private static String session() {
        if (!racingRuns) {
            return "{\"ok\":{\"revision\":1,\"state\":\"idle\",\"owner\":\"none\",\"app\":null,"
                    + "\"game_running\":false,\"since_unix_ms\":1000,\"actions\":[]}}";
        }
        return "{\"ok\":{\"revision\":1,\"state\":\"streaming\",\"owner\":\"you\","
                + "\"app\":{\"install_id\":\"demo-2\",\"app_id\":2,\"title\":\"Racing Game\"},"
                + "\"game_running\":true,\"since_unix_ms\":1000,\"actions\":[]}}";
    }

    private static String capabilities() {
        return "{\"ok\":{\"api_version\":1,\"host\":{\"name\":\"Living-room PC\",\"version\":\"3.0.0\",\"unique_id\":\"U\"},"
                + "\"features\":[\"library\",\"session.close_game\",\"library.unadded\""
                + (hostAdds ? ",\"library.add\"" : "") + "],\"permissions\":{\"list\":true,\"view\":true,"
                + "\"launch\":true,\"host_control\":" + hostControl + "}}}";
    }

    private static final class Transport implements NativeApiTransport {
        @Override
        public String request(HostEndpoint host, String op, String argument) throws NativeApiException {
            switch (op) {
                case "library":
                    return library(argument);
                case "library_add":
                    return add(argument);
                case "session":
                    return session();
                case "capabilities":
                    return capabilities();
                case "radeon":
                    return DemoRadeon.read();
                case "radeon_write":
                    return DemoRadeon.write(argument);
                default:
                    throw new NativeApiException(NativeApiException.Kind.UNAVAILABLE, "the demo has no " + op);
            }
        }
    }
}
