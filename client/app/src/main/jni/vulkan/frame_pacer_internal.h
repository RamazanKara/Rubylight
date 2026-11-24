// Internal constants and helpers shared across the frame_pacer translation units.
#pragma once

#include <cmath>
#include <cstddef>
#include <cstdint>

namespace vkr {
namespace pacer_detail {

// A timestamp this far outside the running timeline means the stream restarted
inline constexpr int64_t kMaxPtsBackstepNs = 100'000'000;
inline constexpr int64_t kMaxPtsGapNs = 5'000'000'000;

// Phase locking (see FramePacer::updatePhase)
inline constexpr int64_t kMinPhaseSpanNs = 2'000'000'000;
inline constexpr double kMaxDriftPerFrame = 0.02;
inline constexpr double kLockArc = 0.7;         // Spread of frames around the fitted line, in slots
inline constexpr double kUnlockArc = 1.0;       // Past a full slot, frames would collide
inline constexpr double kPhaseMargin = 0.1;     // Clearance from the vsyncs where frames change
inline constexpr double kScheduleWidth = 0.04;  // Allowance for the fit moving
inline constexpr double kLateDecayPerFrame = 0.0005;
inline constexpr int64_t kPlacementSettleNs = 5'000'000'000;
// Settled, with a frame every few vsyncs: how long every frame must have been due this long
// before the vsync ahead of its own for the slots to move there (see updatePhase())
inline constexpr int64_t kSoonerHoldNs = 3'000'000'000;
inline constexpr int64_t kSoonerRoomDivisor = 4;  // A quarter vsync
// ...and how long the host drifting slow must take to carry them back
inline constexpr double kSoonerKeepNs = 60e9;
// While frames are scheduled on the fitted line with the buffer measured against it (Balanced
// and Smooth, see updatePhase()), the buffer gives delay back over about 17 s (1024 frames at
// 60 fps): stretches of late host timestamps come back, and a buffer that forgot them between
// stretches left the frames at the start of each one late
inline constexpr int64_t kLineDecayDivisor = 1024;
inline constexpr size_t kSlotDeltaWindow = 63;

// Vsyncs the period is measured over
inline constexpr size_t kVsyncWindow = 480;
inline constexpr size_t kMinVsyncsForPeriod = 60;
// The period is refitted this often at most, unless a vsync comes off its place. One new
// sample in the window moves the fit by little, and the renderer refines the period on every
// callback in between.
inline constexpr size_t kRefitEveryVsyncs = 8;

// Presets chosen by replaying Wi-Fi sessions at 72/144 fps (tools/pacer-trace.sh): Balanced
// 3 s / 99% / 64, Smooth 30 s / 99.9% / 4096, LowLatency 2 s / 98% (95% at >=2 vsyncs/frame) / 32,
// LowestLatency & Instant 2 s / 90% / 32.
inline constexpr double kLowLatencySlowSlotCoverage = 0.95;

inline double wrapCycles(double x) {
    x -= std::floor(x);
    return x >= 1.0 ? 0.0 : x;
}

}  // namespace pacer_detail
}  // namespace vkr
