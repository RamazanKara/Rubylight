#pragma once

// The decoder's sync fence for one frame (AImageReader_acquireNextImageAsync), and the wait for
// it before the frame is drawn: on the GPU, imported into a semaphore with
// VK_KHR_external_semaphore_fd, or on the CPU. Kept free of Vulkan types so the host tests can
// run it; the import is passed in.

#include <cerrno>
#include <poll.h>
#include <unistd.h>

namespace vkr {

// Owns a sync fence fd: closed exactly once, when the frame is known to be written or goes.
class DecoderFence {
public:
    DecoderFence() = default;
    explicit DecoderFence(int fd) : fd_(fd) {}
    ~DecoderFence() { reset(); }
    DecoderFence(const DecoderFence&) = delete;
    DecoderFence& operator=(const DecoderFence&) = delete;

    int fd() const { return fd_; }
    bool pending() const { return fd_ >= 0; }

    // Closes the fence held, and takes fd (-1 for none).
    void reset(int fd = -1) {
        if (fd_ >= 0 && fd_ != fd) close(fd_);
        fd_ = fd;
    }

private:
    int fd_ = -1;
};

// True once a sync fence fd has signalled, waiting up to timeoutMs (0 only looks).
inline bool syncFdSignalled(int fd, int timeoutMs) {
    pollfd fence {fd, POLLIN, 0};
    int ready;
    do {
        ready = poll(&fence, 1, timeoutMs);
    } while (ready < 0 && errno == EINTR);
    return ready > 0;
}

enum class DecoderWait {
    // The decoder has finished: nothing to wait for. The fence is closed.
    WRITTEN,
    // The submit waits for the imported semaphore. The frame keeps its fence.
    GPU,
    // The import was refused; waited for on the CPU instead, and written. The fence is closed.
    IMPORT_FAILED,
    // Waited for on the CPU (import refused or not tried) and never signalled. The fence is closed.
    TIMED_OUT,
};

// Arranges the wait for a frame's decoder before its draw is submitted. import(fd) imports a
// duplicate of the fence and takes ownership of it only when it returns true; a refused duplicate
// is closed here. The frame keeps its own fence through a GPU wait, so a repeated draw of it can
// wait again; WRITTEN, IMPORT_FAILED and TIMED_OUT close it.
template <class Import>
DecoderWait waitForDecoder(DecoderFence& fence, bool gpuImport, int timeoutMs, Import&& import) {
    if (!fence.pending() || syncFdSignalled(fence.fd(), 0)) {
        fence.reset();
        return DecoderWait::WRITTEN;
    }
    bool refused = false;
    if (gpuImport) {
        const int duplicate = dup(fence.fd());
        if (duplicate >= 0 && import(duplicate)) {
            return DecoderWait::GPU;
        }
        if (duplicate >= 0) close(duplicate);
        refused = true;
    }
    const bool written = syncFdSignalled(fence.fd(), timeoutMs);
    fence.reset();
    if (!written) return DecoderWait::TIMED_OUT;
    return refused ? DecoderWait::IMPORT_FAILED : DecoderWait::WRITTEN;
}

}  // namespace vkr
