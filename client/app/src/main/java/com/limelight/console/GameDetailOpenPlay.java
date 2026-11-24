package com.limelight.console;

import android.content.Context;
import android.net.ConnectivityManager;
import android.os.Handler;
import android.os.SystemClock;
import android.view.View;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.pin.PinGate;
import com.limelight.ui.NavigationDialogBuilder;

/**
 * Automatic "When Nightjar opens" startup flow for {@link GameDetailActivity}: waiting for the
 * network, counting down with a cancelable dialog, arming the retry guard, and waking or launching
 * once a live page read arrives.
 */
final class GameDetailOpenPlay {
    /** How long an automatic start waits for the page to load; a wake that follows has its own limit. */
    private static final long AUTO_LOAD_LIMIT_MS = 45_000;
    /** How often an automatic start asks again for what the PC has not said yet. */
    private static final long AUTO_RETRY_MS = 2_000;
    /** As on the desktop: how long a start waits for the network, a TV launcher's at boot being ahead of its Wi-Fi. */
    private static final long NETWORK_LIMIT_MS = 60_000;
    /** After the network came back: longer than a poll, so the countdown never names a PC not yet heard from. */
    private static final long NETWORK_SETTLE_MS = 6_000;
    private static final long NETWORK_CHECK_MS = 500;

    private final GameDetailActivity activity;
    private final Handler handler;

    private AlertDialog countdown;
    private long networkLimit;
    /** When the wait for the network ends, now that it is back; 0 until then. */
    private long networkSettled;
    /** {@link #autoLoadLimit} is on its way. */
    private boolean autoLimitPosted;
    /** The automatic start has acted on a live page. */
    boolean autoActed;

    private final Runnable countdownTick = this::tickCountdown;
    private final Runnable networkTick = this::tickNetwork;
    private final Runnable autoLoadLimit = () -> {
        autoLimitPosted = false;
        autoLoadFailed();
    };

    GameDetailOpenPlay(GameDetailActivity activity, Handler handler) {
        this.activity = activity;
        this.handler = handler;
    }

    void onResume() {
        if (activity.auto == GameDetailActivity.Auto.WAITING && !PinGate.needsPinForApp(activity)) {
            if (hasNetwork()) {
                startCountdown();
            } else {
                waitForNetwork();
            }
        }
    }

    void onPause() {
        if (activity.auto == GameDetailActivity.Auto.COUNTING || activity.auto == GameDetailActivity.Auto.NETWORK) {
            stopCountdown();
        } else if (activity.auto == GameDetailActivity.Auto.GOING) {
            // Leaving the page during the start, the wake's wait for one, is the player's choice.
            endAuto();
            OpenPlay.disarm(activity);
        }
    }

    private boolean hasNetwork() {
        ConnectivityManager connectivity = (ConnectivityManager) activity.getApplicationContext().getSystemService(Context.CONNECTIVITY_SERVICE);
        return connectivity == null || connectivity.getActiveNetworkInfo() != null;
    }

    /** No network yet: a wait the player can still say no to, with the same ways out as the countdown. */
    private void waitForNetwork() {
        activity.auto = GameDetailActivity.Auto.NETWORK;
        networkLimit = SystemClock.elapsedRealtime() + NETWORK_LIMIT_MS;
        networkSettled = 0;
        countdown = new NavigationDialogBuilder(activity)
                .setTitle(activity.titleExtra)
                .setMessage(R.string.open_play_waiting_network)
                .setNegativeButton(android.R.string.cancel, (dialog, which) -> cancelCountdown())
                .setOnCancelListener(dialog -> cancelCountdown())
                .show();
        focusCancel();
        handler.postDelayed(networkTick, NETWORK_CHECK_MS);
    }

    /** Also after a touch, or the first press of a pad would only leave touch mode and the game would start. */
    private void focusCancel() {
        View cancel = countdown.getButton(AlertDialog.BUTTON_NEGATIVE);
        cancel.setFocusableInTouchMode(true);
        cancel.requestFocus();
    }

    private void tickNetwork() {
        if (activity.auto != GameDetailActivity.Auto.NETWORK) return;
        long now = SystemClock.elapsedRealtime();
        if (networkSettled == 0 && hasNetwork()) {
            networkSettled = now + NETWORK_SETTLE_MS;
            // What the page read without a network says nothing.
            activity.modelLive = false;
            activity.loadedSignature = null;
            activity.scheduleReload();
        }
        if (networkSettled == 0 && now >= networkLimit) {
            countdown.dismiss();
            countdown = null;
            // No guard for a start that cannot happen.
            activity.auto = GameDetailActivity.Auto.GOING;
            autoFailed(WakeFeedback.said(activity, activity.hostNameExtra, activity.getString(R.string.open_play_no_network)));
        } else if (networkSettled != 0 && now >= networkSettled) {
            countdown.dismiss();
            countdown = null;
            startCountdown();
        } else {
            handler.postDelayed(networkTick, NETWORK_CHECK_MS);
        }
    }

    /** The 3 seconds in which the player can still say no, naming the game and the PC. */
    private void startCountdown() {
        activity.auto = GameDetailActivity.Auto.COUNTING;
        activity.countdownEnd = SystemClock.elapsedRealtime() + OpenPlayModel.timing()[0];
        countdown = new NavigationDialogBuilder(activity)
                .setTitle(activity.titleExtra)
                .setMessage(countdownText())
                .setNegativeButton(android.R.string.cancel, (dialog, which) -> cancelCountdown())
                .setOnCancelListener(dialog -> cancelCountdown())
                .show();
        focusCancel();
        handler.postDelayed(countdownTick, 100);
    }

    private String countdownText() {
        long left = Math.max(0, activity.countdownEnd - SystemClock.elapsedRealtime());
        return activity.getString(R.string.open_play_countdown, activity.hostNameExtra, (int) ((left + 999) / 1000));
    }

    private void tickCountdown() {
        if (activity.auto != GameDetailActivity.Auto.COUNTING) return;
        long now = SystemClock.elapsedRealtime();
        if (now < activity.countdownEnd) {
            countdown.setMessage(countdownText());
            // Each tick comes after the last one ran, so a late one moves the rest on: the last waits only for the end.
            handler.postDelayed(countdownTick, Math.min(100, activity.countdownEnd - now));
            return;
        }
        countdown.dismiss();
        countdown = null;
        activity.auto = GameDetailActivity.Auto.GOING;
        // Armed before anything can go wrong: only a good ending clears it.
        OpenPlay.arm(activity);
        startAutoLimit();
        // Another device may have started in these seconds, so only a read from after them decides.
        activity.forceLiveReload();
    }

    /** The countdown is over without a start; the guard stays as it was. */
    private void stopCountdown() {
        handler.removeCallbacks(countdownTick);
        handler.removeCallbacks(networkTick);
        if (countdown != null) {
            countdown.dismiss();
            countdown = null;
        }
        activity.auto = GameDetailActivity.Auto.OFF;
    }

    /** Cancel, B, Back or Escape: Home, or the list of PCs where that is the first screen. */
    private void cancelCountdown() {
        stopCountdown();
        activity.finish();
    }

    /** The page is loaded and the countdown over: wake the PC, or start the game. */
    void actOnAuto() {
        ComputerDetails computer = activity.currentHost();
        if (computer != null && computer.state == ComputerDetails.State.UNKNOWN) {
            // Not heard from yet is not offline: the next poll's news comes back here, within the limit.
            activity.modelLive = false;
            return;
        }
        if (computer != null && computer.state == ComputerDetails.State.ONLINE && !activity.model.hostOnline) {
            // It came back after this page was read, which is out of date: read it again, within the limit.
            activity.modelLive = false;
            activity.scheduleReload();
            return;
        }
        autoActed = true;
        if (activity.wakeable(activity.model)) {
            // The wake has its own limit.
            stopAutoLimit();
            activity.wakeAndPlay();
        } else {
            startAuto(activity.model);
        }
    }

    void startAutoLimit() {
        if (autoLimitPosted) return;
        autoLimitPosted = true;
        handler.postDelayed(autoLoadLimit, AUTO_LOAD_LIMIT_MS);
    }

    void stopAutoLimit() {
        autoLimitPosted = false;
        handler.removeCallbacks(autoLoadLimit);
    }

    /** The PC answers, or cannot be woken: the game starts, or the player learns why not. */
    void startAuto(GameDetailModel m) {
        if (!m.canStart()) {
            autoFailed(WakeFeedback.said(activity, m.hostName, activity.whyNot(m)));
        } else if (m.playingElsewhere) {
            // Nothing went wrong, so the next start is not skipped for it. Never moved automatically.
            OpenPlay.disarm(activity);
            autoFailed(WakeFeedback.said(activity, m.hostName, activity.getString(R.string.open_play_other_device, m.hostName)));
        } else if (m.libraryRead && (m.source == null ? !activity.classicListCache
                : !activity.stableId.isEmpty() && !OpenPlay.sameGame(m.source, activity.stableId))) {
            // Its id is stale, or another game's by now; the PC would be asked for a game it no longer has.
            autoFailed(WakeFeedback.said(activity, m.hostName, activity.getString(R.string.open_play_not_listed, m.hostName)));
        } else if (m.sessionUnread && (m.source != null || activity.classicListCache)) {
            // Free is not what a failed read says: ask again, within the limit. (With nothing to start, Play says so.)
            autoActed = false;
            activity.modelLive = false;
            startAutoLimit();
            activity.scheduleReloadDelayed(AUTO_RETRY_MS);
        } else {
            activity.onPlay();
        }
    }

    /** The page never loaded in time: either the PC is away, or it answers without telling what it has. */
    private void autoLoadFailed() {
        ComputerDetails computer = activity.currentHost();
        boolean online = computer != null && computer.state == ComputerDetails.State.ONLINE;
        autoFailed(WakeFeedback.said(activity, activity.hostNameExtra, !online ? activity.getString(R.string.error_pc_offline)
                : activity.model != null && modelSaidNoSession() ? activity.getString(R.string.open_play_no_answer, activity.hostNameExtra)
                : activity.getString(R.string.open_play_no_list, activity.hostNameExtra)));
    }

    /** The list came, and it was the session that kept the start waiting. */
    private boolean modelSaidNoSession() {
        return activity.model.sessionUnread && (activity.model.source != null || activity.classicListCache);
    }

    /** The automatic part is over; what follows is the page's usual behaviour. */
    void endAuto() {
        activity.auto = GameDetailActivity.Auto.OFF;
        stopAutoLimit();
        activity.followTurnSoon();
    }

    /** An automatic start failed and says so: the player lands on Home, and the guard stays armed. */
    void autoFailed(AlertDialog said) {
        if (activity.auto != GameDetailActivity.Auto.GOING) return;
        endAuto();
        if (said == null) {
            activity.finish();
        } else {
            said.setOnDismissListener(dialog -> activity.finish());
        }
    }

    /** The usual feedback, ending an automatic start: it does not offer to try again, it goes to Home. */
    void wakeFailed(int message, Runnable retry) {
        autoFailed(WakeFeedback.failed(activity, activity.model.hostName, message,
                activity.auto == GameDetailActivity.Auto.GOING ? null : retry));
    }
}
