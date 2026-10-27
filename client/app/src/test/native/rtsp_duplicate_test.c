#include <stdlib.h>
#include <string.h>

static int allocations;

static void* trackedMalloc(size_t size) {
    void* result = malloc(size + 4);
    if (result) {
        // Adjacent memory must not be mistaken for the end of a truncated RTSP message.
        memcpy((char*)result + size, "\n\r\n", 4);
        allocations++;
    }
    return result;
}

static void trackedFree(void* ptr) {
    if (ptr) allocations--;
    free(ptr);
}

#define malloc trackedMalloc
#define free trackedFree
#include "../../main/jni/moonlight-core/android_rtsp_parser.c"
#undef malloc
#undef free
#undef NDEBUG
#include <assert.h>

CONNECTION_LISTENER_CALLBACKS ListenerCallbacks;

int main(void) {
    char response[] = "RTSP/1.0 200 OK\r\nCSeq: 1\r\nSession: first\r\n"
                      "CSeq: 2\r\nSession: second\r\n\r\n";
    RTSP_MESSAGE message;
    assert(parseRtspMessage(&message, response, sizeof(response) - 1) == RTSP_ERROR_SUCCESS);
    assert(message.sequenceNumber == 2);
    assert(strcmp(getOptionContent(message.options, "Session"), "second") == 0);
    freeMessage(&message);
    assert(allocations == 0);

    char truncated[] = "RTSP/1.0 200 OK\r\nCSeq: 1";
    assert(parseRtspMessage(&message, truncated, sizeof(truncated) - 1) == RTSP_ERROR_MALFORMED);
    assert(allocations == 0);
    return 0;
}
