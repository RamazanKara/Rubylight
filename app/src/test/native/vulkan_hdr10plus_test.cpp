#include <cassert>
#include <cerrno>
#include <cstdarg>
#include <cstring>
#include <type_traits>
#include <vector>
#include "../../main/jni/vulkan/hdr10plus_window.h"
#include "../../main/jni/window-dataspace/window_dataspace.h"

using namespace vkr;

// What the fake window's perform was called with
struct Calls {
    int count = 0;
    int operation = -1;
    std::vector<uint8_t> blob;
    bool nullData = false;
    int result = 0;
};
static Calls calls;

static int fakePerform(NativeWindowMirror*, int operation, ...) {
    va_list args;
    va_start(args, operation);
    const size_t size = va_arg(args, size_t);
    const uint8_t* data = va_arg(args, const uint8_t*);
    va_end(args);
    calls.count++;
    calls.operation = operation;
    calls.nullData = data == nullptr;
    calls.blob.assign(data, data + (data ? size : 0));
    return calls.result;
}

static NativeWindowMirror window() {
    NativeWindowMirror window {};
    window.common.magic = kNativeWindowMagic;
    window.common.version = sizeof(NativeWindowMirror);
    window.perform = fakePerform;
    return window;
}

// The start of an HDR10+ T.35 payload (hdr_dynamic.rs Hdr10Plus::to_t35)
const uint8_t kBlob[] = {0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04, 0x01, 0x40};

static void theMagicIsAndroidsWindowMagic() {
    assert(kNativeWindowMagic == 0x5f776e64);
}

// HDR10+ and the window dataspace call perform on the one ANativeWindow mirror (native_window_mirror.h)
static_assert(std::is_same<NativeWindowMirror, nwds::NativeWindowMirror>::value, "one ANativeWindow mirror");

static void performGetsOperation34WithThePayload() {
    calls = {};
    NativeWindowMirror fake = window();
    Hdr10PlusWindow hdr10Plus(&fake);
    assert(hdr10Plus.set(kBlob, sizeof(kBlob)) == Hdr10PlusResult::kSet);
    assert(calls.count == 1 && calls.operation == 34);
    assert(calls.blob == std::vector<uint8_t>(kBlob, kBlob + sizeof(kBlob)));
    assert(hdr10Plus.holds() && !hdr10Plus.refused());

    // Clearing passes size 0 and no data, once
    assert(hdr10Plus.set(nullptr, 0) == Hdr10PlusResult::kCleared);
    assert(calls.count == 2 && calls.blob.empty() && calls.nullData);
    assert(!hdr10Plus.holds());
    assert(hdr10Plus.set(nullptr, 0) == Hdr10PlusResult::kUnchanged);
    assert(calls.count == 2);
}

static void nothingIsClearedThatWasNeverSet() {
    calls = {};
    NativeWindowMirror fake = window();
    Hdr10PlusWindow hdr10Plus(&fake);
    assert(hdr10Plus.set(nullptr, 0) == Hdr10PlusResult::kUnchanged);
    assert(hdr10Plus.set(kBlob, 0) == Hdr10PlusResult::kUnchanged);
    assert(calls.count == 0);
}

static void aWindowOfAnotherLayoutIsRefusedOnceAndNeverCalled() {
    calls = {};
    NativeWindowMirror wrongVersion = window();
    wrongVersion.common.version = sizeof(NativeWindowMirror) + sizeof(void*);
    Hdr10PlusWindow hdr10Plus(&wrongVersion);
    assert(hdr10Plus.set(kBlob, sizeof(kBlob)) == Hdr10PlusResult::kUnsupported);
    assert(hdr10Plus.refused() && hdr10Plus.version() == static_cast<int>(sizeof(NativeWindowMirror) + sizeof(void*)));
    assert(hdr10Plus.set(kBlob, sizeof(kBlob)) == Hdr10PlusResult::kUnchanged);
    assert(calls.count == 0);

    NativeWindowMirror wrongMagic = window();
    wrongMagic.common.magic = 0x5f627566;  // '_buf', a window buffer
    Hdr10PlusWindow other(&wrongMagic);
    assert(other.set(kBlob, sizeof(kBlob)) == Hdr10PlusResult::kUnsupported);
    assert(other.magic() == 0x5f627566);
    assert(calls.count == 0);

    NativeWindowMirror noPerform = window();
    noPerform.perform = nullptr;
    Hdr10PlusWindow third(&noPerform);
    assert(third.set(kBlob, sizeof(kBlob)) == Hdr10PlusResult::kUnsupported);

    Hdr10PlusWindow none(nullptr);
    assert(none.set(kBlob, sizeof(kBlob)) == Hdr10PlusResult::kUnchanged && !none.refused());
}

static void anUnknownOperationIsReportedOnceAndThenLeftAlone() {
    calls = {};
    calls.result = -ENOENT;
    NativeWindowMirror fake = window();
    Hdr10PlusWindow hdr10Plus(&fake);
    assert(hdr10Plus.set(kBlob, sizeof(kBlob)) == Hdr10PlusResult::kFailed);
    assert(hdr10Plus.error() == -ENOENT && hdr10Plus.refused() && !hdr10Plus.holds());
    assert(calls.count == 1);
    // Later frames, and the clear when HDR ends, call nothing more
    assert(hdr10Plus.set(kBlob, sizeof(kBlob)) == Hdr10PlusResult::kUnchanged);
    assert(hdr10Plus.set(nullptr, 0) == Hdr10PlusResult::kUnchanged);
    assert(calls.count == 1);
}

int main() {
    theMagicIsAndroidsWindowMagic();
    performGetsOperation34WithThePayload();
    nothingIsClearedThatWasNeverSet();
    aWindowOfAnotherLayoutIsRefusedOnceAndNeverCalled();
    anUnknownOperationIsReportedOnceAndThenLeftAlone();
    return 0;
}
