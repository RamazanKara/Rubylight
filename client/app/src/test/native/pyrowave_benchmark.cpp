// Uses the MIT upstream C API to generate/replay synthetic frames on Windows or Android.
#include <vulkan/vulkan.h>
#include <pyrowave/pyrowave.h>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <thread>
#include <vector>
#include "../../main/jni/pyrowave-renderer/frame.h"

static void checked(pyrowave_result result) {
    if (result != PYROWAVE_SUCCESS) {
        std::fprintf(stderr, "PyroWave error %d\n", result);
        std::exit(1);
    }
}

static uint64_t nowNs() {
    return std::chrono::duration_cast<std::chrono::nanoseconds>(
        std::chrono::steady_clock::now().time_since_epoch()).count();
}

static void appendWord(std::vector<uint8_t> &bytes, uint32_t word) {
    for (int i = 0; i < 4; ++i) bytes.push_back(uint8_t(word >> (8 * i)));
}

int main() {
    constexpr int width = 1280, height = 720;
    pyrowave_device device = nullptr;
    checked(pyrowave_create_default_device(&device));
    std::puts("chroma,path,frame,input_ns,output_ns");
    for (bool fullChroma : {false, true}) {
        pyrowave_encoder_create_info info = {};
        info.device = device;
        info.width = width;
        info.height = height;
        info.chroma = fullChroma ? PYROWAVE_CHROMA_SUBSAMPLING_444 : PYROWAVE_CHROMA_SUBSAMPLING_420;
        pyrowave_encoder encoder;
        checked(pyrowave_encoder_create(&info, &encoder));
        std::vector<uint8_t> planes[3];
        pyrowave_cpu_buffer buffers = {};
        buffers.width = width;
        buffers.height = height;
        buffers.format = fullChroma ? PYROWAVE_CPU_BUFFER_FORMAT_YUV444P : PYROWAVE_CPU_BUFFER_FORMAT_YUV420P;
        for (int p = 0; p < 3; ++p) {
            int w = p == 0 || fullChroma ? width : width / 2;
            int h = p == 0 || fullChroma ? height : height / 2;
            planes[p].resize(w * h);
            for (int y = 0; y < h; ++y) {
                for (int x = 0; x < w; ++x) {
                    planes[p][y * w + x] = p == 0 ? 16 + 219 * x / (w - 1) : 64 + 128 * ((x / 8 + y / 8 + p) & 1);
                }
            }
            buffers.data[p] = planes[p].data();
            buffers.row_stride_in_bytes[p] = w;
            buffers.plane_size_in_bytes[p] = planes[p].size();
        }
        const pyrowave_rate_control rate = {500000};
        checked(pyrowave_encoder_encode_cpu_synchronous(encoder, &buffers, &rate));
        size_t count = 0;
        checked(pyrowave_encoder_compute_num_packets(encoder, 1024, &count));
        std::vector<pyrowave_packet> packets(count);
        std::vector<uint8_t> bitstream(rate.maximum_bitstream_size), container;
        checked(pyrowave_encoder_packetize(encoder, packets.data(), 1024, &count, bitstream.data(), bitstream.size()));
        appendWord(container, uint32_t(count));
        for (size_t i = 0; i < count; ++i) {
            appendWord(container, uint32_t(packets[i].size));
            container.insert(container.end(), bitstream.begin() + packets[i].offset,
                             bitstream.begin() + packets[i].offset + packets[i].size);
        }
        const char *name = fullChroma ? "synthetic-720p-444.pyrw" : "synthetic-720p-420.pyrw";
        FILE *fixture = std::fopen(name, "wb");
        if (!fixture || std::fwrite(container.data(), 1, container.size(), fixture) != container.size()) return 1;
        std::fclose(fixture);
        pyrowave_encoder_destroy(encoder);
        for (bool fragment : {false, true}) {
            pyrowave_decoder_create_info decodeInfo = {};
            decodeInfo.device = device;
            decodeInfo.width = width;
            decodeInfo.height = height;
            decodeInfo.chroma = info.chroma;
            decodeInfo.fragment_path = fragment;
            checked(pyrowave_device_set_queue_type(device, fragment ? VK_QUEUE_GRAPHICS_BIT : VK_QUEUE_COMPUTE_BIT));
            pyrowave_decoder decoder;
            checked(pyrowave_decoder_create(&decodeInfo, &decoder));
            for (int i = -10; i < 60; ++i) {
                pyrowave_decoder_clear(decoder);
                uint64_t input = nowNs();
                bool valid = pushPyroWaveFrame(container.data(), container.size(), [decoder](const uint8_t *p, size_t n) {
                    return pyrowave_decoder_push_packet(decoder, p, n) == PYROWAVE_SUCCESS;
                });
                if (!valid || !pyrowave_decoder_decode_is_ready(decoder, false)) return 1;
                checked(pyrowave_decoder_decode_cpu_buffer_synchronous(decoder, &buffers));
                uint64_t output = nowNs();
                if (i >= 0) std::printf("%s,%s,%d,%llu,%llu\n", fullChroma ? "444" : "420",
                    fragment ? "fragment" : "compute", i, (unsigned long long)input, (unsigned long long)output);
                // Keep the shared GPU workload below a 60 Hz stream.
                std::this_thread::sleep_for(std::chrono::milliseconds(17));
            }
            pyrowave_decoder_destroy(decoder);
        }
    }
    pyrowave_device_destroy(device);
}
