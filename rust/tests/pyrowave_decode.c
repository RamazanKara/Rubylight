/* Offline decoder for pyrowave_compatibility fixtures. No network or host API. */
#include <vulkan/vulkan.h>
#include <pyrowave/pyrowave.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

static uint32_t word(const uint8_t *p)
{
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) |
           ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

int main(int argc, char **argv)
{
    int result = 1;
    FILE *file = NULL;
    uint8_t *bytes = NULL;
    pyrowave_device device = NULL;
    pyrowave_decoder decoder = NULL;
    pyrowave_cpu_buffer output = {0};
#define REQUIRE(condition, message) do { \
    if (!(condition)) { fprintf(stderr, "%s\n", message); goto cleanup; } \
} while (0)
    REQUIRE(argc == 2, "usage: pyrowave-decode-probe FRAME_FILE");
    file = fopen(argv[1], "rb");
    REQUIRE(file, "cannot open frame file");
    REQUIRE(fseek(file, 0, SEEK_END) == 0, "cannot seek frame file");
    long length = ftell(file);
    REQUIRE(length >= 8 && length <= 64 * 1024 * 1024, "invalid frame file size");
    REQUIRE(fseek(file, 0, SEEK_SET) == 0, "cannot rewind frame file");
    size_t size = (size_t)length;
    bytes = malloc(size);
    REQUIRE(bytes, "cannot allocate frame input");
    REQUIRE(fread(bytes, 1, size, file) == size, "cannot read frame file");
    fclose(file);
    file = NULL;

    int records = (word(bytes) & UINT32_C(0x80000000)) != 0;
    size_t first = records ? 0 : 8;
    REQUIRE(size - first >= 8, "missing codec frame header");
    if (!records) {
        REQUIRE(word(bytes) > 0 && word(bytes) <= 65535, "invalid legacy packet count");
        REQUIRE(word(bytes + 4) >= 8 && word(bytes + 4) <= size - first,
                "invalid first legacy packet");
    }
    uint32_t header = word(bytes + first), metadata = word(bytes + first + 4);
    REQUIRE(header != UINT32_MAX && (header & UINT32_C(0x80000000)),
            "missing codec sequence header");
    REQUIRE(((metadata >> 24) & 3) == 0, "unsupported bitstream version");
    int width = (int)(header & 0x3fff) + 1;
    int height = (int)((header >> 14) & 0x3fff) + 1;
    int chroma = (int)((metadata >> 26) & 1);
    REQUIRE(width <= 8192 && height <= 8192 &&
            (chroma || ((width | height) & 1) == 0), "unsupported frame geometry");
    REQUIRE(pyrowave_create_default_device(&device) == PYROWAVE_SUCCESS,
            "cannot create Vulkan decoder device");
    pyrowave_decoder_create_info info = {
        .device = device, .width = width, .height = height,
        .chroma = chroma, .fragment_path = false
    };
    REQUIRE(pyrowave_decoder_create(&info, &decoder) == PYROWAVE_SUCCESS,
            "cannot create codec decoder");
    size_t offset = records ? 0 : 4;
    uint32_t packets = 0;
    while (offset < size) {
        REQUIRE(size - offset >= 8, "truncated packet header");
        uint32_t tag = word(bytes + offset);
        size_t count;
        int padding = records && tag == UINT32_MAX;
        if (!records) {
            count = tag;
            offset += 4;
        } else if (padding) {
            count = 8 + (size_t)word(bytes + offset + 4) * 4;
        } else {
            count = tag & UINT32_C(0x80000000) ? 8 : ((tag >> 16) & 0xfff) * 4;
        }
        REQUIRE(count >= 8 && count % 4 == 0 && count <= size - offset,
                "invalid codec packet length");
        if (!padding) {
            REQUIRE(pyrowave_decoder_push_packet(decoder, bytes + offset, count) == PYROWAVE_SUCCESS,
                    "codec rejected a packet");
            packets++;
        }
        offset += count;
    }
    REQUIRE(records || packets == word(bytes), "legacy packet count mismatch");
    REQUIRE(pyrowave_decoder_decode_is_ready(decoder, false), "codec frame is incomplete");
    output.width = width;
    output.height = height;
    /* The SDK CPU readback is eight-bit. Native GPU tests validate full HDR precision. */
    output.format = chroma ? PYROWAVE_CPU_BUFFER_FORMAT_YUV444P : PYROWAVE_CPU_BUFFER_FORMAT_YUV420P;
    for (int plane = 0; plane < 3; plane++) {
        int divisor = plane && !chroma ? 2 : 1;
        output.row_stride_in_bytes[plane] = (size_t)width / divisor;
        output.plane_size_in_bytes[plane] = output.row_stride_in_bytes[plane] * ((size_t)height / divisor);
        output.data[plane] = calloc(1, output.plane_size_in_bytes[plane]);
        REQUIRE(output.data[plane], "cannot allocate decoded plane");
    }
    REQUIRE(pyrowave_decoder_decode_cpu_buffer_synchronous(decoder, &output) == PYROWAVE_SUCCESS,
            "codec decode failed");
    for (int plane = 0; plane < 3; plane++) {
        const uint8_t *pixels = output.data[plane];
        unsigned low = 255, high = 0;
        for (size_t i = 0; i < output.plane_size_in_bytes[plane]; i++) {
            if (pixels[i] < low) low = pixels[i];
            if (pixels[i] > high) high = pixels[i];
        }
        REQUIRE(high > low + 16, "fixture decoded to an empty or constant plane");
    }
    printf("PYROWAVE DECODE PASS width=%d height=%d chroma=%d packets=%u readback_bits=8\n",
           width, height, chroma ? 444 : 420, packets);
    result = 0;
cleanup:
    if (decoder) pyrowave_decoder_destroy(decoder);
    if (device) pyrowave_device_destroy(device);
    for (int plane = 0; plane < 3; plane++) free(output.data[plane]);
    if (file) fclose(file);
    free(bytes);
    return result;
}
