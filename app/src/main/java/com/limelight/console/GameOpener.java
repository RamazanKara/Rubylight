package com.limelight.console;

import android.app.Activity;
import android.content.Intent;
import android.widget.Toast;

import com.limelight.R;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeLibrary;
import com.limelight.nvstream.http.ComputerDetails;

import java.util.Set;
import java.util.concurrent.Executor;

/**
 * Opens a card from Home or All games. A game in the library opens its detail screen. A game the host found but
 * has not added is added first when the host can do that on request; otherwise the card says to add it on the PC.
 */
final class GameOpener {
    private GameOpener() {
    }

    /** True while the host is adding this game for a press on this screen; another press then does nothing. */
    static boolean adding(Screen screen, String hostUuid, String gameId) {
        return screen.adding().contains(hostUuid + "|" + gameId);
    }

    /** What opening needs from the screen: its data, the host behind a card and what to do once a game was added. */
    interface Screen {
        Activity activity();

        ConsoleRepository repository();

        ComputerDetails host(String uuid);

        Class<? extends GameDetailActivity> detailActivity();

        /** The library changed on the host: load it again. */
        void onAdded();

        /**
         * The games this screen is adding now, by host and game id; touched on the UI thread only. The screen owns
         * it, so an add its worker drops on destroy never blocks a later screen's press.
         */
        Set<String> adding();
    }

    static void open(Screen screen, Executor worker, HomeModel.GameCard card) {
        if (!card.notAdded()) {
            showDetail(screen, card.hostUuid, card.appId, card.title, card.hostName);
            return;
        }
        Activity activity = screen.activity();
        ConsoleRepository repo = screen.repository();
        ComputerDetails computer = screen.host(card.hostUuid);
        if (!card.addable || repo == null || computer == null || card.source.gameId == null
                || !computer.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
            Toast.makeText(activity, R.string.console_add_on_pc, Toast.LENGTH_SHORT).show();
            return;
        }
        String gameId = card.source.gameId;
        String addKey = card.hostUuid + "|" + gameId;
        Set<String> adding = screen.adding();
        if (!adding.add(addKey)) {
            return;
        }
        Toast.makeText(activity, activity.getString(R.string.console_adding, card.title), Toast.LENGTH_SHORT).show();
        Runnable add = () -> {
            NativeLibrary.Game added;
            try {
                added = repo.addToLibrary(computer, gameId);
            } catch (NativeApiException e) {
                com.limelight.LimeLog.warning("Could not add " + gameId + ": " + e);
                added = null;
            }
            final NativeLibrary.Game game = added;
            activity.runOnUiThread(() -> {
                adding.remove(addKey);
                if (activity.isFinishing() || activity.isDestroyed()) {
                    return;
                }
                screen.onAdded();
                if (game == null || game.appId <= 0) {
                    Toast.makeText(activity, activity.getString(R.string.console_add_failed, card.title),
                            Toast.LENGTH_LONG).show();
                    return;
                }
                showDetail(screen, card.hostUuid, game.appId, game.title.isEmpty() ? card.title : game.title,
                        card.hostName);
            });
        };
        try {
            worker.execute(add);
        } catch (java.util.concurrent.RejectedExecutionException e) {
            adding.remove(addKey);
        }
    }

    private static void showDetail(Screen screen, String hostUuid, int appId, String title, String hostName) {
        Activity activity = screen.activity();
        activity.startActivity(new Intent(activity, screen.detailActivity())
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, hostUuid)
                .putExtra(GameDetailActivity.EXTRA_APP_ID, appId)
                .putExtra(GameDetailActivity.EXTRA_TITLE, title)
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, hostName));
    }
}
