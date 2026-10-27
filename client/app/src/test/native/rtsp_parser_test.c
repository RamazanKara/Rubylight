typedef unsigned long long size_t;
typedef _Bool bool;
#define true 1
#define false 0
#define NULL ((void*)0)
#define LC_ASSERT(x) ((void)0)
static unsigned char heap[4096];
static size_t used;
static void* malloc(size_t size) {
    void* result = heap + used;
    used += (size + 7) & ~7;
    return used <= sizeof(heap) ? result : NULL;
}
static void free(void* pointer) { }
static void* memcpy(void* target, const void* source, size_t size) {
    for (size_t i = 0; i < size; i++) ((char*)target)[i] = ((const char*)source)[i];
    return target;
}
static size_t strlen(const char* text) {
    size_t length = 0;
    while (text[length]) length++;
    return length;
}
static int strcmp(const char* a, const char* b) {
    while (*a && *a == *b) { a++; b++; }
    return (unsigned char)*a - (unsigned char)*b;
}
static int strncmp(const char* a, const char* b, size_t count) {
    for (size_t i = 0; i < count; i++) {
        if (a[i] != b[i] || a[i] == 0) return (unsigned char)a[i] - (unsigned char)b[i];
    }
    return 0;
}
static bool delimiter(char c, const char* delimiters) {
    for (int i = 0; delimiters[i]; i++) if (c == delimiters[i]) return true;
    return false;
}
static char* strtok_r(char* text, const char* delimiters, char** context) {
    char* start = text != NULL ? text : *context;
    while (*start && delimiter(*start, delimiters)) start++;
    if (*start == 0) { *context = start; return NULL; }
    char* end = start;
    while (*end && !delimiter(*end, delimiters)) end++;
    *context = *end ? end + 1 : end;
    if (*end) *end = 0;
    return start;
}
static int atoi(const char* text) {
    int value = 0;
    while (*text >= '0' && *text <= '9') value = value * 10 + *text++ - '0';
    return value;
}
static int snprintf(char* target, size_t size, const char* format, ...) { return -1; }
static void* allocations[64];
static size_t allocationSizes[64];
static int outstanding;
static int outOfBounds;
static void* trackedMalloc(size_t size) {
    void* result = malloc(size);
    for (int i = 0; i < 64; i++) {
        if (allocations[i] == NULL) {
            allocations[i] = result;
            allocationSizes[i] = size;
            outstanding++;
            break;
        }
    }
    return result;
}
static void trackedFree(void* pointer) {
    if (pointer == NULL) return;
    for (int i = 0; i < 64; i++) {
        if (allocations[i] == pointer) {
            allocations[i] = NULL;
            outstanding--;
            break;
        }
    }
    free(pointer);
}
static int checkedStrncmp(const char* a, const char* b, size_t count) {
    for (int i = 0; i < 64; i++) {
        if (allocations[i] != NULL && a == (char*)allocations[i] + allocationSizes[i]) {
            outOfBounds++;
            return 1;
        }
    }
    return strncmp(a, b, count);
}
#define malloc trackedMalloc
#define free trackedFree
#define strncmp checkedStrncmp
#define TYPE_REQUEST 0
#define TYPE_RESPONSE 1

#define TOKEN_OPTION 0

#define RTSP_ERROR_SUCCESS 0
#define RTSP_ERROR_NO_MEMORY -1
#define RTSP_ERROR_MALFORMED -2

#define SEQ_INVALID -1

#define FLAG_ALLOCATED_OPTION_FIELDS 0x1
#define FLAG_ALLOCATED_MESSAGE_BUFFER 0x2
#define FLAG_ALLOCATED_OPTION_ITEMS 0x4
#define FLAG_ALLOCATED_PAYLOAD 0x8

#define CRLF_LENGTH 2
#define MESSAGE_END_LENGTH (2 + CRLF_LENGTH)

typedef struct _OPTION_ITEM {
    char flags;
    char* option;
    char* content;
    struct _OPTION_ITEM* next;
} OPTION_ITEM, *POPTION_ITEM;

// In this implementation, a flag indicates the message type:
// TYPE_REQUEST = 0
// TYPE_RESPONSE = 1
typedef struct _RTSP_MESSAGE {
    char type;
    char flags;
    int sequenceNumber;
    char* protocol;
    POPTION_ITEM options;
    char* payload;
    int payloadLength;

    char* messageBuffer;

    union {
        struct {
            // Request fields
            char* command;
            char* target;
        } request;
        struct {
            // Response fields
            char* statusString;
            int statusCode;
        } response;
    } message;
} RTSP_MESSAGE, *PRTSP_MESSAGE;

int parseRtspMessage(PRTSP_MESSAGE msg, char* rtspMessage, int length);
void freeMessage(PRTSP_MESSAGE msg);
void createRtspResponse(PRTSP_MESSAGE msg, char* messageBuffer, int flags, char* protocol, int statusCode, char* statusString, int sequenceNumber, POPTION_ITEM optionsHead, char* payload, int payloadLength);
void createRtspRequest(PRTSP_MESSAGE msg, char* messageBuffer, int flags, char* command, char* target, char* protocol, int sequenceNumber, POPTION_ITEM optionsHead, char* payload, int payloadLength);
char* getOptionContent(POPTION_ITEM optionsHead, char* option);
void insertOption(POPTION_ITEM* optionsHead, POPTION_ITEM opt);
void freeOptionList(POPTION_ITEM optionsHead);
char* serializeRtspMessage(PRTSP_MESSAGE msg, int* serializedLength);

#include "../../main/jni/moonlight-core/android_rtsp_parser.h"

int main(void) {
    int failures = 0;
    RTSP_MESSAGE message;
    char duplicate[] = "RTSP/1.0 200 OK\r\nCSeq: 1\r\nCSeq: 2\r\n\r\n";
    if (parseRtspMessage(&message, duplicate, sizeof(duplicate) - 1) != RTSP_ERROR_SUCCESS) return 128;
    if (message.sequenceNumber != 2) failures |= 1;
    freeMessage(&message);
    if (outstanding != 0) failures |= 2;
    char truncated[] = "RTSP/1.0 200 OK\r\nCSeq: 123";
    if (parseRtspMessage(&message, truncated, sizeof(truncated) - 1) != RTSP_ERROR_MALFORMED) failures |= 4;
    if (outOfBounds != 0) failures |= 8;
    return failures;
}