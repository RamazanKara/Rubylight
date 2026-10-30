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

// The first `a=<attribute>:` line of a DESCRIBE answer as a 7.1.4 layout, read as the Rust
// client's audio_ext::Layout::parse reads it: channels, streams, coupled streams and 12 mapping
// entries, comma-separated (`12,7,5,0,1,10,11,2,3,4,5,6,7,8,9`). 1 when the line is there and
// valid, 0 when there is none, negative when it is malformed.
static int parseSurround714Layout(const char* payload, const char* attribute, POPUS_MULTISTREAM_CONFIGURATION opusConfig) {
    char needle[64];
    const char* p;
    int numbers[15];
    int i;

    snprintf(needle, sizeof(needle), "a=%s:", attribute);
    // Only at the start of a line.
    p = strstr(payload, needle);
    while (p && p != payload && p[-1] != '\n') {
        p = strstr(p + 1, needle);
    }
    if (!p) {
        return 0;
    }
    p += strlen(needle);
    while (*p == ' ' || *p == '\t') {
        p++;
    }
    for (i = 0; i < 15; i++) {
        int digits = 0;

        if (i > 0 && *p++ != ',') {
            Limelog("%s has too few values\n", attribute);
            return -1;
        }
        numbers[i] = 0;
        for (; SURROUND_PARAMS_IS_DIGIT(*p) && digits < 4; p++, digits++) {
            numbers[i] = numbers[i] * 10 + (*p - '0');
        }
        if (digits == 0 || digits > 3 || numbers[i] > 255) {
            Limelog("Invalid value %d in %s\n", i, attribute);
            return -1;
        }
    }
    while (*p == ' ' || *p == '\t') {
        p++;
    }
    if (*p != '\0' && *p != '\r' && *p != '\n') {
        Limelog("%s has trailing data\n", attribute);
        return -1;
    }
    if (numbers[0] != 12 || numbers[2] > numbers[1] || numbers[1] + numbers[2] != 12) {
        Limelog("%s is not a 7.1.4 layout\n", attribute);
        return -1;
    }
    for (i = 0; i < 12; i++) {
        if (numbers[3 + i] >= 12) {
            Limelog("%s maps past 12 channels\n", attribute);
            return -1;
        }
    }

    opusConfig->channelCount = 12;
    opusConfig->streams = numbers[1];
    opusConfig->coupledStreams = numbers[2];
    for (i = 0; i < 12; i++) {
        opusConfig->mapping[i] = (unsigned char)numbers[3 + i];
    }
    return 1;
}

// 7.1.4 from a DESCRIBE answer without surround-params for it. A Rubylight 3.0 rc (rc.4, rc.5)
// names its layouts only in `a=x-rl-audio.layout714` and `layout714Hq` (7 streams, 5 coupled);
// with neither, V+'s fallback. 0, or negative when an x-rl line is malformed.
static int setSurround714FromDescribe(const char* payload, POPUS_MULTISTREAM_CONFIGURATION normal,
                                      POPUS_MULTISTREAM_CONFIGURATION highQuality, bool* highQualitySupported) {
    int found;

    *highQualitySupported = false;
    found = parseSurround714Layout(payload, "x-rl-audio.layout714", normal);
    if (found < 0) {
        return found;
    }
    if (found == 0) {
        setSurround714Fallback(normal);
        return 0;
    }
    found = parseSurround714Layout(payload, "x-rl-audio.layout714Hq", highQuality);
    if (found < 0) {
        return found;
    }
    *highQualitySupported = found == 1;
    return 0;
}
