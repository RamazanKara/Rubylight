#include <cassert>
#include <cerrno>
#include <cstdarg>
#include <string>
#include <vector>
#include "../../main/jni/window-dataspace/window_dataspace.h"

using namespace nwds;

// Every call the probe made, in order: "get", "public <dataspace>", "perform <op> <dataspace>"
static std::vector<std::string> calls;
static int publicResult;
static int performResult;
static int previous;

static int fakePerform(NativeWindowMirror*, int operation, ...) {
    va_list args;
    va_start(args, operation);
    const int dataspace = va_arg(args, int);
    va_end(args);
    calls.push_back("perform " + std::to_string(operation) + " " + std::to_string(dataspace));
    return performResult;
}

static NativeWindowMirror window() {
    NativeWindowMirror window {};
    window.common.magic = kNativeWindowMagic;
    window.common.version = sizeof(NativeWindowMirror);
    window.perform = fakePerform;
    return window;
}

static DataspaceProbe probe(NativeWindowMirror* fake) {
    calls.clear();
    return probeDataspace(
            fake, kDataspaceBt2020Hlg,
            [](void* w, int32_t d) {
                assert(w != nullptr);
                calls.push_back("public " + std::to_string(d));
                // The previous dataspace always goes back through the public call here.
                return d == kDataspaceBt2020Hlg ? publicResult : 0;
            },
            [](void*) {
                calls.push_back("get");
                return previous;
            });
}

static const std::string HLG = std::to_string(kDataspaceBt2020Hlg);
static const int PQ = 163971072;

static void theConstantsAreThePlatformsOwn() {
    assert(kNativeWindowMagic == 0x5f776e64);
    assert(kNativeWindowSetBuffersDataspace == 19);
    assert(kDataspaceBt2020Hlg == 0xa060000);
}

static void aPublicAcceptIsPutBackAndPerformIsNeverCalled() {
    NativeWindowMirror fake = window();
    publicResult = 0;
    performResult = -EINVAL;
    previous = PQ;
    DataspaceProbe result = probe(&fake);
    assert(result.set.call == DataspaceCall::kPublic && result.set.accepted());
    assert(result.set.performResult == kNotCalled);
    assert(result.previousKnown && result.previous == PQ);
    assert(result.restore.call == DataspaceCall::kPublic);
    assert((calls == std::vector<std::string>{"get", "public " + HLG, "public " + std::to_string(PQ)}));
}

static void aPublicRefusalAsksPerformWithOperation19() {
    NativeWindowMirror fake = window();
    publicResult = -EINVAL;  // android13 isDataSpaceValid
    performResult = 0;
    previous = PQ;
    DataspaceProbe result = probe(&fake);
    assert(result.set.call == DataspaceCall::kPerform && result.set.accepted());
    assert(result.set.publicResult == -EINVAL && result.set.performResult == 0);
    assert(result.restore.call == DataspaceCall::kPublic);
    assert((calls == std::vector<std::string>{"get", "public " + HLG, "perform 19 " + HLG, "public " + std::to_string(PQ)}));
}

static void bothRefusingLeavesTheWindowAlone() {
    NativeWindowMirror fake = window();
    publicResult = -EINVAL;
    performResult = -EINVAL;
    previous = PQ;
    DataspaceProbe result = probe(&fake);
    assert(result.set.call == DataspaceCall::kNone && !result.set.accepted());
    assert(result.set.publicResult == -EINVAL && result.set.performResult == -EINVAL);
    // Nothing was changed, so nothing is put back.
    assert(result.restore.publicResult == kNotCalled && result.restore.performResult == kNotCalled);
    assert((calls == std::vector<std::string>{"get", "public " + HLG, "perform 19 " + HLG}));
}

static void anotherWindowLayoutIsNeverPerformed() {
    for (int field = 0; field < 3; field++) {
        NativeWindowMirror fake = window();
        if (field == 0) fake.common.magic = 0;
        if (field == 1) fake.common.version = 0;
        if (field == 2) fake.perform = nullptr;
        publicResult = -EINVAL;
        performResult = 0;
        previous = PQ;
        DataspaceProbe result = probe(&fake);
        assert(result.set.call == DataspaceCall::kNone);
        assert(result.set.performResult == kNotMirrored);
        assert((calls == std::vector<std::string>{"get", "public " + HLG}));
    }
}

static void aMissingPublicCallGoesToPerform() {
    NativeWindowMirror fake = window();
    publicResult = kMissing;
    performResult = 0;
    previous = 0;
    DataspaceProbe result = probe(&fake);
    assert(result.set.call == DataspaceCall::kPerform && result.set.publicResult == kMissing);
}

static void anUnknownPreviousDataspaceRestoresUnknown() {
    NativeWindowMirror fake = window();
    publicResult = 0;
    performResult = 0;
    previous = -EINVAL;
    DataspaceProbe result = probe(&fake);
    assert(!result.previousKnown && result.previous == kDataspaceUnknown);
    assert((calls == std::vector<std::string>{"get", "public " + HLG, "public 0"}));
}

static void setWithoutPerformStopsAtThePublicCall() {
    NativeWindowMirror fake = window();
    performResult = 0;
    calls.clear();
    DataspaceOutcome outcome = setDataspace(&fake, PQ, [](void*, int32_t) { return -EINVAL; }, false);
    assert(outcome.call == DataspaceCall::kNone && outcome.performResult == kNotCalled);
    assert(calls.empty());
    outcome = setDataspace(&fake, kDataspaceBt2020Hlg, [](void*, int32_t) { return -EINVAL; }, true);
    assert(outcome.call == DataspaceCall::kPerform);
    assert((calls == std::vector<std::string>{"perform 19 " + HLG}));
}

int main() {
    theConstantsAreThePlatformsOwn();
    aPublicAcceptIsPutBackAndPerformIsNeverCalled();
    aPublicRefusalAsksPerformWithOperation19();
    bothRefusingLeavesTheWindowAlone();
    anotherWindowLayoutIsNeverPerformed();
    aMissingPublicCallGoesToPerform();
    anUnknownPreviousDataspaceRestoresUnknown();
    setWithoutPerformStopsAtThePublicCall();
    return 0;
}
