package com.limelight;

import org.junit.Test;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

import static org.junit.Assert.*;

public class WindowFocusActionQueueTest {
    @Test
    public void menuActionsWaitForFocusAndRunInOrder() {
        WindowFocusActionQueue queue = new WindowFocusActionQueue();
        List<String> events = new ArrayList<>();
        queue.add(() -> events.add("restore input"), 1000);
        queue.add(() -> events.add("keyboard"), 1000);

        queue.runPending(false, 1000);
        queue.runPending(false, 1100);
        assertTrue(events.isEmpty());

        queue.runPending(true, 1100);
        assertEquals(Arrays.asList("restore input", "keyboard"), events);
        queue.runPending(true, 1200);
        queue.runPending(false, 1000 + WindowFocusActionQueue.TIMEOUT_MS);
        assertEquals(2, events.size());
    }

    @Test
    public void missingFocusCallbackFallsBackOnceAtTheDeadline() {
        WindowFocusActionQueue queue = new WindowFocusActionQueue();
        List<String> events = new ArrayList<>();
        queue.add(() -> events.add("keyboard"), 1000);

        queue.runPending(false, 1000 + WindowFocusActionQueue.TIMEOUT_MS - 1);
        assertTrue(events.isEmpty());
        queue.runPending(false, 1000 + WindowFocusActionQueue.TIMEOUT_MS);
        queue.runPending(true, 2000);
        assertEquals(Arrays.asList("keyboard"), events);
    }

    @Test
    public void pausedOrStoppedActivityDiscardsPendingActions() {
        WindowFocusActionQueue queue = new WindowFocusActionQueue();
        queue.add(() -> fail("Cancelled keyboard action ran"), 1000);
        queue.clear();
        queue.runPending(true, 1100);
        queue.runPending(false, 2000);
    }

    @Test
    public void oldTimeoutDoesNotRunANewActionBeforeItsDeadline() {
        WindowFocusActionQueue queue = new WindowFocusActionQueue();
        List<String> events = new ArrayList<>();
        queue.add(() -> events.add("first"), 1000);
        queue.runPending(true, 1100);
        queue.add(() -> events.add("second"), 1200);

        queue.runPending(false, 1000 + WindowFocusActionQueue.TIMEOUT_MS);
        assertEquals(Arrays.asList("first"), events);
        queue.runPending(false, 1200 + WindowFocusActionQueue.TIMEOUT_MS);
        assertEquals(Arrays.asList("first", "second"), events);
    }

    @Test
    public void cancellingFromAnActionStopsTheRemainingQueue() {
        WindowFocusActionQueue queue = new WindowFocusActionQueue();
        queue.add(queue::clear, 1000);
        queue.add(() -> fail("Action ran after disconnect"), 1000);
        queue.runPending(true, 1100);
    }
}
