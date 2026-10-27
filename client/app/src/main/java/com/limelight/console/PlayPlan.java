package com.limelight.console;

import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nvstream.http.NvApp;

/**
 * What Play does on a game's detail screen, decided from plain facts so it can be tested without a screen:
 * start at once, or ask first because another game runs on the PC and starting this one would close it.
 * A host that confirms replacement asks the player itself, and a device without launch permission cannot
 * close anything, so neither is asked here.
 */
final class PlayPlan {
    enum Kind { START, ASK }

    /** What the host runs now, compared with the game a button was shown for. */
    enum Check { SAME, GONE, CHANGED }

    final Kind kind;
    /** Replacing closes the running game through the native API first (native host with host control). */
    final boolean closeNatively;
    /** The running game the prompt names; null for START. */
    final String runningTitle;
    final int runningAppId;
    final ConsoleSnapshot.Source runningSource;
    /** False when the running game cannot be told apart to resume it: the prompt then offers only Cancel. */
    final boolean canResume;

    private PlayPlan(Kind kind, boolean closeNatively, String runningTitle, int runningAppId,
                     ConsoleSnapshot.Source runningSource, boolean canResume) {
        this.kind = kind;
        this.closeNatively = closeNatively;
        this.runningTitle = runningTitle;
        this.runningAppId = runningAppId;
        this.runningSource = runningSource;
        this.canResume = canResume;
    }

    static PlayPlan of(GameDetailModel model, boolean hostConfirmsReplacement, boolean launchPermission,
                       boolean nativeHost, boolean hostControl) {
        boolean ask = model.otherRunningTitle != null && !hostConfirmsReplacement && launchPermission;
        if (!ask) {
            return new PlayPlan(Kind.START, false, null, 0, null, false);
        }
        boolean identified = model.otherRunningAppId != 0
                || (model.otherRunningSource != null && !model.otherRunningSource.appUuid.isEmpty());
        return new PlayPlan(Kind.ASK, nativeHost && hostControl, model.otherRunningTitle, model.otherRunningAppId,
                model.otherRunningSource, identified);
    }

    /** The running game as the stream needs it. */
    NvApp runningApp() {
        return CoverLoaders.appOf(runningTitle, runningAppId, runningSource);
    }

    /** Whether {@code expected} still is what the host runs: it may have ended or been replaced meanwhile. */
    static Check recheck(NvApp expected, int runningGameId, String runningGameUuid) {
        boolean nothing = runningGameId == 0 && (runningGameUuid == null || runningGameUuid.isEmpty());
        if (nothing) {
            return Check.GONE;
        }
        return expected.matchesRunningApp(runningGameId, runningGameUuid) ? Check.SAME : Check.CHANGED;
    }
}
