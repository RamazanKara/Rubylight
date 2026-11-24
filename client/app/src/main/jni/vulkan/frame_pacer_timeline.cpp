// HostTimeline: sliding transit-time window and jitter-buffer adaptation.

#include "frame_pacer.h"
#include "frame_pacer_internal.h"

#include <algorithm>
#include <cmath>

namespace vkr {

using namespace pacer_detail;

HostTimeline::HostTimeline(JitterBuffer jitterBuffer) : jitterBuffer_(jitterBuffer) {
    applyPreset();
}

void HostTimeline::setJitterBuffer(JitterBuffer jitterBuffer) {
    jitterBuffer_ = jitterBuffer;
    applyPreset();
}

void HostTimeline::applyPreset() {
    switch (jitterBuffer_) {
        case JitterBuffer::LowLatency:
            baseWindowNs_ = 2'000'000'000;
            baseCoverage_ = 0.98;
            baseDecayDivisor_ = 32;
            break;
        case JitterBuffer::LowestLatency:
        case JitterBuffer::Instant:
            baseWindowNs_ = 2'000'000'000;
            baseCoverage_ = 0.90;
            baseDecayDivisor_ = 32;
            break;
        case JitterBuffer::Smooth:
            baseWindowNs_ = 30'000'000'000;
            baseCoverage_ = 0.999;
            baseDecayDivisor_ = 4096;
            break;
        case JitterBuffer::Balanced:
        default:
            baseWindowNs_ = 3'000'000'000;
            baseCoverage_ = 0.99;
            baseDecayDivisor_ = 64;
            break;
    }
    windowNs_ = std::max(baseWindowNs_, minWindowNs_);
    decayDivisor_ = std::max(baseDecayDivisor_, minDecayDivisor_);
    setSlotVsyncs(slotVsyncs_);
}

void HostTimeline::setSlotVsyncs(int64_t slotVsyncs) {
    slotVsyncs_ = slotVsyncs;
    if (jitterBuffer_ == JitterBuffer::LowLatency && slotVsyncs >= 2) {
        coverage_ = std::min(baseCoverage_, kLowLatencySlowSlotCoverage);
    }
    else {
        coverage_ = baseCoverage_;
    }
}

void HostTimeline::reset() {
    window_.clear();
    minQueue_.clear();
    quantile_.clear();
    offsetNs_ = 0;
    lastPtsNs_ = 0;
    started_ = false;
}

void HostTimeline::restartWindow(int64_t minWindowNs, int64_t minDecayDivisor) {
    window_.clear();
    minQueue_.clear();
    quantile_.clear();
    minWindowNs_ = minWindowNs;
    minDecayDivisor_ = minDecayDivisor;
    windowNs_ = std::max(baseWindowNs_, minWindowNs);
    decayDivisor_ = std::max(baseDecayDivisor_, minDecayDivisor);
}

void HostTimeline::shift(int64_t deltaNs) {
    offsetNs_ += deltaNs;
    quantile_.shift(deltaNs);
    for (Sample& s : window_) {
        s.transitNs += deltaNs;
    }
    for (Sample& s : minQueue_) {
        s.transitNs += deltaNs;
    }
}

bool HostTimeline::checkContinuity(int64_t hostPtsNs) {
    bool continuous = true;
    if (started_ && (hostPtsNs < lastPtsNs_ - kMaxPtsBackstepNs || hostPtsNs > lastPtsNs_ + kMaxPtsGapNs)) {
        reset();
        continuous = false;
    }
    lastPtsNs_ = hostPtsNs;
    return continuous;
}

int64_t HostTimeline::minTransitNs() const {
    return minQueue_.empty() ? 0 : minQueue_.front().transitNs;
}

void HostTimeline::addSample(int64_t scheduledPtsNs, int64_t arrivalNs) {
    if (!started_) {
        started_ = true;
        startNs_ = arrivalNs;
        warmedUp_ = false;
    }
    const bool warmingUp = arrivalNs - startNs_ < kWarmupNs;
    const Sample sample {arrivalNs, arrivalNs - scheduledPtsNs};

    // The warm-up's frames (the first keyframe is large, and the decoder is starting) say little
    // about the stream once it settles. Kept in the window, they held the buffer near its cap
    // for seconds: 50 ms at the start of a 60 fps stream, taking five seconds to come down.
    if (!warmingUp && !warmedUp_) {
        window_.clear();
        minQueue_.clear();
        quantile_.clear();
    }

    window_.push_back(sample);
    quantile_.push(sample.transitNs);
    while (!minQueue_.empty() && minQueue_.back().transitNs >= sample.transitNs) {
        minQueue_.pop_back();
    }
    minQueue_.push_back(sample);

    while (window_.front().arrivalNs < arrivalNs - windowNs_) {
        const Sample& old = window_.front();
        if (minQueue_.front().arrivalNs == old.arrivalNs && minQueue_.front().transitNs == old.transitNs) {
            minQueue_.pop_front();
        }
        window_.pop_front();
        quantile_.popOldest();
    }

    // Through the warm-up, no buffer: frames are scheduled by the quickest transit seen, and any
    // that arrive later are shown as they arrive. The buffer then starts from the settled stream,
    // growing at once if frames arrive later and coming down slowly.
    if (warmingUp) {
        offsetNs_ = minTransitNs();
        return;
    }
    warmedUp_ = true;

    // Until there are enough samples for a percentile, cover all of them. After that, leave out
    // at least the two slowest: with few samples, the percentile is otherwise just the slowest
    // frame, which may be a one-off.
    const size_t count = window_.size();
    const double n = static_cast<double>(count);
    size_t index = count - 1;
    if (count >= 8) {
        const double coverage = std::min(coverage_, 1.0 - 2.0 / n);
        index = static_cast<size_t>(std::ceil(coverage * (n - 1)));
    }
    const int64_t target = quantile_.kth(index);

    // Take more delay right away when frames start arriving later, and give it back slowly
    // so one quiet stretch doesn't leave us exposed to the next burst of jitter
    if (target > offsetNs_) {
        offsetNs_ = target;
    }
    else {
        offsetNs_ -= (offsetNs_ - target) / decayDivisor_;
    }

    if (maxBufferNs_ > 0) {
        offsetNs_ = std::min(offsetNs_, minTransitNs() + maxBufferNs_);
    }
}


}  // namespace vkr
