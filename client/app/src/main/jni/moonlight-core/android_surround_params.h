#pragma once

// The Opus layouts in a DESCRIBE answer's `a=fmtp:97 surround-params=` values. Needs Limelight.h
// (OPUS_MULTISTREAM_CONFIGURATION) and Limelog.
//
// A value is the channel count (part of the prefix the caller matched), then streams, coupled
// streams and one mapping entry per channel. GFE and stock hosts write single digits; Foundation
// Sunshine and Rubylight write 7.1.4, whose mapping reaches 11, comma-separated
// (`12,8,4,0,1,...,11`). Moonlight V+'s moonlight-common-c (`parseOpusConfigFromDelimitedString`)
// reads that form for more than 8 channels or whenever a delimiter appears, and so does this.

// surround-714.patch makes room for 7.1.4's 12 channels.
_Static_assert(AUDIO_CONFIGURATION_MAX_CHANNEL_COUNT >= 12, "surround-714.patch is not applied");

#define SURROUND_PARAMS_IS_DIGIT(x) ((x) >= '0' && (x) <= '9')

static bool surroundParamsIsDelimiter(char c) {
    return c == ',' || c == ';' || c == ':' || c == '|';
}

static bool surroundParamsIsDelimited(const char* paramStr) {
    for (; *paramStr && *paramStr != ' ' && *paramStr != '\r' && *paramStr != '\n'; paramStr++) {
        if (surroundParamsIsDelimiter(*paramStr)) {
            return true;
        }
    }
    return false;
}

// The next number of a delimited value, skipping the delimiters before it.
static bool surroundParamsNextInt(const char** paramStr, int* value) {
    const char* p = *paramStr;
    int number = 0;
    int digits = 0;

    while (surroundParamsIsDelimiter(*p)) {
        p++;
    }
    for (; SURROUND_PARAMS_IS_DIGIT(*p); p++) {
        number = number * 10 + (*p - '0');
        if (++digits > 3) {
            return false;
        }
    }
    if (digits == 0) {
        return false;
    }
    *value = number;
    *paramStr = p;
    return true;
}

static int parseOpusConfigFromParamString(const char* paramStr, int channelCount, POPUS_MULTISTREAM_CONFIGURATION opusConfig) {
    int i;

    if (channelCount > AUDIO_CONFIGURATION_MAX_CHANNEL_COUNT) {
        Limelog("Invalid channel count: %d\n", channelCount);
        return -1;
    }

    // Set channel count (included in the prefix, so not parsed below)
    opusConfig->channelCount = channelCount;

    if (channelCount > 8 || surroundParamsIsDelimited(paramStr)) {
        int value;

        if (!surroundParamsNextInt(&paramStr, &value) || value > 255) {
            Limelog("Invalid stream count in delimited surround-params\n");
            return -1;
        }
        opusConfig->streams = value;
        if (!surroundParamsNextInt(&paramStr, &value) || value > 255) {
            Limelog("Invalid coupled stream count in delimited surround-params\n");
            return -2;
        }
        opusConfig->coupledStreams = value;
        for (i = 0; i < channelCount; i++) {
            if (!surroundParamsNextInt(&paramStr, &value) || value > 255) {
                Limelog("Invalid mapping value at %d in delimited surround-params\n", i);
                return -3;
            }
            opusConfig->mapping[i] = (unsigned char)value;
        }
        return 0;
    }

    // Parse the remaining data from the surround-params value
    if (!SURROUND_PARAMS_IS_DIGIT(*paramStr)) {
        Limelog("Invalid stream count: %c\n", *paramStr);
        return -1;
    }
    opusConfig->streams = *paramStr - '0';
    paramStr++;

    if (!SURROUND_PARAMS_IS_DIGIT(*paramStr)) {
        Limelog("Invalid coupled stream count: %c\n", *paramStr);
        return -2;
    }
    opusConfig->coupledStreams = *paramStr - '0';
    paramStr++;

    for (i = 0; i < opusConfig->channelCount; i++) {
        if (!SURROUND_PARAMS_IS_DIGIT(*paramStr)) {
            Limelog("Invalid mapping value at %d: %c\n", i, *paramStr);
            return -3;
        }

        opusConfig->mapping[i] = *paramStr - '0';
        paramStr++;
    }

    return 0;
}

// Moonlight V+'s layout for a 7.1.4 host that sends no parameters: Foundation Sunshine's
// 8 streams, 4 coupled (FL FR, FC LFE, BL BR, SL SR) and the heights mono, mapped in order.
static void setSurround714Fallback(POPUS_MULTISTREAM_CONFIGURATION opusConfig) {
    int i;

    opusConfig->channelCount = 12;
    opusConfig->streams = 8;
    opusConfig->coupledStreams = 4;
    for (i = 0; i < 12; i++) {
        opusConfig->mapping[i] = (unsigned char)i;
    }
}
