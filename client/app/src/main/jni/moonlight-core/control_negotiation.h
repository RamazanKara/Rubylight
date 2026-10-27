#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

// Rubylight control messages the host accepts, from its RTSP DESCRIBE answer
// (Rubylight 2.2.0+):
//   a=x-rl-control:0x5530,0x5531,0x5532
// Ids are 16-bit, hexadecimal with 0x or decimal; malformed entries are skipped.

#define RL_MAX_CONTROL_MESSAGES 32

// Stored per connection in android_control.c.
void setHostControlMessages(const uint16_t* ids, int count);
void resetHostControlMessages(void);
bool hostSupportsControlMessage(uint16_t messageType);

static inline int rlHexDigit(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

// Parses one id from [start, end); false when it is not a whole 16-bit number.
static inline bool rlParseControlId(const char* start, const char* end, uint16_t* id) {
    while (start < end && (*start == ' ' || *start == '\t')) start++;
    while (end > start && (end[-1] == ' ' || end[-1] == '\t')) end--;
    unsigned int base = 10;
    if (end - start > 2 && start[0] == '0' && (start[1] == 'x' || start[1] == 'X')) {
        base = 16;
        start += 2;
    }
    if (start == end) return false;
    uint32_t value = 0;
    for (; start < end; start++) {
        int digit = rlHexDigit(*start);
        if (digit < 0 || (unsigned int)digit >= base) return false;
        value = value * base + (uint32_t)digit;
        if (value > 0xFFFF) return false;
    }
    *id = (uint16_t)value;
    return true;
}

// Returns how many ids the SDP's x-rl-control lines name (written to ids, at most maxIds,
// duplicates dropped); 0 for hosts that send no such line.
static inline int rlParseControlMessages(const char* sdp, uint16_t* ids, int maxIds) {
    static const char prefix[] = "a=x-rl-control:";
    const size_t prefixLength = sizeof(prefix) - 1;
    int count = 0;
    if (sdp == NULL) return 0;
    for (const char* line = sdp; *line;) {
        const char* end = strchr(line, '\n');
        size_t length = end ? (size_t)(end - line) : strlen(line);
        if (length && line[length - 1] == '\r') length--;
        if (length > prefixLength && !memcmp(line, prefix, prefixLength)) {
            const char* item = line + prefixLength;
            const char* lineEnd = line + length;
            while (item < lineEnd) {
                const char* comma = memchr(item, ',', (size_t)(lineEnd - item));
                const char* itemEnd = comma ? comma : lineEnd;
                uint16_t id;
                if (rlParseControlId(item, itemEnd, &id)) {
                    bool seen = false;
                    for (int i = 0; i < count; i++) seen |= ids[i] == id;
                    if (!seen && count < maxIds) ids[count++] = id;
                }
                item = itemEnd + 1;
            }
        }
        if (!end) break;
        line = end + 1;
    }
    return count;
}
