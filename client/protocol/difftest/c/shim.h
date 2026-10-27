// Test-only harness around Nightjar's patched moonlight-common-c receive paths.
#pragma once
#include <stddef.h>
#include <stdint.h>

typedef struct {
    int format;
    int packet_size;
    int app_version[4];
    int rfi;
    int records;
    int colorspace;
    int encrypted;
    uint8_t key[16];
    int queue_bound;
} dt_video_config;

typedef struct {
    int app_version[4];
    int packet_duration;
    int encrypted;
    uint8_t key[16];
    uint8_t iv[16];
} dt_audio_config;

// Event log shared by the video and audio harnesses.
const uint8_t* dt_log(size_t* length);
void dt_log_clear(void);

void dt_video_init(const dt_video_config* config);
void dt_video_destroy(void);
void dt_video_receive(const uint8_t* data, int length, uint64_t now_us);
// Completes every queued frame in order with a DR_* status, like the decoder thread.
void dt_video_complete_all(int status);
void dt_video_set_hdr(int active);
uint32_t dt_video_current_frame(void);
void dt_video_stats(uint32_t* video, uint32_t* fec);

void dt_audio_init(const dt_audio_config* config);
void dt_audio_destroy(void);
void dt_audio_receive(const uint8_t* data, int length, uint64_t now_us);
void dt_audio_timeout(void);
void dt_audio_stats(uint32_t out[7]);
