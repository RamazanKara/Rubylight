typedef unsigned long long size_t;
static unsigned short htons(unsigned short value) { return (value << 8) | (value >> 8); }
#define Limelog(...) ((void)0)
#define STUN_ATTRIBUTE_XOR_MAPPED_ADDRESS 0x0020
#include "../../main/jni/moonlight-core/android_stun.h"

int main(void) {
    unsigned int cookie = 0x42a41221;
    unsigned int expected = 0x010200c0;
    unsigned int encoded = expected ^ cookie;
    unsigned int address = 0;
    char padded[] = { (char)0x80, 0x22, 0, 1, 'a', 0, 0, 0,
                     0, 0x20, 0, 8, 0, 1, 0x12, 0x34, 0, 0, 0, 0 };
    for (int i = 0; i < 4; i++) padded[16 + i] = (char)(encoded >> (8 * i));
    int failures = 0;
    if (parseStunAttributes(padded, sizeof(padded), cookie, &address) != 0 || address != expected) failures |= 1;
    if (parseStunAttributes(padded + 8, 12, cookie, &address) != 0 || address != expected) failures |= 2;
    if (parseStunAttributes(padded, 7, cookie, &address) != -5) failures |= 4;
    return failures;
}