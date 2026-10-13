#pragma once

// Uses the core's socket/clock helpers. The response belongs to the caller even on failure.
// A timeout covers the entire response; receiving another fragment must not restart it.
static bool receiveRtspResponse(SOCKET socket, char** response, int* length, int* error,
                                int timeoutMs, int maxResponseSize) {
    const uint64_t deadline = PltGetMillis() + timeoutMs;
    int capacity = 0;
    *response = NULL;
    *length = 0;
    for (;;) {
        if (ConnectionInterrupted) {
            *error = EINTR;
            return false;
        }
        uint64_t now = PltGetMillis();
        if (now >= deadline) {
            *error = ETIMEDOUT;
            return false;
        }
        struct pollfd pfd = {0};
        pfd.fd = socket;
        pfd.events = POLLIN;
        int waitMs = (int)(deadline - now);
        if (waitMs > 100) waitMs = 100;
        int ready = pollSockets(&pfd, 1, waitMs);
        if (ready < 0) {
            if (LastSocketError() == EINTR) continue;
            *error = LastSocketError();
            return false;
        }
        if (ready == 0) continue;
        // Check cancellation again before consuming an immediately readable socket.
        if (ConnectionInterrupted) {
            *error = EINTR;
            return false;
        }
        if (*length == capacity && capacity < maxResponseSize) {
            int nextCapacity = capacity + 16384;
            if (nextCapacity > maxResponseSize) nextCapacity = maxResponseSize;
            char* grown = realloc(*response, nextCapacity);
            if (grown == NULL) {
                *error = ENOMEM;
                return false;
            }
            *response = grown;
            capacity = nextCapacity;
        }
        // At the limit, permit EOF but reject even one additional byte.
        char excess;
        SOCK_RET received = recv(socket, *length == maxResponseSize ? &excess : *response + *length,
                                 *length == maxResponseSize ? 1 : capacity - *length, MSG_DONTWAIT);
        if (received < 0) {
            int receiveError = LastSocketError();
            if (receiveError == EINTR || receiveError == EAGAIN || receiveError == EWOULDBLOCK) continue;
            *error = receiveError;
            return false;
        }
        if (received == 0) return true;
        if (*length == maxResponseSize) {
            *error = EMSGSIZE;
            return false;
        }
        *length += received;
    }
}
