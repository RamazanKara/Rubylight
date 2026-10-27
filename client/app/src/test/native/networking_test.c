typedef int SOCKET;
typedef _Bool bool;
#define true 1
#define false 0
#define AF_INET 2
#define AF_INET6 10
#define TCPv4_MSS 536
#define SOCKET_ERROR -1
#define EINTR 4
#define EPIPE 32
struct sockaddr_storage { int ss_family; };
struct sockaddr_in { int sin_family; };
struct sockaddr_in6 { int sin6_family; struct { unsigned char s6_addr[16]; } sin6_addr; };
static bool isPrivateNetworkAddressV4(struct sockaddr_in* address, bool cgn) { return false; }
static int socketError;
#define LastSocketError() socketError
#define SetLastSocketError(value) (socketError = (value))
static int received, calls, mode, wrongData, oversized;
static char expected[1200];
static struct sockaddr_in6 address;
static int send(SOCKET socket, const char* data, int length, int flags) {
    calls++;
    if (mode == 1 && calls == 1) { socketError = EINTR; return -1; }
    if (mode == 2) return 0;
    if (mode == 3) { socketError = 123; return -1; }
    if (length > TCPv4_MSS) oversized = 1;
    int count = length < 97 ? length : 97;
    for (int i = 0; i < count; i++) {
        if (data[i] != expected[received++]) wrongData = 1;
    }
    return count;
}
#include "../../main/jni/moonlight-core/android_sockets.h"
static void reset(int nextMode) {
    received = calls = wrongData = oversized = socketError = 0;
    mode = nextMode;
}
int main(void) {
    int failures = 0;
    address.sin6_family = AF_INET6;
    address.sin6_addr.s6_addr[0] = 0xfd;
    if (!isPrivateNetworkAddress((struct sockaddr_storage*)&address)) failures |= 1;
    address.sin6_addr.s6_addr[0] = 0xfe;
    address.sin6_addr.s6_addr[1] = 0x9b;
    if (!isPrivateNetworkAddress((struct sockaddr_storage*)&address)) failures |= 2;
    address.sin6_addr.s6_addr[1] = 0;
    if (isPrivateNetworkAddress((struct sockaddr_storage*)&address)) failures |= 4;
    for (int i = 0; i < sizeof(expected); i++) expected[i] = (char)(i % 251);
    reset(0);
    if (sendMtuSafe(0, expected, sizeof(expected)) != sizeof(expected) ||
            received != sizeof(expected) || wrongData || oversized) failures |= 8;
    reset(1);
    if (sendMtuSafe(0, expected, sizeof(expected)) != sizeof(expected) ||
            received != sizeof(expected) || wrongData) failures |= 16;
    reset(2);
    if (sendMtuSafe(0, expected, sizeof(expected)) != -1 || socketError != EPIPE) failures |= 32;
    reset(3);
    if (sendMtuSafe(0, expected, sizeof(expected)) != -1 || socketError != 123) failures |= 64;
    reset(0);
    if (sendMtuSafe(0, expected, 0) != 0 || calls != 0) failures |= 128;
    return failures;
}
