#pragma once

#include <cstdint>

// One vkAcquireNextImageKHR outcome, free of Vulkan types for the host tests. READY includes
// VK_SUBOPTIMAL_KHR: an image was acquired either way.
enum class PyroAcquire { READY, NOT_READY, OUT_OF_DATE, FAILED };

// The swapchain image between its acquire and its present, and the renderer's one acquire fence.
// With pre-acquire (debug.nightjar.pyro_preacquire) the presenter takes the next image right
// after a present, while the next frame decodes, so that frame's present skips the acquire.
//
// Every path keeps track of both: an image acquired and not yet presented stays held until a
// present takes it or its swapchain goes, and the fence is waited for and reset before it is used
// again or destroyed. Nightjar 3.2.3 reverted a first version that dropped an acquired image when
// the fence wait after it failed, and left the fence unreset: each such failure kept one image
// from the swapchain until none were left.
//
// Acquire is `PyroAcquire(uint32_t *index)`; Settle is `bool()`, waiting for the fence and
// resetting it, true only when both worked. Present thread only.
class PyroWaveImageHold {
public:
    bool held() const { return held_; }
    bool fencePending() const { return fencePending_; }

    // After a present: the next image if one is free now (the acquire should not wait). Its fence
    // is left to take(), which by then usually finds it signalled. Holds nothing on any failure;
    // the next frame's own acquire meets and reports it.
    template <class Acquire>
    void preacquire(Acquire &&acquire) {
        if (held_ || fencePending_) return;
        uint32_t index = 0;
        if (acquire(&index) != PyroAcquire::READY) return;
        held_ = true;
        index_ = index;
        fencePending_ = true;
    }

    // The image for a present, its fence settled: the held one, else a fresh acquire. On FAILED
    // the image stays held and the fence pending, for the next take() or release() to settle.
    template <class Acquire, class Settle>
    PyroAcquire take(uint32_t *index, Acquire &&acquire, Settle &&settle) {
        if (!held_) {
            // A fence left pending by release() must signal before it is used again.
            if (fencePending_) {
                if (!settle()) return PyroAcquire::FAILED;
                fencePending_ = false;
            }
            uint32_t acquired = 0;
            const PyroAcquire result = acquire(&acquired);
            if (result != PyroAcquire::READY) return result;
            held_ = true;
            index_ = acquired;
            fencePending_ = true;
        }
        if (fencePending_) {
            if (!settle()) return PyroAcquire::FAILED;
            fencePending_ = false;
        }
        held_ = false;
        *index = index_;
        return PyroAcquire::READY;
    }

    // Before the swapchain goes (recreated, or the renderer destroyed): a held image goes with it,
    // and the fence is settled so it can be used again or destroyed. False when it never
    // signalled; it stays pending then.
    template <class Settle>
    bool release(Settle &&settle) {
        held_ = false;
        if (fencePending_) {
            if (!settle()) return false;
            fencePending_ = false;
        }
        return true;
    }

private:
    bool held_ = false;
    bool fencePending_ = false;
    uint32_t index_ = 0;
};
