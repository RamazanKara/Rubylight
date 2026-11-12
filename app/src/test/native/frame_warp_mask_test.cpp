#include <cassert>
#include <cstdint>
#include <cstring>
#include <vector>
#include "../../main/jni/frame_warp_mask.h"

namespace {

// The host side of FrameWarpApi::mask as frame_warp.rs answers it: a generation (-1 for none) and
// the cells, copied only into room enough for them.
struct FakeHost {
    int32_t generation = -1;
    uint32_t width = 0;
    uint32_t height = 0;
    std::vector<uint8_t> cells;
    int calls = 0;
};

FakeHost host;

int64_t fakeFrameForPts(int64_t) { return -1; }

int32_t fakeWarp(int64_t, float *out) {
    if (out != nullptr) {
        out[0] = out[1] = out[2] = out[3] = 0.0f;
    }
    return FRAME_WARP_INACTIVE;
}

int32_t fakeMask(int32_t known, uint32_t *width, uint32_t *height, uint8_t *cells, size_t capacity) {
    ++host.calls;
    if (host.generation < 0) {
        return known < 0 ? FRAME_WARP_MASK_UNCHANGED : FRAME_WARP_MASK_CLEARED;
    }
    if (known == host.generation) {
        return FRAME_WARP_MASK_UNCHANGED;
    }
    *width = host.width;
    *height = host.height;
    const size_t size = static_cast<size_t>(host.width) * host.height;
    if (cells == nullptr || capacity < size) {
        return FRAME_WARP_MASK_TOO_SMALL;
    }
    std::memcpy(cells, host.cells.data(), size);
    return host.generation;
}

const FrameWarpApi api = {FRAME_WARP_API_VERSION, fakeFrameForPts, fakeWarp, fakeMask, fakeWarp};

void sendMask(int32_t generation, uint32_t width, uint32_t height, uint8_t fill) {
    host.generation = generation;
    host.width = width;
    host.height = height;
    host.cells.assign(static_cast<size_t>(width) * height, fill);
}

void startsEmptyAndStaysSoWithoutAMask() {
    host = FakeHost();
    FrameWarpCells cells;
    assert(!cells.any());
    assert(cells.generation == FrameWarpCells::kEmpty);
    assert(!cells.poll(&api));
    assert(!cells.any());
    assert(host.calls == 1);
}

void growsToTheMaskAndTakesIt() {
    host = FakeHost();
    FrameWarpCells cells;
    sendMask(4, 3, 2, 255);
    // The first ask has no room: the size comes back, the cells grow and the second ask fills them
    assert(cells.poll(&api));
    assert(host.calls == 2);
    assert(cells.any());
    assert(cells.generation == 4 && cells.width == 3 && cells.height == 2);
    assert(cells.cells.size() == 6);
    for (uint8_t cell : cells.cells) {
        assert(cell == 255);
    }

    // The same generation again: one ask, nothing changes
    assert(!cells.poll(&api));
    assert(host.calls == 3);
    assert(cells.generation == 4);

    // A smaller newer mask fits the cells already there: one ask
    sendMask(5, 2, 2, 0);
    assert(cells.poll(&api));
    assert(host.calls == 4);
    assert(cells.generation == 5 && cells.width == 2 && cells.height == 2);
    assert(cells.cells[0] == 0 && cells.cells[3] == 0);
}

void clearsWhenTheHostDropsTheMask() {
    host = FakeHost();
    FrameWarpCells cells;
    sendMask(7, 2, 1, 255);
    assert(cells.poll(&api));
    host.generation = -1;
    assert(cells.poll(&api));
    assert(!cells.any());
    assert(cells.generation == FrameWarpCells::kEmpty && cells.width == 0 && cells.height == 0);
    // Already empty: nothing changed
    assert(!cells.poll(&api));
}

void refusesAMaskLargerThanAnyTheHostSends() {
    host = FakeHost();
    FrameWarpCells cells;
    host.generation = 2;
    host.width = FrameWarpCells::kMaxSide + 1;
    host.height = 1;
    assert(!cells.poll(&api));
    assert(!cells.any());
    assert(cells.cells.empty());
    assert(host.calls == 1);
}

void redrawsOnlyAfterAQuarterPixel() {
    assert(!frameWarpMoved(0.0f, 0.0f, 0.0f, 0.0f));
    assert(!frameWarpMoved(1.0f, -2.0f, 1.2f, -2.0f));
    assert(frameWarpMoved(1.0f, -2.0f, 1.25f, -2.0f));
    assert(frameWarpMoved(0.0f, 0.0f, 0.0f, -0.3f));
    // Diagonal: 0.2 across and 0.2 down is 0.28 pixels
    assert(frameWarpMoved(0.0f, 0.0f, 0.2f, 0.2f));
    assert(!frameWarpMoved(0.0f, 0.0f, 0.15f, 0.15f));
}

} // namespace

int main() {
    startsEmptyAndStaysSoWithoutAMask();
    growsToTheMaskAndTakesIt();
    clearsWhenTheHostDropsTheMask();
    refusesAMaskLargerThanAnyTheHostSends();
    redrawsOnlyAfterAQuarterPixel();
    return 0;
}
