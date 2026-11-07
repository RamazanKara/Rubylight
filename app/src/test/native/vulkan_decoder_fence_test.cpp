#include <cassert>
#include <fcntl.h>
#include <sys/eventfd.h>
#include <unistd.h>
#include <vector>
#include "../../main/jni/vulkan/decoder_fence.h"

using namespace vkr;

// An eventfd stands in for a sync fence: readable (signalled) once written to.
static int fence() {
    const int fd = eventfd(0, EFD_CLOEXEC);
    assert(fd >= 0);
    return fd;
}

static void signalFence(int fd) {
    const uint64_t one = 1;
    const ssize_t written = write(fd, &one, sizeof(one));
    assert(written == ssize_t(sizeof(one)));
    (void) written;
}

static bool isOpen(int fd) {
    return fcntl(fd, F_GETFD) != -1;
}

static void signalledFenceIsClosedWithoutAnImport() {
    const int fd = fence();
    signalFence(fd);
    DecoderFence owned(fd);
    bool imported = false;
    assert(waitForDecoder(owned, true, 10, [&](int) { return imported = true; }) == DecoderWait::WRITTEN);
    assert(!imported && !owned.pending() && !isOpen(fd));
    // No fence at all: nothing to wait for.
    DecoderFence none;
    assert(waitForDecoder(none, true, 10, [](int) { return true; }) == DecoderWait::WRITTEN);
}

static void gpuWaitTakesADuplicateAndTheFrameKeepsItsFence() {
    const int fd = fence();
    std::vector<int> vulkan;  // Imported duplicates, Vulkan's to close.
    {
        DecoderFence owned(fd);
        const auto import = [&](int duplicate) {
            assert(duplicate != fd && isOpen(duplicate));
            vulkan.push_back(duplicate);
            return true;
        };
        assert(waitForDecoder(owned, true, 10, import) == DecoderWait::GPU);
        assert(owned.pending() && owned.fd() == fd && isOpen(fd));
        // A repeated draw before the decoder finished waits again, on a new duplicate.
        assert(waitForDecoder(owned, true, 10, import) == DecoderWait::GPU);
        assert(vulkan.size() == 2 && vulkan[0] != vulkan[1]);
        // Once written, the next draw closes it.
        signalFence(fd);
        assert(waitForDecoder(owned, true, 10, import) == DecoderWait::WRITTEN);
        assert(!owned.pending() && !isOpen(fd) && vulkan.size() == 2);
    }
    for (int duplicate : vulkan) {
        assert(isOpen(duplicate));
        close(duplicate);
    }

    // A frame dropped before its draw closes its fence once.
    const int dropped = fence();
    {
        DecoderFence owned(dropped);
        assert(waitForDecoder(owned, true, 10, [](int duplicate) { close(duplicate); return true; }) ==
               DecoderWait::GPU);
    }
    assert(!isOpen(dropped));
}

static void refusedImportClosesItsDuplicateAndWaitsOnTheCpu() {
    const int fd = fence();
    int refused = -1;
    DecoderFence owned(fd);
    signalFence(fd);
    // Signalled before the wait: WRITTEN without trying the import.
    assert(waitForDecoder(owned, true, 10, [&](int duplicate) { refused = duplicate; return false; }) ==
           DecoderWait::WRITTEN);
    assert(refused == -1);

    // Pending: the import is refused, its duplicate closed, and the CPU waits instead.
    const int pending = fence();
    DecoderFence second(pending);
    const auto refuse = [&](int duplicate) {
        refused = duplicate;
        signalFence(pending);  // The decoder finishes while the import fails.
        return false;
    };
    assert(waitForDecoder(second, true, 10, refuse) == DecoderWait::IMPORT_FAILED);
    assert(refused >= 0 && !isOpen(refused) && !second.pending() && !isOpen(pending));
}

static void cpuWaitThatTimesOutClosesTheFence() {
    const int fd = fence();
    DecoderFence owned(fd);
    bool imported = false;
    assert(waitForDecoder(owned, false, 5, [&](int) { return imported = true; }) == DecoderWait::TIMED_OUT);
    assert(!imported && !owned.pending() && !isOpen(fd));

    const int refusedFd = fence();
    DecoderFence refused(refusedFd);
    assert(waitForDecoder(refused, true, 5, [](int) { return false; }) == DecoderWait::TIMED_OUT);
    assert(!refused.pending() && !isOpen(refusedFd));
}

int main() {
    signalledFenceIsClosedWithoutAnImport();
    gpuWaitTakesADuplicateAndTheFrameKeepsItsFence();
    refusedImportClosesItsDuplicateAndWaitsOnTheCpu();
    cpuWaitThatTimesOutClosesTheFence();
    return 0;
}
