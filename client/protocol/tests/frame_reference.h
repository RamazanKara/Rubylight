// The C++ PyroWave framing the Android client used before rubylight-protocol, kept as the
// reference for protocol/tests/differential.cpp. Not built into the app.
#pragma once
#include "../../app/src/main/jni/pyrowave-renderer/frame.h"

#include <algorithm>
#include <array>
#include <map>
#include <vector>

namespace reference {

// Rubylight's packet container, after the core has removed the short header and FEC padding.
template <typename Push>
bool pushPyroWaveFrame(const uint8_t *data, size_t length, Push push) {
    if (length < 4) return false;
    const uint32_t count = pyroReadLe32(data);
    if (count == 0 || count > (length - 4) / 12) return false;
    size_t offset = 4;
    uint32_t sequence = UINT32_MAX;
    for (uint32_t i = 0; i < count; ++i) {
        if (length - offset < 4) return false;
        const uint32_t size = pyroReadLe32(data + offset);
        offset += 4;
        if (size < 8 || (size & 3) || size > length - offset) return false;
        // Reject short blocks before the decoder, including duplicated zero-length blocks.
        for (size_t at = 0; at < size;) {
            if (size - at < 8) return false;
            const uint32_t word = pyroReadLe32(data + offset + at);
            const size_t blockSize = word & 0x80000000u ? 8 : ((word >> 16) & 0xfff) * 4;
            if (word == UINT32_MAX || blockSize < 8 || blockSize > size - at) return false;
            const uint32_t blockSequence = (word >> 28) & 7;
            if (sequence != UINT32_MAX && sequence != blockSequence) return false;
            sequence = blockSequence;
            at += blockSize;
        }
        offset += size;
    }
    if (offset != length) return false;

    // Reject the whole container before any packet can change the decoder's sequence state.
    offset = 4;
    for (uint32_t i = 0; i < count; ++i) {
        const uint32_t size = pyroReadLe32(data + offset);
        offset += 4;
        if (!push(data + offset, size)) return false;
        offset += size;
    }
    return true;
}

class PyroWaveRecords {
public:
    PyroWaveRecords() = default;

    PyroWaveRecords(uint32_t width, uint32_t height, bool chroma444) : havePrevious(true) {
        // The negotiated geometry lets even the first frame survive a lost sequence-header packet.
        pyroWriteLe32(previousHeader.data(), 0x80000000u | (width - 1) | ((height - 1) << 14));
        pyroWriteLe32(previousHeader.data() + 4, uint32_t(chroma444) << 26);
    }

    float lossPercent = 0;

    template <typename Push>
    bool pushFrame(const uint8_t *data, size_t length, Push push) {
        lossPercent = 100;
        if (length < 4 || length > PYRO_MAX_FRAME_BYTES || pyroReadLe32(data) != 0) return false;
        std::vector<uint8_t> run;
        std::map<uint32_t, std::vector<uint8_t>> received;
        std::array<uint8_t, 8> header = {};
        bool hasHeader = false, partial = false, ended = false, collecting = false;
        uint32_t sequence = UINT32_MAX;
        uint32_t lastLength = 0;

        auto parseRun = [&](bool interrupted) {
            for (size_t at = 0; at < run.size();) {
                if (!hasHeader && std::all_of(run.begin() + at, run.end(),
                        [](uint8_t byte) { return byte == 0; })) return true;
                if (run.size() - at < 8) return interrupted;
                const uint32_t word = pyroReadLe32(run.data() + at);
                const bool padding = word == UINT32_MAX;
                const uint64_t size = padding ? 8ull + 4ull * pyroReadLe32(run.data() + at + 4) :
                        ((word >> 16) & 0xfff) * 4u;
                if (size < 8 || (!padding && (word & 0x80000000u))) return false;
                if (size > run.size() - at) return interrupted;
                if (!padding) {
                    const uint32_t seq = (word >> 28) & 7;
                    if (sequence != UINT32_MAX && sequence != seq) return false;
                    sequence = seq;
                    const uint32_t index = pyroReadLe32(run.data() + at + 4) >> 8;
                    if (!received.emplace(index, std::vector<uint8_t>(run.begin() + at,
                            run.begin() + at + size)).second) return false;
                }
                at += (size_t)size;
            }
            return true;
        };

        size_t offset = 4;
        while (offset < length) {
            if (ended || length - offset < 8) return false;
            size_t size = pyroReadLe32(data + offset);
            const uint32_t flags = pyroReadLe32(data + offset + 4);
            offset += 8;
            if (!size || size > 1400 || size > length - offset || (flags & ~15u)) return false;
            const uint8_t *packet = data + offset;
            offset += size;
            if (!(flags & PYRO_PACKET_CONTIGUOUS)) {
                if (!run.empty() && !parseRun(true)) return false;
                run.clear();
                collecting = (flags & (PYRO_PACKET_RECORD_START | PYRO_PACKET_FRAME_START)) != 0;
                if (!(flags & PYRO_PACKET_FRAME_START)) partial = true;
            }
            if (flags & PYRO_PACKET_FRAME_START) {
                if (hasHeader || offset != 12 + size || size < 16 || packet[0] != 1) return false;
                lastLength = packet[4] | (uint32_t(packet[5]) << 8);
                if (!lastLength || lastLength > 1400) return false;
                std::copy(packet + 8, packet + 16, header.begin());
                const uint32_t word = pyroReadLe32(header.data());
                if (!(word & 0x80000000u) || word == UINT32_MAX ||
                        ((pyroReadLe32(header.data() + 4) >> 24) & 3)) return false;
                sequence = (word >> 28) & 7;
                hasHeader = true;
            }
            if (flags & PYRO_PACKET_RECORD_START) collecting = true;
            if (flags & PYRO_PACKET_FRAME_END) {
                ended = true;
                if (hasHeader) {
                    if (lastLength > size) return false;
                    size = lastLength;
                }
            }
            if (flags & PYRO_PACKET_FRAME_START) {
                if (size < 16) return false;
                packet += 16;
                size -= 16;
            }
            if (collecting) run.insert(run.end(), packet, packet + size);
        }
        partial |= !ended || !hasHeader;
        if (!parseRun(!ended) || sequence == UINT32_MAX) return false;
        if (!hasHeader) {
            if (!havePrevious) return false;
            header = previousHeader;
            header[3] = uint8_t((header[3] & 0x8f) | (sequence << 4));
        }
        const uint32_t count = pyroReadLe32(header.data() + 4) & 0xffffff;
        if (hasHeader && (received.size() > count || (!partial && received.size() != count))) return false;
        if (received.empty() && count) return false;
        const float loss = !hasHeader ? -1 : (count ? 100.0f * (count - received.size()) / count : 0);

        const bool sameFormat = havePrevious &&
                (pyroReadLe32(previousHeader.data()) & ~0x70000000u) ==
                (pyroReadLe32(header.data()) & ~0x70000000u) &&
                (previousHeader[7] & 0xfc) == (header[7] & 0xfc);
        const bool retain = partial && sameFormat;
        size_t decodedCount = received.size();
        if (retain) {
            for (const auto &record : previous) decodedCount += received.count(record.first) == 0;
        }
        // The SDK zeros absent coefficients; replay cached blocks with this frame's sequence.
        const auto originalHeader = header;
        pyroWriteLe32(header.data() + 4, (pyroReadLe32(header.data() + 4) & 0xff000000u) |
                uint32_t(decodedCount));
        if (!push(header.data(), header.size())) return false;
        for (auto &record : received) {
            if (!push(record.second.data(), record.second.size())) return false;
        }
        if (retain) for (auto &record : previous) {
            if (received.count(record.first)) continue;
            record.second[3] = uint8_t((record.second[3] & 0x8f) | (sequence << 4));
            if (!push(record.second.data(), record.second.size())) return false;
        }
        if (!retain) previous.clear();
        for (auto &record : received) previous[record.first] = std::move(record.second);
        previousHeader = originalHeader;
        havePrevious = true;
        lossPercent = loss;
        return true;
    }

private:
    bool havePrevious = false;
    std::array<uint8_t, 8> previousHeader = {};
    std::map<uint32_t, std::vector<uint8_t>> previous;
};
}  // namespace reference
