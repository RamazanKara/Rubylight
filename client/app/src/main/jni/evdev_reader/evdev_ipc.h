#pragma once

#include <stdbool.h>
#include <sys/socket.h>
#include <sys/types.h>

// Check before reading input devices or sending any input to a connected Unix socket.
static bool evdevPeerUidMatches(int fd, uid_t expectedUid) {
    struct ucred credentials;
    socklen_t length = sizeof(credentials);
    return getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &credentials, &length) == 0 &&
            length == sizeof(credentials) && credentials.uid == expectedUid;
}
