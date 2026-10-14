#include <cassert>
#include <cstring>
#include <cstdio>
#include <vector>
#include <algorithm>
#include "../../main/jni/pyrowave-renderer/frame.h"
#include "../../main/jni/moonlight-core/pyrowave_protocol.h"
#include "../../../../protocol/ffi/include/rubylight_protocol.h"

static std::vector<std::vector<uint8_t>> recordPackets(unsigned sequence, uint8_t value) {
    // Host record_frame() at a 24-byte payload: padding fills the space before each 16-byte record.
    std::vector<std::vector<uint8_t>> packets(4, std::vector<uint8_t>(24));
    packets[0][0] = 1;
    packets[0][3] = 2;
    packets[0][4] = 16;
    pyroWriteLe32(packets[0].data() + 8, 0x80000000u | (sequence << 28) | 63 | (63 << 14));
    pyroWriteLe32(packets[0].data() + 12, 3);
    pyroWriteLe32(packets[0].data() + 16, UINT32_MAX);
    for (unsigned i = 1; i < 4; i++) {
        pyroWriteLe32(packets[i].data(), (sequence << 28) | (4 << 16));
        pyroWriteLe32(packets[i].data() + 4, ((i - 1) << 8) | value);
        if (i != 3) pyroWriteLe32(packets[i].data() + 16, UINT32_MAX);
    }
    return packets;
}

static std::vector<uint8_t> recordEnvelope(const std::vector<std::vector<uint8_t>>& packets, int lost = -1) {
    std::vector<uint8_t> out(4);
    int previous = -1;
    for (int i = 0; i < int(packets.size()); ++i) {
        if (i == lost) continue;
        uint32_t flags = PYRO_PACKET_RECORD_START;
        if (i == 0) flags |= PYRO_PACKET_FRAME_START;
        if (i == int(packets.size()) - 1) flags |= PYRO_PACKET_FRAME_END;
        if (previous >= 0 && previous == i - 1) flags |= PYRO_PACKET_CONTIGUOUS;
        size_t at = out.size();
        out.resize(at + 8);
        pyroWriteLe32(out.data() + at, uint32_t(packets[i].size()));
        pyroWriteLe32(out.data() + at + 4, flags);
        out.insert(out.end(), packets[i].begin(), packets[i].end());
        previous = i;
    }
    return out;
}

static void testRecords() {
    PyroWaveRecords state;
    std::vector<std::vector<uint8_t>> decoded;
    auto push = [&decoded](const uint8_t *data, size_t length) {
        decoded.emplace_back(data, data + length);
        return true;
    };
    auto decode = [&](const std::vector<uint8_t>& frame) {
        decoded.clear();
        bool result = state.pushFrame(frame.data(), frame.size(), push);
        if (result) std::sort(decoded.begin() + 1, decoded.end(), [](const auto& a, const auto& b) {
            return (pyroReadLe32(a.data() + 4) >> 8) < (pyroReadLe32(b.data() + 4) >> 8);
        });
        return result;
    };
    auto full = recordEnvelope(recordPackets(7, 10));
    assert(decode(full));
    assert(decoded.size() == 4 && state.lossPercent == 0);
    assert(pyroReadLe32(decoded[0].data() + 4) == 3);
    for (unsigned i = 1; i < 4; ++i) assert(decoded[i][4] == 10);

    // A missing packet costs one record, and the cached block uses the new sequence after wrap.
    assert(decode(recordEnvelope(recordPackets(0, 20), 2)));
    assert(decoded.size() == 4 && state.lossPercent > 33 && state.lossPercent < 34);
    assert(decoded[1][4] == 20 && decoded[2][4] == 10 && decoded[3][4] == 20);
    for (const auto& block : decoded) assert(((block[3] >> 4) & 7) == 0);

    assert(decode(recordEnvelope(recordPackets(1, 30), 0)));
    assert(decoded.size() == 4 && state.lossPercent == -1);
    assert(decoded[1][4] == 30 && decoded[3][4] == 30);

    assert(decode(recordEnvelope(recordPackets(2, 40), 3)));
    assert(decoded[1][4] == 40 && decoded[3][4] == 30);

    // A partial first frame still submits its complete records without inventing missing blocks.
    PyroWaveRecords fresh;
    auto partial = recordEnvelope(recordPackets(0, 20), 2);
    decoded.clear();
    assert(fresh.pushFrame(partial.data(), partial.size(), push));
    assert(decoded.size() == 3 && pyroReadLe32(decoded[0].data() + 4) == 2);

    PyroWaveRecords negotiated(64, 64, false);
    auto noHeader = recordEnvelope(recordPackets(0, 20), 0);
    decoded.clear();
    assert(negotiated.pushFrame(noHeader.data(), noHeader.size(), push));
    assert(decoded.size() == 4);

    for (size_t length = 0; length < full.size(); ++length) {
        // Ending exactly between packets is a lost tail, which is recoverable.
        if (length >= 36 && (length - 4) % 32 == 0) continue;
        decoded.clear();
        assert(!state.pushFrame(full.data(), length, push));
        assert(decoded.empty());
    }

    auto packets = recordPackets(3, 50);
    packets[2][2] = 0;
    assert(!decode(recordEnvelope(packets)) && decoded.empty());
    packets[2][2] = 1;
    assert(!decode(recordEnvelope(packets)) && decoded.empty());
    packets[2][2] = 0xff;
    assert(!decode(recordEnvelope(packets)) && decoded.empty());

    packets = recordPackets(3, 50);
    packets[3][5] = 1;
    assert(!decode(recordEnvelope(packets)) && decoded.empty());

    for (int count : {0, 2, 4, 0xffffff}) {
        packets = recordPackets(3, 50);
        pyroWriteLe32(packets[0].data() + 12, count);
        assert(!decode(recordEnvelope(packets)) && decoded.empty());
    }
    packets = recordPackets(3, 50);
    pyroWriteLe32(packets[1].data() + 20, UINT32_MAX);
    assert(!decode(recordEnvelope(packets)) && decoded.empty());
    packets = recordPackets(3, 50);
    packets[2][3] ^= 0x10;
    assert(!decode(recordEnvelope(packets)) && decoded.empty());

    // A record spanning packets cannot be joined across a hole. Resume only at a marked boundary.
    packets = recordPackets(3, 50);
    pyroWriteLe32(packets[1].data(), (3u << 28) | (12 << 16));
    auto spanning = recordEnvelope(packets, 2);
    assert(decode(spanning));
    assert(decoded[1][4] == 40 && decoded[2][4] == 40 && decoded[3][4] == 50);

    // A good frame after loss replaces the cache, including intentional omissions by the encoder.
    packets = recordPackets(4, 60);
    packets.resize(2);
    pyroWriteLe32(packets[0].data() + 12, 1);
    assert(decode(recordEnvelope(packets)));
    assert(decoded.size() == 2 && decoded[1][4] == 60);

    full = recordEnvelope(recordPackets(5, 70));
    assert(!state.pushFrame(full.data(), full.size(), [](const uint8_t *, size_t) { return false; }));
    assert(state.lossPercent == 100);
    assert(decode(recordEnvelope(recordPackets(6, 80), 1)));
    assert(decoded[1][4] == 60 && decoded[2][4] == 80 && decoded[3][4] == 80);

    // Concealment must never reuse coefficients from a different geometry or chroma layout.
    for (bool changeChroma : {false, true}) {
        PyroWaveRecords changed;
        auto baseline = recordEnvelope(recordPackets(0, 10));
        assert(changed.pushFrame(baseline.data(), baseline.size(), push));
        auto resized = recordPackets(1, 20);
        if (changeChroma) resized[0][15] |= 4;
        else pyroWriteLe32(resized[0].data() + 8, 0x90000000u | 127 | (63 << 14));
        auto lossy = recordEnvelope(resized, 2);
        decoded.clear();
        assert(changed.pushFrame(lossy.data(), lossy.size(), push));
        assert(decoded.size() == 3 && pyroReadLe32(decoded[0].data() + 4) % (1u << 24) == 2);
        assert(decoded[1][4] == 20 && decoded[2][4] == 20);
    }
    std::puts("PyroWave records: passed (including truncation and count sweeps)");
}

int main() {
    testRecords();
    int calls = 0;
    auto push = [&calls](const uint8_t *, size_t) { calls++; return true; };
    // One sequence header and an empty eight-byte coefficient block in a single packet.
    std::vector<uint8_t> frame = {1,0,0,0, 16,0,0,0, 63,0,0,128, 1,0,0,0, 0,0,2,0, 0,0,0,0};
    assert(pushPyroWaveFrame(frame.data(), frame.size(), push));
    assert(calls == 1);
    calls = 0;
    for (size_t n = 0; n < frame.size(); ++n) {
        assert(!pushPyroWaveFrame(frame.data(), n, push));
    }
    auto bad = frame;
    bad[0] = 0;
    assert(!pushPyroWaveFrame(bad.data(), bad.size(), push));
    bad = frame;
    bad[0] = 255;
    assert(!pushPyroWaveFrame(bad.data(), bad.size(), push));
    bad = frame;
    bad[4] = 255;
    assert(!pushPyroWaveFrame(bad.data(), bad.size(), push));
    bad = frame;
    bad[18] = 0;
    assert(!pushPyroWaveFrame(bad.data(), bad.size(), push));
    bad[18] = 1;
    assert(!pushPyroWaveFrame(bad.data(), bad.size(), push));
    bad = frame;
    bad.push_back(0);
    assert(!pushPyroWaveFrame(bad.data(), bad.size(), push));
    assert(calls == 0);

    // A corrupt later packet must not feed even its valid predecessor to the decoder.
    auto twoPackets = frame;
    twoPackets[0] = 2;
    twoPackets.insert(twoPackets.end(), frame.begin() + 4, frame.end());
    for (size_t n = 0; n < twoPackets.size(); ++n) {
        assert(!pushPyroWaveFrame(twoPackets.data(), n, push));
        assert(calls == 0);
    }
    bad = twoPackets;
    bad[bad.size() - 6] = 0;
    assert(!pushPyroWaveFrame(bad.data(), bad.size(), push));
    assert(calls == 0);
    bad = twoPackets;
    bad[bad.size() - 5] = 0x10;
    assert(!pushPyroWaveFrame(bad.data(), bad.size(), push));
    assert(calls == 0);
    assert(pushPyroWaveFrame(twoPackets.data(), twoPackets.size(), push));
    assert(calls == 2);
    calls = 0;
    // The three-bit codec sequence may wrap; all blocks of one frame must agree.
    auto wrapped = frame;
    wrapped[11] |= 0x70;
    wrapped[19] |= 0x70;
    assert(pushPyroWaveFrame(wrapped.data(), wrapped.size(), push));
    assert(calls == 1);
    assert(!pushPyroWaveFrame(frame.data(), frame.size(), [](const uint8_t *, size_t) { return false; }));

    const char *sdp = "a=rtpmap:99 PYROWAVE/90000\r\na=x-ss-pyrowave.bitstream:186f0393\r\n";
    assert(selectPyroWaveFormat(0xf0000, 0x07800000, sdp) == VIDEO_FORMAT_PYROWAVE_MAIN10_444);
    const int profiles[] = {0x10000, 0x20000, 0x40000, 0x80000};
    for (int i = 0; i < 4; ++i) {
        assert(selectPyroWaveFormat(0xf0000, 0x800000u << i, sdp) == profiles[i]);
        assert(selectPyroWaveFormat(profiles[i], 0x07800000, sdp) == profiles[i]);
    }
    assert(selectPyroWaveFormat(0xf0000, 0, sdp) == 0);
    assert(selectPyroWaveFormat(0xf0000, 0x07800000, "PYROWAVE/90000") == 0);
    assert(selectPyroWaveFormat(0xf0000, 0x07800000, "AV1/90000") == 0);
    assert(selectPyroWaveFormat(0xf0000, 0x07800000,
        "PYROWAVE/90000\na=x-ss-pyrowave.bitstream:186f03930\n") == 0);
    assert(selectPyroWaveFormat(0xf0000, 0x07800000,
        "a=rtpmap:99 PYROWAVE/90000\na=x-ss-pyrowave.bitstream:186f0393") == VIDEO_FORMAT_PYROWAVE_MAIN10_444);
    assert(selectPyroWaveFormat(0xf0000, 0x07800000,
        "a=rtpmap:99 PYROWAVE/90000\na=x-ss-pyrowave.bitstream:186f03930\n") == 0);
    assert(selectPyroWaveFormat(0xf0000, 0x07800000,
        "a=rtpmap:99 PYROWAVE/90000\na=description:a=x-ss-pyrowave.bitstream:186f0393\n") == 0);
    assert(selectPyroWaveFormat(0xf0000, 0x07800000,
        "a=description:PYROWAVE/90000\na=x-ss-pyrowave.bitstream:186f0393\n") == 0);
    assert(selectPyroWaveFormat(0xf0000, 0x07800000,
        "a=rtpmap:99 OTHERPYROWAVE/90000\na=x-ss-pyrowave.bitstream:186f0393\n") == 0);
    assert(selectPyroWaveFormat(0xf0000, 0x07800000,
        "a=rtpmap:99 PYROWAVE/900001\na=x-ss-pyrowave.bitstream:186f0393\n") == 0);
    assert(selectPyroWaveFormat(0xf0000, 0x07800000,
        "a=rtpmap: PYROWAVE/90000\na=x-ss-pyrowave.bitstream:186f0393\n") == 0);
    std::puts("PyroWave ordinary framing and negotiation: passed");
}
