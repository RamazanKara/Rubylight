#pragma once

#include <array>
#include <cassert>

// Plane ownership shared by the submission, completion and presentation threads.
// The renderer holds its slot mutex for every call, including the fence predicate.
class PyroWaveSchedule {
public:
    static constexpr int SLOT_COUNT = 3;
    static constexpr int MAX_IN_FLIGHT = 2;

    template<class DrawFinished>
    int reserve(DrawFinished drawFinished) {
        if (inFlight_ >= MAX_IN_FLIGHT) return -1;
        for (int offset = 1; offset <= SLOT_COUNT; ++offset) {
            const int index = (lastReserved_ + offset) % SLOT_COUNT;
            if (decoding_[index] || index == ready_ || index == drawing_ || !drawFinished(index)) continue;
            decoding_[index] = true;
            ++inFlight_;
            lastReserved_ = index;
            return index;
        }
        return -1;
    }

    // Called only after this slot's GPU completion. Publish its output event first,
    // before making the slot visible to the presenter or replacing an older output.
    // Completions follow submission order on the decoder's timeline.
    int complete(int index, bool display = true) {
        assert(isDecoding(index));
        if (!isDecoding(index)) return -1;
        decoding_[index] = false;
        --inFlight_;
        if (!display) return -1;
        const int replaced = ready_;
        ready_ = index;
        return replaced;
    }

    // Undo a reservation only when no GPU work was submitted. A failed or timed-out
    // GPU submission must retain its planes until the renderer has stopped the device.
    void cancel(int index) {
        assert(isDecoding(index));
        if (!isDecoding(index)) return;
        decoding_[index] = false;
        --inFlight_;
    }

    int beginPresent() {
        assert(drawing_ < 0);
        if (drawing_ >= 0 || ready_ < 0) return -1;
        drawing_ = ready_;
        ready_ = -1;
        return drawing_;
    }

    // After the swapchain image becomes available, use the newest completed frame.
    // The caller records the returned old frame as dropped.
    int replaceDrawingWithReady() {
        if (drawing_ < 0 || ready_ < 0) return -1;
        const int replaced = drawing_;
        drawing_ = ready_;
        ready_ = -1;
        return replaced;
    }

    // A submitted draw may still be using these planes. reserve() also requires the
    // renderer's draw fence to be signaled before returning the slot for another decode.
    void finishPresent() { drawing_ = -1; }

    int ready() const { return ready_; }
    int drawing() const { return drawing_; }
    int inFlight() const { return inFlight_; }
    bool isDecoding(int index) const {
        return index >= 0 && index < SLOT_COUNT && decoding_[index];
    }

private:
    std::array<bool, SLOT_COUNT> decoding_{};
    int inFlight_ = 0;
    int ready_ = -1;
    int drawing_ = -1;
    int lastReserved_ = SLOT_COUNT - 1;
};
