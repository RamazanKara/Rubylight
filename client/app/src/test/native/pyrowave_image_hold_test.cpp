#include <cassert>
#include <cstdint>
#include <vector>
#include "../../main/jni/pyrowave-renderer/image_hold.h"

// A swapchain and its acquire fence as the presenter sees them: images acquired and not yet
// presented, and whether the fence has a signal on the way or is signalled and not reset.
struct FakeSwapchain {
    unsigned images = 4;
    std::vector<uint32_t> acquired;
    std::vector<PyroAcquire> results;  // The next acquires' outcomes; READY once empty.
    bool fenceInUse = false;           // Acquired with the fence, not yet reset.
    bool fenceSignals = true;          // The display frees the image (the wait succeeds).
    unsigned fenceMisuse = 0;          // Acquires with a fence still in use.
    unsigned waits = 0, resets = 0;

    PyroAcquire acquire(uint32_t *index) {
        PyroAcquire result = PyroAcquire::READY;
        if (!results.empty()) {
            result = results.front();
            results.erase(results.begin());
        }
        if (result != PyroAcquire::READY) return result;
        if (acquired.size() == images) return PyroAcquire::NOT_READY;
        if (fenceInUse) fenceMisuse++;
        for (uint32_t i = 0; i < images; i++) {
            bool taken = false;
            for (uint32_t a : acquired) taken |= a == i;
            if (!taken) {
                acquired.push_back(i);
                fenceInUse = true;
                *index = i;
                return PyroAcquire::READY;
            }
        }
        return PyroAcquire::NOT_READY;
    }

    bool settle() {
        waits++;
        if (!fenceSignals) return false;
        resets++;
        fenceInUse = false;
        return true;
    }

    void present(uint32_t index) {
        for (auto it = acquired.begin(); it != acquired.end(); ++it) {
            if (*it == index) {
                acquired.erase(it);
                return;
            }
        }
        assert(!"presented an image that was not acquired");
    }

    void recreate() {
        acquired.clear();
    }
};

static PyroAcquire take(PyroWaveImageHold &hold, FakeSwapchain &swapchain, uint32_t *index) {
    return hold.take(index, [&](uint32_t *i) { return swapchain.acquire(i); }, [&] { return swapchain.settle(); });
}

static void preacquire(PyroWaveImageHold &hold, FakeSwapchain &swapchain) {
    hold.preacquire([&](uint32_t *i) { return swapchain.acquire(i); });
}

static bool release(PyroWaveImageHold &hold, FakeSwapchain &swapchain) {
    return hold.release([&] { return swapchain.settle(); });
}

static void withoutPreacquireEachPresentAcquiresAndSettles() {
    PyroWaveImageHold hold;
    FakeSwapchain swapchain;
    for (int frame = 0; frame < 100; frame++) {
        uint32_t index = 99;
        assert(take(hold, swapchain, &index) == PyroAcquire::READY);
        swapchain.present(index);
    }
    assert(swapchain.acquired.empty() && swapchain.fenceMisuse == 0);
    assert(swapchain.waits == 100 && swapchain.resets == 100);
}

static void preacquiredImageGoesToTheNextPresent() {
    PyroWaveImageHold hold;
    FakeSwapchain swapchain;
    for (int frame = 0; frame < 100; frame++) {
        uint32_t index = 99;
        assert(take(hold, swapchain, &index) == PyroAcquire::READY);
        swapchain.present(index);
        preacquire(hold, swapchain);
        assert(hold.held() && hold.fencePending());
    }
    // One image ahead, never more, and every fence settled before its next acquire.
    assert(swapchain.acquired.size() == 1 && swapchain.fenceMisuse == 0);
    // A second pre-acquire while one is held takes nothing.
    preacquire(hold, swapchain);
    assert(swapchain.acquired.size() == 1);
}

// The reverted version's leak: the fence wait after an acquire failed, the image was dropped and
// the fence left in use. Here the image stays held and the fence is settled before any reuse.
static void failedFenceWaitKeepsTheImageAndTheFence() {
    PyroWaveImageHold hold;
    FakeSwapchain swapchain;
    preacquire(hold, swapchain);
    swapchain.fenceSignals = false;
    uint32_t index = 99;
    assert(take(hold, swapchain, &index) == PyroAcquire::FAILED);
    assert(hold.held() && hold.fencePending());
    assert(swapchain.acquired.size() == 1);
    // A pre-acquire now would reuse the fence still in use: it takes nothing.
    preacquire(hold, swapchain);
    assert(swapchain.acquired.size() == 1 && swapchain.fenceMisuse == 0);
    // Once the fence signals the same image goes to the present; nothing leaked.
    swapchain.fenceSignals = true;
    assert(take(hold, swapchain, &index) == PyroAcquire::READY);
    swapchain.present(index);
    assert(swapchain.acquired.empty() && !hold.held() && !hold.fencePending());

    // The same for a fresh acquire whose wait fails.
    swapchain.fenceSignals = false;
    assert(take(hold, swapchain, &index) == PyroAcquire::FAILED);
    assert(hold.held() && swapchain.acquired.size() == 1);
    swapchain.fenceSignals = true;
    assert(take(hold, swapchain, &index) == PyroAcquire::READY);
    swapchain.present(index);
    assert(swapchain.acquired.empty() && swapchain.fenceMisuse == 0);
}

static void failuresBeforeAnAcquireHoldNothing() {
    PyroWaveImageHold hold;
    FakeSwapchain swapchain;
    for (PyroAcquire failure : {PyroAcquire::NOT_READY, PyroAcquire::OUT_OF_DATE, PyroAcquire::FAILED}) {
        swapchain.results = {failure};
        preacquire(hold, swapchain);
        assert(!hold.held() && !hold.fencePending());
        swapchain.results = {failure};
        uint32_t index = 99;
        assert(take(hold, swapchain, &index) == failure);
        assert(!hold.held() && !hold.fencePending());
    }
    assert(swapchain.acquired.empty() && swapchain.waits == 0);
    // Every image held already: the pre-acquire takes none.
    swapchain.acquired = {0, 1, 2, 3};
    preacquire(hold, swapchain);
    assert(!hold.held());
}

// Surface change, HDR switch or stop: the held image goes with its swapchain, and the fence is
// settled so the new swapchain's first acquire can use it.
static void swapchainChangeReleasesTheHeldImageAndSettlesTheFence() {
    PyroWaveImageHold hold;
    FakeSwapchain swapchain;
    preacquire(hold, swapchain);
    assert(release(hold, swapchain));
    swapchain.recreate();
    assert(!hold.held() && !hold.fencePending() && !swapchain.fenceInUse);
    uint32_t index = 99;
    assert(take(hold, swapchain, &index) == PyroAcquire::READY);
    swapchain.present(index);
    assert(swapchain.fenceMisuse == 0);

    // A fence that does not signal by the release stays pending, and the next take settles it
    // before acquiring with it.
    preacquire(hold, swapchain);
    swapchain.fenceSignals = false;
    assert(!release(hold, swapchain));
    swapchain.recreate();
    assert(!hold.held() && hold.fencePending());
    assert(take(hold, swapchain, &index) == PyroAcquire::FAILED);
    assert(swapchain.acquired.empty());
    swapchain.fenceSignals = true;
    assert(take(hold, swapchain, &index) == PyroAcquire::READY);
    swapchain.present(index);
    assert(swapchain.fenceMisuse == 0 && !hold.fencePending());

    // Released with nothing held: nothing to wait for.
    const unsigned waits = swapchain.waits;
    assert(release(hold, swapchain));
    assert(swapchain.waits == waits);
}

// A soak of presents with pre-acquire and a fence that sometimes fails: the swapchain never runs
// out of images (the stall the reverted version plausibly caused).
static void soakNeverRunsOutOfImages() {
    PyroWaveImageHold hold;
    FakeSwapchain swapchain;
    unsigned presented = 0;
    for (int frame = 0; frame < 36000; frame++) {
        swapchain.fenceSignals = frame % 97 != 0;
        if (frame % 1201 == 0) {
            // The swapchain goes whether or not its fence signalled in time.
            release(hold, swapchain);
            swapchain.recreate();
        }
        uint32_t index = 99;
        if (take(hold, swapchain, &index) == PyroAcquire::READY) {
            swapchain.present(index);
            presented++;
            preacquire(hold, swapchain);
        }
        assert(swapchain.acquired.size() <= 1);
    }
    assert(presented > 35000 && swapchain.fenceMisuse == 0);
}

int main() {
    withoutPreacquireEachPresentAcquiresAndSettles();
    preacquiredImageGoesToTheNextPresent();
    failedFenceWaitKeepsTheImageAndTheFence();
    failuresBeforeAnAcquireHoldNothing();
    swapchainChangeReleasesTheHeldImageAndSettlesTheFence();
    soakNeverRunsOutOfImages();
    return 0;
}
