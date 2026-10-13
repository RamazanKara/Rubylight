#define _GNU_SOURCE
#include <assert.h>
#include <unistd.h>
#include "../../main/jni/evdev_reader/evdev_ipc.h"

int main(void) {
    int sockets[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);
    // Credentials come from the kernel, not data sent by the other socket endpoint.
    assert(evdevPeerUidMatches(sockets[0], geteuid()));
    assert(evdevPeerUidMatches(sockets[1], geteuid()));
    assert(!evdevPeerUidMatches(sockets[0], geteuid() == 0 ? 1 : 0));
    assert(!evdevPeerUidMatches(-1, geteuid()));
    close(sockets[0]);
    close(sockets[1]);
    return 0;
}
