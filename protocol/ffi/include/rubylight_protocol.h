// C interface of rubylight-protocol (protocol/ffi/src/lib.rs), the wire formats Rubylight's
// host and clients share. Built as a static library by protocol/build-android.sh.
#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Receives one decoder packet; return false to stop and fail the frame.
typedef bool (*RpPushPacket)(void *user, const uint8_t *data, size_t length);

typedef struct RpPyroWaveRecords RpPyroWaveRecords;

// Record reassembly for a stream of the negotiated geometry; a zero width or height means none
// was negotiated.
RpPyroWaveRecords *rp_pyrowave_records_new(uint32_t width, uint32_t height, bool chroma444);
void rp_pyrowave_records_free(RpPyroWaveRecords *records);
// Feeds one record-framed frame to push, sequence header first; records the frame lost are
// replayed from earlier frames of the same format.
bool rp_pyrowave_records_push_frame(RpPyroWaveRecords *records, const uint8_t *data, size_t length,
                                    RpPushPacket push, void *user);
// Share of the last frame's records that were lost, 0-100 (100 after a rejected frame).
float rp_pyrowave_records_loss_percent(const RpPyroWaveRecords *records);

// Validates an ordinary (pre-record) container and feeds its packets to push.
bool rp_pyrowave_push_container(const uint8_t *data, size_t length, RpPushPacket push, void *user);

// Phase lock (core/src/phase_lock.rs): the client reports how early its frames are ready
// before the display latch, and the host times its frames to match.
#define RP_PHASE_REPORT_MESSAGE_TYPE 0x5530
#define RP_PHASE_REPORT_BYTES 16

typedef struct RpSlackWindow RpSlackWindow;

RpSlackWindow *rp_slack_window_new(void);
void rp_slack_window_free(RpSlackWindow *window);
// Records one shown frame: time from ready to the next latch deadline, and the refresh period.
void rp_slack_window_push(RpSlackWindow *window, int64_t slack_ns, int64_t period_ns);
size_t rp_slack_window_len(const RpSlackWindow *window);
// Writes the report message to out and clears the window; false below min_frames frames.
bool rp_slack_window_report(RpSlackWindow *window, size_t min_frames, uint8_t *out);

// Display HDR luminance (client to host, 2.2.0+). Luminance in nits, 0 when unknown.
#define RP_DISPLAY_CAPS_MESSAGE_TYPE 0x5531
#define RP_DISPLAY_CAPS_BYTES 16
void rp_display_caps_encode(bool hdr, float max_nits, float max_average_nits, float min_nits, uint8_t *out);

// Change resolution or frame rate mid-stream (client to host, 2.2.0+).
#define RP_RECONFIGURE_MESSAGE_TYPE 0x5532
#define RP_RECONFIGURE_BYTES 12
void rp_reconfigure_encode(uint16_t width, uint16_t height, uint32_t fps_millihz, uint8_t *out);

// Clock sync (both ways): the host answers each request with its RTP-clock receive and send
// times (core/src/control.rs, ClockSyncReply).
#define RP_CLOCK_SYNC_MESSAGE_TYPE 0x5537
#define RP_CLOCK_SYNC_REQUEST_BYTES 16
#define RP_CLOCK_SYNC_REPLY_BYTES 32
void rp_clock_sync_request_encode(uint32_t sequence, uint64_t client_us, uint8_t *out);

// Timestamp presentation for VRR displays (core/src/present_clock.rs): each frame is shown at
// its host present time plus a delay that covers recent jitter, so the screen keeps the game's
// frame times. All times are client microseconds on one monotonic clock.
typedef struct RpPresentSettings {
    uint32_t window;         // frames whose lateness sets the delay, up to 512
    uint8_t percentile;      // share of frames that should be ready in time, percent
    int64_t margin_us;       // added for the present call itself
    int64_t decay_us;        // how fast the delay may fall per frame
    int64_t max_hold_us;     // longest a ready frame is held
    int64_t min_interval_us; // the display's shortest refresh interval
} RpPresentSettings;

typedef struct RpPresentSchedule {
    uint64_t present_us; // present at this time; the ready time when late
    int64_t hold_us;     // present_us minus the ready time
    int64_t latency_us;  // host present to this present, -1 without clock sync
    bool late;
} RpPresentSchedule;

typedef struct RpPresentClock RpPresentClock;

void rp_present_settings_default(RpPresentSettings *out);
RpPresentClock *rp_present_clock_new(const RpPresentSettings *settings);
void rp_present_clock_free(RpPresentClock *clock);
// Feeds a 0x5537 reply received at client_receive_us; returns its round trip or -1.
int64_t rp_present_clock_sync(RpPresentClock *clock, const uint8_t *data, size_t length, uint64_t client_receive_us);
RpPresentSchedule rp_present_clock_schedule(RpPresentClock *clock, uint32_t rtp, uint64_t ready_us);
void rp_present_clock_reset(RpPresentClock *clock);
void rp_present_clock_stats(const RpPresentClock *clock, uint64_t *frames, uint64_t *late, int64_t *rtt_us);
// The offset (client = host + offset) at client time local_us and its exchange's round trip, in
// microseconds; false, writing nothing, before the first clock sync exchange.
bool rp_present_clock_offset(const RpPresentClock *clock, uint64_t local_us, int64_t *offset_us, int64_t *rtt_us);

#ifdef __cplusplus
}  // extern "C"

#include <memory>

// Calls a C++ callable through RpPushPacket.
template <typename Push>
inline bool rpPushThunk(void *user, const uint8_t *data, size_t length) {
    return (*static_cast<Push *>(user))(data, length);
}

// Owns RpPyroWaveRecords with the interface the renderer used before (frame.h's PyroWaveRecords).
class PyroWaveRecords {
public:
    PyroWaveRecords() : PyroWaveRecords(0, 0, false) {}
    PyroWaveRecords(uint32_t width, uint32_t height, bool chroma444)
        : state(rp_pyrowave_records_new(width, height, chroma444)) {}

    template <typename Push>
    bool pushFrame(const uint8_t *data, size_t length, Push push) {
        const bool ok = state && rp_pyrowave_records_push_frame(state.get(), data, length, &rpPushThunk<Push>, &push);
        lossPercent = state ? rp_pyrowave_records_loss_percent(state.get()) : 100;
        return ok;
    }

    float lossPercent = 0;

private:
    struct Free {
        void operator()(RpPyroWaveRecords *records) const { rp_pyrowave_records_free(records); }
    };
    std::unique_ptr<RpPyroWaveRecords, Free> state;
};

template <typename Push>
bool pushPyroWaveFrame(const uint8_t *data, size_t length, Push push) {
    return rp_pyrowave_push_container(data, length, &rpPushThunk<Push>, &push);
}
#endif
