package com.limelight.console;

import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nativeapi.NativeSession;

/**
 * What a game's detail screen offers, from the host's last library and session: play or resume, and how
 * to end a game that runs. Close game needs the host_control permission of this device on the host;
 * without it the screen offers the classic Quit, which works for any paired device.
 */
public final class GameDetailModel {
    public enum State { IDLE, RUNNING, SUSPENDED }

    public enum EndAction {
        NONE,
        /** The native API's Close game (needs host control). */
        CLOSE_GAME,
        /** The classic Quit through /cancel. */
        QUIT
    }

    public final String hostUuid;
    public final String hostName;
    public final String title;
    public final int appId;
    /** How the host lists the game; null when its library does not (any more). */
    public final ConsoleSnapshot.Source source;
    public final State state;
    public final boolean hostOnline;
    public final EndAction endAction;
    /** The title of another game that runs on the host and would be replaced by playing this one; else null. */
    public final String otherRunningTitle;
    /** The game behind {@link #otherRunningTitle}, to resume it instead; meaningful only when that title is set. */
    public final int otherRunningAppId;
    /** How the host lists that game; null when its library does not. */
    public final ConsoleSnapshot.Source otherRunningSource;

    private GameDetailModel(String hostUuid, String hostName, String title, int appId, ConsoleSnapshot.Source source,
                            State state, boolean hostOnline, EndAction endAction, String otherRunningTitle,
                            int otherRunningAppId, ConsoleSnapshot.Source otherRunningSource) {
        this.hostUuid = hostUuid;
        this.hostName = hostName;
        this.title = title;
        this.appId = appId;
        this.source = source;
        this.state = state;
        this.hostOnline = hostOnline;
        this.endAction = endAction;
        this.otherRunningTitle = otherRunningTitle;
        this.otherRunningAppId = otherRunningAppId;
        this.otherRunningSource = otherRunningSource;
    }

    /** Resume when the game runs or is suspended on this host, Play otherwise. */
    public boolean resumes() {
        return state != State.IDLE;
    }

    public boolean canStart() {
        return hostOnline;
    }

    /**
     * @param fallbackTitle shown when the host's library does not list the game
     * @param hostOnline    whether the host answers now
     * @param nativeHost    whether the host has the native API
     * @param hostControl   whether this device has host control there; unknown (not read) counts as no
     */
    public static GameDetailModel of(ConsoleSnapshot snapshot, String hostUuid, int appId, String fallbackTitle,
                                     String fallbackHostName, boolean hostOnline, boolean nativeHost,
                                     boolean hostControl) {
        ConsoleSnapshot.Host host = snapshot.host(hostUuid);
        ConsoleSnapshot.Source source = null;
        String otherRunning = null;
        int otherRunningId = 0;
        ConsoleSnapshot.Source otherRunningSource = null;
        if (host != null) {
            for (ConsoleSnapshot.Source candidate : host.sources) {
                if (candidate.appId == appId) {
                    source = candidate;
                } else if (candidate.running && otherRunning == null) {
                    otherRunning = candidate.title;
                    otherRunningId = candidate.appId;
                    otherRunningSource = candidate;
                }
            }
        }
        State state = State.IDLE;
        NativeSession session = host == null ? null : host.session;
        if (session != null && session.isActive()) {
            if (session.app.appId == appId) {
                state = session.state == NativeSession.State.SUSPENDED ? State.SUSPENDED : State.RUNNING;
                otherRunning = null;
            } else {
                otherRunning = session.app.title;
                otherRunningId = session.app.appId;
                otherRunningSource = null;
                for (ConsoleSnapshot.Source candidate : host.sources) {
                    if (candidate.appId == otherRunningId) {
                        otherRunningSource = candidate;
                    }
                }
            }
        } else if (source != null && source.running) {
            state = State.RUNNING;
        }
        if (!hostOnline) {
            // A saved library's running flag is old news; nothing can be resumed or closed on a host that is away.
            state = State.IDLE;
            otherRunning = null;
        }
        if (state != State.IDLE) {
            otherRunning = null;
        }
        EndAction end = state == State.IDLE ? EndAction.NONE
                : nativeHost && hostControl ? EndAction.CLOSE_GAME : EndAction.QUIT;
        return new GameDetailModel(hostUuid, host != null ? host.name : fallbackHostName,
                source != null ? source.title : fallbackTitle, appId, source, state, hostOnline, end, otherRunning,
                otherRunning == null ? 0 : otherRunningId, otherRunning == null ? null : otherRunningSource);
    }
}
