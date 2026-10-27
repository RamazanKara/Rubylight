package com.limelight;

import java.util.ArrayDeque;

final class WindowFocusActionQueue {
    static final long TIMEOUT_MS = 250;

    private final ArrayDeque<Runnable> actions = new ArrayDeque<>();
    private long deadline;

    void add(Runnable action, long now) {
        if (actions.isEmpty()) {
            deadline = now + TIMEOUT_MS;
        }
        actions.add(action);
    }

    void runPending(boolean hasFocus, long now) {
        if (!hasFocus && now < deadline) {
            return;
        }
        while (!actions.isEmpty()) {
            actions.removeFirst().run();
        }
    }

    void clear() {
        actions.clear();
    }
}
