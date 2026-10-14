package com.limelight.nvstream.http;

public final class LaunchConfirmation {
    public enum State { WAITING, CONFIRMED, CANCELLED, TIMED_OUT }

    private static final String REPLACE = "An app is already running. Launch this app again within 60 seconds to confirm that you want to close it.";
    private static final String TERMINATE = "This will close the active stream but leave Remote Monitor and Remote Input connected. Launch Terminate again within 60 seconds to confirm this was intentional.";
    private final long deadline;
    private final boolean terminate;
    private State state = State.WAITING;

    public LaunchConfirmation(boolean terminate, long nowMs) {
        this.terminate = terminate;
        deadline = nowMs + 60_000;
    }

    public static boolean isRequired(NvApp app, HostHttpResponseException error) {
        // 410 also reports completed actions. Never confirm an unknown response by retrying it.
        return error.getErrorCode() == 410 &&
                (app.getControl() == NvApp.Control.TERMINATE ? TERMINATE.equals(error.getErrorMessage()) :
                        app.getControl() == NvApp.Control.NONE && REPLACE.equals(error.getErrorMessage()));
    }

    public static boolean isCompleted(NvApp app, HostHttpResponseException error) {
        if (error.getErrorCode() != 410) {
            return false;
        }
        // Rubylight's wording first (rust/host/src/nvhttp.rs), then Vibepollo's control tiles
        // (remote_session.cpp successful_control_completion), then Apollo's Terminate app, which
        // Vibepollo also still serves (nvhttp.cpp launch).
        switch (app.getControl()) {
            case DISCONNECT_MONITOR:
                return reports(error, "Remote monitor disconnected", "Remote Monitor disconnected successfully");
            case DISCONNECT_INPUT:
                return reports(error, "Remote input disconnected", "Remote Input disconnected successfully");
            case TERMINATE:
                return reports(error, "Application terminated",
                        "Active stream terminated. Remote Monitor and Remote Input remain connected", "App terminated");
            default:
                return false;
        }
    }

    /** Whether the host's message is one of these, apart from case and a closing period. */
    private static boolean reports(HostHttpResponseException error, String... messages) {
        String message = error.getErrorMessage();
        if (message == null) {
            return false;
        }
        message = message.trim();
        if (message.endsWith(".")) {
            message = message.substring(0, message.length() - 1);
        }
        for (String known : messages) {
            if (known.equalsIgnoreCase(message)) {
                return true;
            }
        }
        return false;
    }

    public static long nowMs() {
        return System.nanoTime() / 1_000_000;
    }

    public boolean isTerminate() {
        return terminate;
    }

    public synchronized State getState(long nowMs) {
        if (state == State.WAITING && nowMs >= deadline) {
            state = State.TIMED_OUT;
            notifyAll();
        }
        return state;
    }

    public synchronized void confirm(long nowMs) {
        if (getState(nowMs) == State.WAITING) {
            state = State.CONFIRMED;
            notifyAll();
        }
    }

    public synchronized void cancel() {
        if (state == State.WAITING) {
            state = State.CANCELLED;
            notifyAll();
        }
    }

    public synchronized State await() throws InterruptedException {
        while (getState(nowMs()) == State.WAITING) {
            wait(Math.max(1, deadline - nowMs()));
        }
        return state;
    }
}
