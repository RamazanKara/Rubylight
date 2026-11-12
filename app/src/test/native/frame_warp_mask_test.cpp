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

uint32_t fakeInputSeq = 0;
int32_t fakeStick = 0;

uint32_t fakeInput() { return fakeInputSeq; }

int32_t fakeStickDeflected() { return fakeStick; }

const FrameWarpApi api = {FRAME_WARP_API_VERSION, fakeFrameForPts, fakeWarp, fakeMask, fakeWarp,
                          fakeInput, fakeStickDeflected};

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

void looksAgainOnlyWhileLockedAndSomethingMoves() {
    FrameWarpDrawn drawn;
    fakeInputSeq = 10;
    fakeStick = 0;
    // Nothing drawn yet, or drawn as it is (unlocked): no refresh looks, whatever is sent
    assert(!drawn.mayMove(&api));
    const float still[4] = {0.0f, 0.0f, 0.0f, 0.0f};
    drawn.note(FRAME_WARP_INACTIVE, still, 10);
    fakeInputSeq = 11;
    fakeStick = 1;
    assert(!drawn.mayMove(&api));

    // Locked, at rest: only new input or a deflected stick
    fakeStick = 0;
    drawn.note(FRAME_WARP_NO_MASK, still, 11);
    assert(drawn.active);
    assert(!drawn.mayMove(&api));
    fakeInputSeq = 12;
    assert(drawn.mayMove(&api));
    fakeInputSeq = 11;
    fakeStick = 1;
    assert(drawn.mayMove(&api));
    fakeStick = 0;

    // A shift on screen can still move (the turn in flight, a fade out)
    const float turning[4] = {0.01f, 0.0f, 19.2f, 0.0f};
    drawn.note(3, turning, 11);
    assert(drawn.px[0] == 19.2f && drawn.px[1] == 0.0f);
    assert(drawn.mayMove(&api));

    // An inactive draw keeps no shift
    drawn.note(FRAME_WARP_INACTIVE, turning, 11);
    assert(!drawn.active && drawn.px[0] == 0.0f);
}

void thePictureLeavesOutAnEncodersPadding() {
    // AMD's AV1 at 2184x1968 decodes as 2240x1968: 56 black columns at the right
    FrameWarpRect padded = frameWarpPicture({0, 0, 2240, 1968}, 2184, 1968);
    assert(padded.left == 0 && padded.top == 0 && padded.right == 2184 && padded.bottom == 1968);
    // 1920x1080 as 1920x1082, and a crop that does not start at the corner
    FrameWarpRect rows = frameWarpPicture({0, 0, 1920, 1082}, 1920, 1080);
    assert(rows.right == 1920 && rows.bottom == 1080);
    FrameWarpRect offset = frameWarpPicture({8, 4, 2248, 1972}, 2184, 1968);
    assert(offset.left == 8 && offset.top == 4 && offset.right == 2192 && offset.bottom == 1972);
    // The stream's own size, and a crop already inside it, are the picture
    FrameWarpRect exact = frameWarpPicture({0, 0, 2184, 1968}, 2184, 1968);
    assert(exact.right == 2184 && exact.bottom == 1968);
    FrameWarpRect smaller = frameWarpPicture({0, 0, 1280, 720}, 2184, 1968);
    assert(smaller.right == 1280 && smaller.bottom == 720);
    // More than alignment adds (another size altogether), or no stream size: the crop as it is
    FrameWarpRect wider = frameWarpPicture({0, 0, 2248, 1968}, 2184, 1968);
    assert(wider.right == 2248);
    FrameWarpRect taller = frameWarpPicture({0, 0, 2240, 1984}, 2184, 1968);
    assert(taller.right == 2240 && taller.bottom == 1984);
    FrameWarpRect unknown = frameWarpPicture({0, 0, 2240, 1968}, 0, 0);
    assert(unknown.right == 2240);
}

} // namespace

int main() {
    startsEmptyAndStaysSoWithoutAMask();
    growsToTheMaskAndTakesIt();
    clearsWhenTheHostDropsTheMask();
    refusesAMaskLargerThanAnyTheHostSends();
    redrawsOnlyAfterAQuarterPixel();
    looksAgainOnlyWhileLockedAndSomethingMoves();
    thePictureLeavesOutAnEncodersPadding();
    return 0;
}
