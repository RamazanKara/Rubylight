#include "Limelight-internal.h"
#include "latency.h"
#include "stream_thread_priority.h"

#include <sys/resource.h>
#include <sys/system_properties.h>

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
    if (pthread_equal(thread, pthread_self()) && atomic_load(&AndroidNetworkPriority)) {
        int savedErrno = errno;
        char property[PROP_VALUE_MAX] = { 0 };
        __system_property_get("debug.nightjar.decode_priority", property);
        // VideoStream.c starts no VideoDec thread with direct submission: VideoRecv decodes.
        bool directSubmit = (VideoCallbacks.capabilities & CAPABILITY_DIRECT_SUBMIT) != 0;
        int nice = streamThreadNice(name, true, directSubmit, decodePriorityOn(property));
        if (nice != 0 && setpriority(PRIO_PROCESS, 0, nice) < 0 &&
                (nice == STREAM_THREAD_NICE || setpriority(PRIO_PROCESS, 0, STREAM_THREAD_NICE) < 0)) {
            Limelog("Streaming thread priority unavailable for %s: %d\n", name, errno);
        }
        errno = savedErrno;
    }
    return result;
}
