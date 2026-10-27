// Feeds the same random and corrupted PyroWave frames to the old C++ framing
// (frame_reference.h) and to rubylight-protocol through its C ABI, and fails on the first
// difference in result, pushed packets or loss. Run by protocol/tests/differential.sh.
#include <cstdio>
#include <cstdlib>
#include <random>

#include "frame_reference.h"
#include "rubylight_protocol.h"

namespace {

using Bytes = std::vector<uint8_t>;
std::mt19937 rng;

uint32_t uniform(uint32_t low, uint32_t high) {
    return std::uniform_int_distribution<uint32_t>(low, high)(rng);
}

void append32(Bytes &out, uint32_t value) {
    for (int i = 0; i < 4; ++i) out.push_back(uint8_t(value >> (8 * i)));
}

struct Format {
    uint32_t width, height;
    bool chroma444;
    uint32_t otherBits;  // Format bits 27-31 of the second header word.
};

Format randomFormat() {
    return {uniform(1, 4096), uniform(1, 4096), uniform(0, 1) == 1, uniform(0, 3) == 0 ? uniform(0, 31) : 0};
}

Bytes sequenceHeader(const Format &format, uint32_t sequence, uint32_t count) {
    Bytes out;
    append32(out, 0x80000000u | (sequence << 28) | (format.width - 1) | ((format.height - 1) << 14));
    append32(out, (format.otherBits << 27) | (uint32_t(format.chroma444) << 26) | count);
    return out;
}

Bytes record(uint32_t sequence, uint32_t index, uint32_t words) {
    Bytes out;
    append32(out, (sequence << 28) | (words << 16));
    append32(out, index << 8);
    for (uint32_t i = 2; i < words; ++i) append32(out, uint32_t(rng()) & 0x7fffffffu);
    return out;
}

struct Packet {
    uint32_t flags;
    Bytes payload;
};

// One frame as the host sends it: records in random order with padding, split into packets.
std::vector<Packet> hostFrame(const Format &format, uint32_t sequence, uint32_t count) {
    std::vector<uint32_t> order(count);
    for (uint32_t i = 0; i < count; ++i) order[i] = i;
    std::shuffle(order.begin(), order.end(), rng);
    Bytes body;
    std::vector<size_t> recordStarts;
    for (uint32_t index : order) {
        recordStarts.push_back(body.size());
        const Bytes block = record(sequence, index, uniform(2, 120));
        body.insert(body.end(), block.begin(), block.end());
        if (uniform(0, 5) == 0) {
            const uint32_t words = uniform(0, 20);
            append32(body, UINT32_MAX);
            append32(body, words);
            body.resize(body.size() + words * 4, 0);
        }
    }
    const size_t size = uniform(0, 3) == 0 ? 1400 : uniform(24, 400);
    std::vector<Packet> packets;
    for (size_t at = 0; at < body.size() || packets.empty(); at += size) {
        const size_t end = std::min(body.size(), at + size);
        Packet packet{0, Bytes(body.begin() + at, body.begin() + end)};
        // The host marks packets whose payload starts at a record boundary.
        for (size_t start : recordStarts) packet.flags |= start == at ? PYRO_PACKET_RECORD_START : 0;
        packets.push_back(packet);
    }
    const Bytes header = sequenceHeader(format, sequence, count);
    const size_t last = packets.back().payload.size() + (packets.size() == 1 ? 16 : 0);
    Bytes prefix = {1, 0, 0, 0, uint8_t(last), uint8_t(last >> 8), 0, 0};
    prefix.insert(prefix.end(), header.begin(), header.end());
    packets.front().payload.insert(packets.front().payload.begin(), prefix.begin(), prefix.end());
    packets.front().flags |= PYRO_PACKET_FRAME_START | PYRO_PACKET_RECORD_START;
    packets.back().flags |= PYRO_PACKET_FRAME_END;
    if (uniform(0, 3) == 0) packets.back().payload.resize(packets.back().payload.size() + uniform(1, 40));
    return packets;
}

// The client's container for the packets that survived.
Bytes recordContainer(const std::vector<Packet> &packets, double lossRate) {
    Bytes out(4, 0);
    long previous = -2;
    for (size_t i = 0; i < packets.size(); ++i) {
        if (std::bernoulli_distribution(lossRate)(rng)) continue;
        uint32_t flags = packets[i].flags;
        if (previous == long(i) - 1) flags |= PYRO_PACKET_CONTIGUOUS;
        append32(out, uint32_t(packets[i].payload.size()));
        append32(out, flags);
        out.insert(out.end(), packets[i].payload.begin(), packets[i].payload.end());
        previous = long(i);
    }
    return out;
}

Bytes ordinaryContainer(const Format &format, uint32_t sequence) {
    const uint32_t count = uniform(1, 6);
    Bytes out;
    append32(out, count);
    uint32_t index = 0;
    for (uint32_t i = 0; i < count; ++i) {
        Bytes packet = i == 0 ? sequenceHeader(format, sequence, 0) : Bytes();
        for (uint32_t blocks = uniform(1, 4); blocks; --blocks) {
            const Bytes block = record(sequence, index++, uniform(2, 50));
            packet.insert(packet.end(), block.begin(), block.end());
        }
        append32(out, uint32_t(packet.size()));
        out.insert(out.end(), packet.begin(), packet.end());
    }
    return out;
}

void corrupt(Bytes &data) {
    switch (uniform(0, 5)) {
    case 0:
        for (uint32_t flips = uniform(1, 4); flips && !data.empty(); --flips)
            data[uniform(0, uint32_t(data.size() - 1))] ^= uint8_t(1u << uniform(0, 7));
        break;
    case 1:
        if (!data.empty()) data.resize(uniform(0, uint32_t(data.size() - 1)));
        break;
    case 2:
        data.push_back(uint8_t(rng()));
        break;
    case 3:
        if (data.size() >= 8) data[uniform(0, uint32_t(data.size() / 4 - 1)) * 4 + 3] ^= 0x70;
        break;
    case 4:
        if (data.size() >= 4) {
            const size_t at = uniform(0, uint32_t(data.size() / 4 - 1)) * 4;
            for (int i = 0; i < 4; ++i) data[at + i] = uint8_t(rng());
        }
        break;
    default:
        break;
    }
}

struct Outcome {
    bool ok;
    std::vector<Bytes> pushed;
};

template <typename Records>
Outcome viaRecords(Records &records, const Bytes &data, uint32_t refuseAfter) {
    Outcome outcome;
    outcome.ok = records.pushFrame(data.data(), data.size(), [&](const uint8_t *packet, size_t size) {
        outcome.pushed.emplace_back(packet, packet + size);
        return outcome.pushed.size() <= refuseAfter;
    });
    return outcome;
}

template <typename Function>
Outcome viaContainer(Function function, const Bytes &data, uint32_t refuseAfter) {
    Outcome outcome;
    outcome.ok = function(data.data(), data.size(), [&](const uint8_t *packet, size_t size) {
        outcome.pushed.emplace_back(packet, packet + size);
        return outcome.pushed.size() <= refuseAfter;
    });
    return outcome;
}

void fail(const char *what, unsigned seed, int stream, int frame) {
    std::fprintf(stderr, "difference in %s (seed %u, stream %d, frame %d)\n", what, seed, stream, frame);
    std::exit(1);
}

}  // namespace

int main(int argc, char **argv) {
    const unsigned seed = argc > 1 ? unsigned(std::strtoul(argv[1], nullptr, 10)) : 1;
    const int streams = argc > 2 ? std::atoi(argv[2]) : 2000;
    rng.seed(seed);
    long frames = 0, accepted = 0, replayed = 0;
    for (int stream = 0; stream < streams; ++stream) {
        Format format = randomFormat();
        const bool negotiated = uniform(0, 3) != 0;
        reference::PyroWaveRecords expected = negotiated ?
                reference::PyroWaveRecords(format.width, format.height, format.chroma444) : reference::PyroWaveRecords();
        PyroWaveRecords actual = negotiated ? PyroWaveRecords(format.width, format.height, format.chroma444) : PyroWaveRecords();
        const double lossRate = std::uniform_real_distribution<double>(0, 0.4)(rng);
        uint32_t sequence = uniform(0, 7);
        for (int frame = 0; frame < 40; ++frame, ++frames) {
            // Changes of geometry, or of the format bits alone.
            if (uniform(0, 30) == 0) format = randomFormat();
            if (uniform(0, 30) == 0) format.chroma444 = !format.chroma444;
            if (uniform(0, 30) == 0) format.otherBits = uniform(0, 31);
            sequence = (sequence + 1) & 7;
            const uint32_t refuseAfter = uniform(0, 20) == 0 ? uniform(0, 5) : UINT32_MAX;
            if (uniform(0, 9) == 0) {
                Bytes data = ordinaryContainer(format, sequence);
                if (uniform(0, 2) == 0) corrupt(data);
                const Outcome a = viaContainer([](const uint8_t *d, size_t l, auto push) {
                    return reference::pushPyroWaveFrame(d, l, push); }, data, refuseAfter);
                const Outcome b = viaContainer([](const uint8_t *d, size_t l, auto push) {
                    return pushPyroWaveFrame(d, l, push); }, data, refuseAfter);
                if (a.ok != b.ok) fail("container result", seed, stream, frame);
                if (a.pushed != b.pushed) fail("container packets", seed, stream, frame);
                continue;
            }
            Bytes data = recordContainer(hostFrame(format, sequence, uniform(0, 40)), lossRate);
            if (uniform(0, 4) == 0) corrupt(data);
            const Outcome a = viaRecords(expected, data, refuseAfter);
            const Outcome b = viaRecords(actual, data, refuseAfter);
            if (a.ok != b.ok) fail("record result", seed, stream, frame);
            if (a.pushed != b.pushed) fail("record packets", seed, stream, frame);
            if (expected.lossPercent != actual.lossPercent) fail("loss", seed, stream, frame);
            accepted += a.ok;
            replayed += a.ok && expected.lossPercent > 0;
        }
    }
    std::printf("seed %u: %ld frames identical (%ld accepted, %ld with lost records)\n",
                seed, frames, accepted, replayed);
    return 0;
}
