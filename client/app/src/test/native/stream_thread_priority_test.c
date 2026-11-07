// stream_thread_priority.h: latency.c raises the thread that calls the decoder (VideoRecv with
// direct submission, VideoDec without) to nice -8, the other stream threads to -4, nothing with
// "prioritize stream traffic" off, and debug.nightjar.decode_priority 0 restores the old levels.
#include "stream_thread_priority.h"
#undef NDEBUG
#include <assert.h>
#include <stdio.h>

static void theDecodingThreadGetsTheDecodeLevel(void) {
    assert(streamThreadNice("VideoRecv", true, true, true) == DECODE_THREAD_NICE);
    assert(streamThreadNice("VideoRecv", true, false, true) == STREAM_THREAD_NICE);
    assert(streamThreadNice("VideoDec", true, false, true) == DECODE_THREAD_NICE);
}

static void theOtherStreamThreadsStayAtTheStreamLevel(void) {
    const char* names[] = { "AudioRecv", "AudioDec", "ControlRecv", "InputSend" };
    for (size_t i = 0; i < sizeof(names) / sizeof(names[0]); i++) {
        for (int decode = 0; decode < 2; decode++) {
            assert(streamThreadNice(names[i], true, true, decode) == STREAM_THREAD_NICE);
        }
    }
    assert(streamThreadNice("VideoPing", true, true, true) == 0);
}

static void streamPriorityOffRaisesNothing(void) {
    const char* names[] = { "VideoRecv", "VideoDec", "AudioRecv", "AudioDec", "ControlRecv", "InputSend" };
    for (size_t i = 0; i < sizeof(names) / sizeof(names[0]); i++) {
        assert(streamThreadNice(names[i], false, true, true) == 0);
        assert(streamThreadNice(names[i], false, false, true) == 0);
    }
}

static void theSwitchOffRestoresTheOldLevels(void) {
    assert(!decodePriorityOn("0"));
    assert(decodePriorityOn(""));
    assert(decodePriorityOn("1"));
    // Before: VideoRecv at -4 even when it decodes, VideoDec left alone.
    assert(streamThreadNice("VideoRecv", true, true, false) == STREAM_THREAD_NICE);
    assert(streamThreadNice("VideoDec", true, false, false) == 0);
}

int main(void) {
    theDecodingThreadGetsTheDecodeLevel();
    theOtherStreamThreadsStayAtTheStreamLevel();
    streamPriorityOffRaisesNothing();
    theSwitchOffRestoresTheOldLevels();
    puts("stream_thread_priority: ok");
    return 0;
}
