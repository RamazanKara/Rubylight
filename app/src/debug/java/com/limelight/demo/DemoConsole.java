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
 * the library is the demo's ten games, Racing Game runs, and this device has host control unless a capture
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
                    DemoFixtures.TITLES[app - 1]), true);
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

    private static String library() {
        StringBuilder games = new StringBuilder();
        for (int i = 0; i < DemoFixtures.TITLES.length; i++) {
            if (i > 0) games.append(',');
            games.append("{\"install_id\":\"demo-").append(i + 1).append("\",\"app_id\":").append(i + 1)
                    .append(",\"title\":\"").append(DemoFixtures.TITLES[i]).append("\",\"game_id\":null,")
                    .append("\"store\":{\"source\":\"steam\",\"id\":\"1\",\"name\":\"Steam\"},\"installed\":true,")
                    .append("\"running\":").append(racingRuns && i == 1)
                    .append(",\"artwork\":{\"cover\":\"/api/native/v1/cover/demo-").append(i + 1)
                    .append("\",\"version\":\"\"}}");
        }
        return "{\"ok\":{\"revision\":\"demo\",\"games\":[" + games + "]}}";
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
                + "\"features\":[\"library\",\"session.close_game\"],\"permissions\":{\"list\":true,\"view\":true,"
                + "\"launch\":true,\"host_control\":" + hostControl + "}}}";
    }

    private static final class Transport implements NativeApiTransport {
        @Override
        public String request(HostEndpoint host, String op, String argument) throws NativeApiException {
            switch (op) {
                case "library":
                    return library();
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
