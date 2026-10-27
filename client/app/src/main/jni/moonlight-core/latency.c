#include "Limelight-internal.h"
#include "latency.h"

#include <sys/resource.h>

atomic_bool AndroidUnbatchedInput;
atomic_bool AndroidNetworkPriority;

int __real_socket(int domain, int type, int protocol);
int __real_pthread_setname_np(pthread_t thread, const char* name);

int __wrap_socket(int domain, int type, int protocol) {
    int socketFd = __real_socket(domain, type, protocol);
    if (socketFd >= 0 && (domain == AF_INET || domain == AF_INET6) &&
            atomic_load(&AndroidNetworkPriority)) {
        int savedErrno = errno;
        int priority = 6;
        // This is a local queue hint, not DSCP marking that a remote router might reject.
        // The core still applies its existing audio/video priorities after binding.
        if (setsockopt(socketFd, SOL_SOCKET, SO_PRIORITY, &priority, sizeof(priority)) < 0) {
            Limelog("Streaming socket priority unavailable: %d\n", errno);
        }
        errno = savedErrno;
    }
    return socketFd;
}

int __wrap_pthread_setname_np(pthread_t thread, const char* name) {
    int result = __real_pthread_setname_np(thread, name);
    // The core names each worker from inside that worker before it starts receiving/sending.
    if (pthread_equal(thread, pthread_self()) && atomic_load(&AndroidNetworkPriority) &&
            (!strcmp(name, "VideoRecv") || !strcmp(name, "AudioRecv") || !strcmp(name, "AudioDec") ||
             !strcmp(name, "ControlRecv") || !strcmp(name, "InputSend"))) {
        int savedErrno = errno;
        if (setpriority(PRIO_PROCESS, 0, -4) < 0) {
            Limelog("Streaming thread priority unavailable for %s: %d\n", name, errno);
        }
        errno = savedErrno;
    }
    return result;
}
