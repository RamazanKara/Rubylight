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

static void holdsTheShownPictureForRedrawsUntilANewerOneIsReady() {
    PyroWaveSchedule schedule;
    auto drawFinished = [](int) { return true; };
    assert(schedule.held() == -1 && schedule.beginRedraw() == -1);

    const int first = schedule.reserve(drawFinished);
    assert(schedule.complete(first) == -1);
    assert(schedule.beginPresent() == first);
    // Nothing is redrawn while a picture is being drawn
    assert(schedule.beginRedraw() == -1);
    schedule.finishPresent();
    schedule.hold(first);
    assert(schedule.held() == first);

    // Between pictures the held one is drawn again and decodes keep MAX_IN_FLIGHT around it
    assert(schedule.beginRedraw() == first);
    assert(schedule.drawing() == first && schedule.beginRedraw() == -1);
    const int second = schedule.reserve(drawFinished);
    const int third = schedule.reserve(drawFinished);
    assert(second >= 0 && third >= 0 && second != first && third != first);
    schedule.finishPresent();
    assert(schedule.held() == first && schedule.beginRedraw() == first);
    schedule.finishPresent();

    // A newer picture ready: no redraw, and the held slot is free for a decode
    assert(schedule.complete(second) == -1);
    assert(schedule.beginRedraw() == -1);
    assert(schedule.reserve(drawFinished) == first);
    assert(schedule.held() == -1);
    assert(schedule.beginPresent() == second);
    schedule.finishPresent();
    schedule.hold(second);

    // Presenting a newer picture lets the held one go
    assert(schedule.complete(third) == -1);
    assert(schedule.beginPresent() == third);
    assert(schedule.held() == -1);
    schedule.finishPresent();
    assert(schedule.beginRedraw() == -1);
}

static void protectsTheHeldPictureOnlyBetweenPictures() {
    PyroWaveSchedule schedule;
    std::array<bool, PyroWaveSchedule::SLOT_COUNT> fences{true, true, true};
    auto drawFinished = [&](int index) { return fences[index]; };
    const int first = schedule.reserve(drawFinished);
    assert(schedule.complete(first) == -1);
    assert(schedule.beginPresent() == first);
    schedule.finishPresent();
    schedule.hold(first);

    // The one other free slot still finishes an earlier draw: with nothing newer ready, a decode
    // waits rather than take the held picture
    const int second = schedule.reserve(drawFinished);
    const int other = 3 - first - second;
    fences[other] = false;
    assert(schedule.reserve(drawFinished) == -1);
    assert(schedule.held() == first);
    fences[other] = true;
    const int third = schedule.reserve(drawFinished);
    assert(third == other);

    // Once a newer picture is ready, the held one is free for a decode
    assert(schedule.complete(second) == -1);
    assert(schedule.reserve(drawFinished) == first);
    assert(schedule.held() == -1);
    assert(schedule.complete(third) == second);
    assert(schedule.complete(first) == third);
    assert(schedule.ready() == first);

    // A slot that is ready is never held
    PyroWaveSchedule fresh;
    const int ready = fresh.reserve(drawFinished);
    assert(fresh.complete(ready) == -1);
    fresh.hold(ready);
    assert(fresh.held() == -1);
}

int main() {
    holdsTheShownPictureForRedrawsUntilANewerOneIsReady();
    protectsTheHeldPictureOnlyBetweenPictures();
    limitsOutstandingGpuWork();
    replacesOnlyCompletedFrames();
    waitsForDrawFenceBeforeReuse();
    warmupAndUnsubmittedFailureDoNotBecomeVisible();
}
