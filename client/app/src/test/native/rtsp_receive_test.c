#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <poll.h>
#include <sys/socket.h>

typedef int SOCKET;
typedef ssize_t SOCK_RET;
#define LastSocketError() errno

static bool ConnectionInterrupted;
static uint64_t now;
static int pollCalls, receiveCalls;
static const char responseText[] = "RTSP/1.0 200 OK\r\n\r\n";
static size_t responseOffset;
struct Step {
    int delayMs, pollResult, pollError, readResult, readError;
    bool cancel;
};
static const struct Step* steps;
static size_t stepCount, nextStep;
static const struct Step* readable;

static uint64_t PltGetMillis(void) { return now; }

static int pollSockets(struct pollfd* fds, int count, int timeout) {
    assert(count == 1 && fds[0].events == POLLIN);
    assert(timeout > 0 && timeout <= 100);
    pollCalls++;
    if (nextStep == stepCount || steps[nextStep].delayMs > timeout) {
        now += timeout;
        return 0;
    }
    readable = &steps[nextStep++];
    now += readable->delayMs;
    if (readable->cancel) ConnectionInterrupted = true;
    errno = readable->pollError;
    return readable->pollResult;
}

static ssize_t scriptedRecv(int fd, void* buffer, size_t length, int flags) {
    (void) fd;
    assert(flags == MSG_DONTWAIT);
    assert(readable != NULL);
    receiveCalls++;
    errno = readable->readError;
    if (readable->readResult > 0) {
        assert((size_t)readable->readResult <= length);
        for (int i = 0; i < readable->readResult; i++) {
            ((char*)buffer)[i] = responseText[responseOffset++ % (sizeof(responseText) - 1)];
        }
    }
    return readable->readResult;
}

#define recv scriptedRecv
#include "../../main/jni/moonlight-core/android_rtsp_receive.h"
#undef recv

static void reset(const struct Step* script, size_t count) {
    steps = script;
    stepCount = count;
    nextStep = responseOffset = 0;
    now = 0;
    pollCalls = receiveCalls = 0;
    ConnectionInterrupted = false;
    readable = NULL;
}

int main(void) {
    char* response;
    int length, error;
    const struct Step fragments[] = {
        {.delayMs = 10, .pollResult = 1, .readResult = 5},
        {.delayMs = 10, .pollResult = 1, .readResult = 14}, {.pollResult = 1}};
    reset(fragments, sizeof(fragments) / sizeof(fragments[0]));
    assert(receiveRtspResponse(1, &response, &length, &error, 250, 1024));
    assert(length == sizeof(responseText) - 1);
    assert(memcmp(response, responseText, length) == 0);
    free(response);

    const struct Step cancelIdle[] = {{50, 0, 0, 0, 0, true}};
    reset(cancelIdle, 1);
    assert(!receiveRtspResponse(1, &response, &length, &error, 15000, 1024));
    assert(error == EINTR && now == 50 && pollCalls == 1 && receiveCalls == 0);
    free(response);

    const struct Step cancelReady[] = {{0, 1, 0, 1, 0, true}};
    reset(cancelReady, 1);
    assert(!receiveRtspResponse(1, &response, &length, &error, 15000, 1024));
    assert(error == EINTR && receiveCalls == 0);
    free(response);

    const struct Step delayedFragments[] = {
        {.delayMs = 90, .pollResult = 1, .readResult = 1},
        {.delayMs = 90, .pollResult = 1, .readResult = 1},
        {.delayMs = 90, .pollResult = 1, .readResult = 1}};
    reset(delayedFragments, 3);
    assert(!receiveRtspResponse(1, &response, &length, &error, 250, 1024));
    assert(error == ETIMEDOUT && now == 250 && length == 2);
    free(response);

    const struct Step retryErrors[] = {
        {.pollResult = -1, .pollError = EINTR},
        {.pollResult = 1, .readResult = -1, .readError = EAGAIN},
        {.pollResult = 1, .readResult = -1, .readError = EINTR},
        {.pollResult = 1, .readResult = 19}, {.pollResult = 1}};
    reset(retryErrors, 5);
    assert(receiveRtspResponse(1, &response, &length, &error, 250, 1024));
    assert(length == sizeof(responseText) - 1 && receiveCalls == 4);
    assert(memcmp(response, responseText, length) == 0);
    free(response);

    const struct Step exactLimit[] = {{.pollResult = 1, .readResult = 19}, {.pollResult = 1}};
    reset(exactLimit, 2);
    assert(receiveRtspResponse(1, &response, &length, &error, 250, 19));
    assert(length == 19);
    free(response);

    const struct Step excessByte[] = {{.pollResult = 1, .readResult = 19}, {.pollResult = 1, .readResult = 1}};
    reset(excessByte, 2);
    assert(!receiveRtspResponse(1, &response, &length, &error, 250, 19));
    assert(error == EMSGSIZE && length == 19);
    free(response);

    // A new session can receive normally after an interrupted response has been discarded.
    reset(fragments, sizeof(fragments) / sizeof(fragments[0]));
    assert(receiveRtspResponse(2, &response, &length, &error, 250, 1024));
    assert(length == sizeof(responseText) - 1);
    free(response);
    return 0;
}
