#pragma once

#include <stdint.h>

typedef struct AAudioStream AAudioStream;
typedef struct AAudioStreamBuilder AAudioStreamBuilder;
typedef int32_t aaudio_result_t;
typedef int32_t aaudio_direction_t;
typedef int32_t aaudio_sharing_mode_t;
typedef int32_t aaudio_performance_mode_t;
typedef int32_t aaudio_format_t;
typedef int32_t aaudio_usage_t;
typedef int32_t aaudio_data_callback_result_t;
typedef aaudio_data_callback_result_t (*AAudioStream_dataCallback)(AAudioStream*, void*, void*, int32_t);
typedef void (*AAudioStream_errorCallback)(AAudioStream*, void*, aaudio_result_t);

enum {
    AAUDIO_OK = 0,
    AAUDIO_DIRECTION_OUTPUT = 0,
    AAUDIO_SHARING_MODE_EXCLUSIVE = 0,
    AAUDIO_SHARING_MODE_SHARED = 1,
    AAUDIO_PERFORMANCE_MODE_LOW_LATENCY = 12,
    AAUDIO_FORMAT_PCM_I16 = 1,
    AAUDIO_USAGE_GAME = 14,
    AAUDIO_CALLBACK_RESULT_CONTINUE = 0,
    AAUDIO_CALLBACK_RESULT_STOP = 1,
};
