#include <array>
#include <cassert>
#include "../../main/jni/pyrowave-renderer/schedule.h"

static void limitsOutstandingGpuWork() {
    PyroWaveSchedule schedule;
    auto drawFinished = [](int) { return true; };
    const int first = schedule.reserve(drawFinished);
    const int second = schedule.reserve(drawFinished);
    assert(first >= 0 && second >= 0 && first != second);
    assert(schedule.inFlight() == 2);

    // Removing a submitted frame from the waiter's queue must not free its planes:
    // it remains in flight until the GPU completes, even while the waiter owns it.
    assert(schedule.isDecoding(first));
    assert(schedule.reserve(drawFinished) == -1);
    assert(schedule.complete(first) == -1);
    assert(schedule.inFlight() == 1);
    const int third = schedule.reserve(drawFinished);
    assert(third >= 0 && third != first && third != second);
    assert(schedule.complete(second) == first);
    assert(!schedule.isDecoding(second));
    assert(schedule.ready() == second);
    assert(schedule.complete(third) == second);
    assert(schedule.ready() == third && schedule.inFlight() == 0);
}

static void replacesOnlyCompletedFrames() {
    PyroWaveSchedule schedule;
    auto drawFinished = [](int) { return true; };
    const int first = schedule.reserve(drawFinished);
    const int second = schedule.reserve(drawFinished);
    assert(schedule.beginPresent() == -1);
    assert(schedule.complete(first) == -1);
    assert(schedule.beginPresent() == first);
    assert(schedule.ready() == -1 && schedule.drawing() == first);
    assert(schedule.replaceDrawingWithReady() == -1);
    assert(schedule.isDecoding(second));

    const int third = schedule.reserve(drawFinished);
    assert(third >= 0 && third != first && third != second);
    assert(schedule.complete(second) == -1);
    // Neither the pending decode nor the newer completed frame can overwrite the
    // frame the presenter owns while it waits for a swapchain image.
    assert(schedule.drawing() == first);
    assert(schedule.reserve(drawFinished) == -1);
    assert(schedule.replaceDrawingWithReady() == first);
    assert(schedule.drawing() == second && schedule.ready() == -1);
    assert(schedule.complete(third) == -1);
    assert(schedule.replaceDrawingWithReady() == second);
    assert(schedule.drawing() == third && schedule.ready() == -1);
    schedule.finishPresent();
    assert(schedule.drawing() == -1);
}

static void waitsForDrawFenceBeforeReuse() {
    PyroWaveSchedule schedule;
    std::array<bool, PyroWaveSchedule::SLOT_COUNT> fences{true, true, true};
    auto drawFinished = [&](int index) { return fences[index]; };
    const int first = schedule.reserve(drawFinished);
    assert(schedule.complete(first) == -1);
    assert(schedule.beginPresent() == first);
    fences[first] = false;
    schedule.finishPresent();

    const int second = schedule.reserve(drawFinished);
    const int third = schedule.reserve(drawFinished);
    assert(second >= 0 && third >= 0 && second != first && third != first);
    assert(schedule.complete(second) == -1);
    assert(schedule.beginPresent() == second);
    assert(schedule.complete(third) == -1);
    // Only the first slot is otherwise free, but its previous draw is still on GPU.
    assert(schedule.inFlight() == 0);
    assert(schedule.reserve(drawFinished) == -1);
    fences[first] = true;
    assert(schedule.reserve(drawFinished) == first);
}

static void warmupAndUnsubmittedFailureDoNotBecomeVisible() {
    PyroWaveSchedule schedule;
    auto drawFinished = [](int) { return true; };
    const int warmup = schedule.reserve(drawFinished);
    assert(schedule.complete(warmup, false) == -1);
    assert(schedule.inFlight() == 0 && schedule.ready() == -1);
    assert(schedule.beginPresent() == -1);
    const int unsubmitted = schedule.reserve(drawFinished);
    schedule.cancel(unsubmitted);
    assert(!schedule.isDecoding(unsubmitted));
    assert(schedule.inFlight() == 0 && schedule.ready() == -1);
    assert(schedule.reserve([](int) { return false; }) == -1);
    assert(schedule.inFlight() == 0);
}

int main() {
    limitsOutstandingGpuWork();
    replacesOnlyCompletedFrames();
    waitsForDrawFenceBeforeReuse();
    warmupAndUnsubmittedFailureDoNotBecomeVisible();
}
