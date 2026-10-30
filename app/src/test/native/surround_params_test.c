// The Opus layouts a DESCRIBE answer names: GFE's single digits, and 7.1.4 in the comma form
// Foundation Sunshine and Rubylight send, which Moonlight V+ reads.
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "Limelight.h"

#define Limelog(...) ((void) 0)
#include "android_surround_params.h"

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__, __LINE__, #x); exit(1); } } while (0)

static void checkLayout(const OPUS_MULTISTREAM_CONFIGURATION* config, int channels, int streams, int coupled,
                        const unsigned char* mapping) {
    CHECK(config->channelCount == channels);
    CHECK(config->streams == streams);
    CHECK(config->coupledStreams == coupled);
    CHECK(memcmp(config->mapping, mapping, (size_t) channels) == 0);
}

int main(void) {
    static const unsigned char inOrder[12] = {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11};
    OPUS_MULTISTREAM_CONFIGURATION config;

    // 7.1.4 after the prefix `a=fmtp:97 surround-params=12`, as Foundation Sunshine sends it.
    memset(&config, 0, sizeof(config));
    CHECK(parseOpusConfigFromParamString(",8,4,0,1,2,3,4,5,6,7,8,9,10,11\r\n", 12, &config) == 0);
    checkLayout(&config, 12, 8, 4, inOrder);
    memset(&config, 0, sizeof(config));
    CHECK(parseOpusConfigFromParamString(",12,0,0,1,2,3,4,5,6,7,8,9,10,11\r\n", 12, &config) == 0);
    checkLayout(&config, 12, 12, 0, inOrder);
    // V+ takes ; : | as well.
    memset(&config, 0, sizeof(config));
    CHECK(parseOpusConfigFromParamString(";8;4;0;1;2;3;4;5;6;7;8;9;10;11", 12, &config) == 0);
    checkLayout(&config, 12, 8, 4, inOrder);

    // GFE's digits for 5.1 and 7.1 are read as before, and so is a delimited 7.1.
    static const unsigned char gfe51[6] = {0, 1, 4, 5, 2, 3};
    memset(&config, 0, sizeof(config));
    CHECK(parseOpusConfigFromParamString("42014523\r\n", 6, &config) == 0);
    checkLayout(&config, 6, 4, 2, gfe51);
    static const unsigned char gfe71[8] = {0, 1, 4, 5, 6, 7, 2, 3};
    memset(&config, 0, sizeof(config));
    CHECK(parseOpusConfigFromParamString("5301456723\r\n", 8, &config) == 0);
    checkLayout(&config, 8, 5, 3, gfe71);
    memset(&config, 0, sizeof(config));
    CHECK(parseOpusConfigFromParamString(",5,3,0,1,4,5,6,7,2,3\r\n", 8, &config) == 0);
    checkLayout(&config, 8, 5, 3, gfe71);

    // Short, non-numeric or out-of-range values are refused.
    CHECK(parseOpusConfigFromParamString(",8,4,0,1,2,3,4,5,6,7,8,9,10\r\n", 12, &config) != 0);
    CHECK(parseOpusConfigFromParamString(",8,4,0,1,2,3,4,5,6,7,8,9,10,x", 12, &config) != 0);
    CHECK(parseOpusConfigFromParamString(",8,4,0,1,2,3,4,5,6,7,8,9,10,256", 12, &config) != 0);
    CHECK(parseOpusConfigFromParamString(",8,4,0,1,2,3,4,5,6,7,8,9,10 11", 12, &config) != 0);
    CHECK(parseOpusConfigFromParamString("4201452", 6, &config) != 0);
    CHECK(parseOpusConfigFromParamString(",8,4", 13, &config) != 0);

    // A host that names no 7.1.4 layout: V+'s fallback, Foundation Sunshine's layout.
    memset(&config, 0, sizeof(config));
    setSurround714Fallback(&config);
    checkLayout(&config, 12, 8, 4, inOrder);

    // V+'s request: 12 channels, mask 0xF63F.
    CHECK(CHANNEL_COUNT_FROM_AUDIO_CONFIGURATION(AUDIO_CONFIGURATION_714_SURROUND) == 12);
    CHECK(CHANNEL_MASK_FROM_AUDIO_CONFIGURATION(AUDIO_CONFIGURATION_714_SURROUND) == 0xF63F);
    CHECK((unsigned) SURROUNDAUDIOINFO_FROM_AUDIO_CONFIGURATION(AUDIO_CONFIGURATION_714_SURROUND) == 0xF63F000Cu);

    puts("surround_params: passed");
    return 0;
}
