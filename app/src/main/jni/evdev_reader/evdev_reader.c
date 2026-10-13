#define _GNU_SOURCE
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <stdatomic.h>
#include <stddef.h>

#include <sys/types.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <fcntl.h>
#include <linux/input.h>
#include <unistd.h>
#include <poll.h>
#include <errno.h>
#include <dirent.h>
#include <pthread.h>
#include <android/log.h>
#include "evdev_ipc.h"

#define EVDEV_MAX_EVENT_SIZE 24

#define REL_X 0x00
#define REL_Y 0x01
#define KEY_Q 16
#define BTN_LEFT 0x110
#define BTN_GAMEPAD 0x130

struct DeviceEntry {
    struct DeviceEntry *next;
    pthread_t thread;
    int fd;
    char devName[128];
};

static struct DeviceEntry *DeviceListHead;
static atomic_int grabbing = 1;
static pthread_mutex_t DeviceListLock = PTHREAD_MUTEX_INITIALIZER;
static pthread_mutex_t SocketSendLock = PTHREAD_MUTEX_INITIALIZER;
static int sock;

// This is a small executable that runs in a root shell. It reads input
// devices and writes the evdev output packets to a socket. This allows
// Moonlight to read input devices without having to muck with changing
// device permissions or modifying SELinux policy (which is prevented in
// Marshmallow anyway).

#define test_bit(bit, array)    (array[bit/8] & (1<<(bit%8)))

static int hasRelAxis(int fd, short axis) {
    unsigned char relBitmask[(REL_MAX + 1) / 8] = {0};

    if (ioctl(fd, EVIOCGBIT(EV_REL, sizeof(relBitmask)), relBitmask) < 0) {
        return 0;
    }

    return test_bit(axis, relBitmask);
}

static int hasKey(int fd, short key) {
    unsigned char keyBitmask[(KEY_MAX + 1) / 8] = {0};

    if (ioctl(fd, EVIOCGBIT(EV_KEY, sizeof(keyBitmask)), keyBitmask) < 0) {
        return 0;
    }

    return test_bit(key, keyBitmask);
}

static void outputEvdevData(char *data, int dataSize) {
    char packetBuffer[EVDEV_MAX_EVENT_SIZE + sizeof(dataSize)];

    // Copy the full packet into our buffer
    memcpy(packetBuffer, &dataSize, sizeof(dataSize));
    memcpy(&packetBuffer[sizeof(dataSize)], data, dataSize);

    // Lock to prevent other threads from sending at the same time
    pthread_mutex_lock(&SocketSendLock);
    size_t offset = 0;
    size_t packetSize = dataSize + sizeof(dataSize);
    while (offset < packetSize) {
        ssize_t sent = send(sock, packetBuffer + offset, packetSize - offset, MSG_NOSIGNAL);
        if (sent < 0 && errno == EINTR) {
            continue;
        }
        if (sent <= 0) {
            shutdown(sock, SHUT_RDWR);
            break;
        }
        offset += sent;
    }
    pthread_mutex_unlock(&SocketSendLock);
}

void* pollThreadFunc(void* context) {
    struct DeviceEntry *device = context;
    struct pollfd pollinfo;
    int pollres, ret;
    char data[EVDEV_MAX_EVENT_SIZE];

    __android_log_print(ANDROID_LOG_INFO, "EvdevReader", "Polling /dev/input/%s", device->devName);

    pthread_mutex_lock(&DeviceListLock);
    if (grabbing) {
        // Exclusively grab the input device (required to make the Android cursor disappear)
        if (ioctl(device->fd, EVIOCGRAB, 1) < 0) {
            __android_log_print(ANDROID_LOG_ERROR, "EvdevReader",
                                "EVIOCGRAB failed for %s: %d", device->devName, errno);
            pthread_mutex_unlock(&DeviceListLock);
            goto cleanup;
        }
    }
    pthread_mutex_unlock(&DeviceListLock);

    for (;;) {
        do {
            // Unwait every 250 ms to return to caller if the fd is closed
            pollinfo.fd = device->fd;
            pollinfo.events = POLLIN;
            pollinfo.revents = 0;
            pollres = poll(&pollinfo, 1, 250);
        }
        while (pollres == 0);

        if (pollres > 0 && (pollinfo.revents & POLLIN)) {
            // We'll have data available now
            ret = read(device->fd, data, EVDEV_MAX_EVENT_SIZE);
            if (ret < 0) {
                __android_log_print(ANDROID_LOG_ERROR, "EvdevReader",
                                    "read() failed: %d", errno);
                goto cleanup;
            }
            else if (ret == 0) {
                __android_log_print(ANDROID_LOG_ERROR, "EvdevReader",
                                    "read() graceful EOF");
                goto cleanup;
            }
            else if (grabbing) {
                // Write out the data to our client
                outputEvdevData(data, ret);
            }
        }
        else {
            if (pollres < 0) {
                __android_log_print(ANDROID_LOG_ERROR, "EvdevReader",
                                    "poll() failed: %d", errno);
            }
            else {
                __android_log_print(ANDROID_LOG_ERROR, "EvdevReader",
                                    "Unexpected revents: %d", pollinfo.revents);
            }

            // Terminate this thread
            goto cleanup;
        }
    }

cleanup:
    __android_log_print(ANDROID_LOG_INFO, "EvdevReader", "Closing /dev/input/%s", device->devName);

    // Remove the context from the linked list
    {
        struct DeviceEntry *lastEntry;

        // Lock the device list
        pthread_mutex_lock(&DeviceListLock);

        if (DeviceListHead == device) {
            DeviceListHead = device->next;
        }
        else {
            lastEntry = DeviceListHead;
            while (lastEntry->next != NULL) {
                if (lastEntry->next == device) {
                    lastEntry->next = device->next;
                    break;
                }

                lastEntry = lastEntry->next;
            }
        }

        // Unlock device list
        pthread_mutex_unlock(&DeviceListLock);
    }

    // Free the context
    ioctl(device->fd, EVIOCGRAB, 0);
    close(device->fd);
    free(device);

    return NULL;
}

static int precheckDeviceForPolling(int fd) {
    int isMouse;
    int isKeyboard;
    int isGamepad;

    // This is the same check that Android does in EventHub.cpp
    isMouse = hasRelAxis(fd, REL_X) &&
           hasRelAxis(fd, REL_Y) &&
           hasKey(fd, BTN_LEFT);

    // This is the same check that Android does in EventHub.cpp
    isKeyboard = hasKey(fd, KEY_Q);

    isGamepad = hasKey(fd, BTN_GAMEPAD);

    // We only handle keyboards and mice that aren't gamepads
    return (isMouse || isKeyboard) && !isGamepad;
}

static void startPollForDevice(char* deviceName) {
    struct DeviceEntry *currentEntry;
    char fullPath[256];
    int fd;

    // Lock the device list
    pthread_mutex_lock(&DeviceListLock);

    // Check if the device is already being polled
    currentEntry = DeviceListHead;
    while (currentEntry != NULL) {
        if (strcmp(currentEntry->devName, deviceName) == 0) {
            // Already polling this device
            goto unlock;
        }

        currentEntry = currentEntry->next;
    }

    // Open the device
    sprintf(fullPath, "/dev/input/%s", deviceName);
    fd = open(fullPath, O_RDWR);
    if (fd < 0) {
        __android_log_print(ANDROID_LOG_ERROR, "EvdevReader", "Couldn't open %s: %d", fullPath, errno);
        goto unlock;
    }

    // Allocate a context
    currentEntry = malloc(sizeof(*currentEntry));
    if (currentEntry == NULL) {
        close(fd);
        goto unlock;
    }

    // Populate context
    currentEntry->fd = fd;
    strcpy(currentEntry->devName, deviceName);

    // Check if we support polling this device
    if (!precheckDeviceForPolling(fd)) {
        // Nope, get out
        free(currentEntry);
        close(fd);
        goto unlock;
    }

    // Start the polling thread
    if (pthread_create(&currentEntry->thread, NULL, pollThreadFunc, currentEntry) != 0) {
        free(currentEntry);
        close(fd);
        goto unlock;
    }
    pthread_detach(currentEntry->thread);

    // Queue this onto the device list
    currentEntry->next = DeviceListHead;
    DeviceListHead = currentEntry;

unlock:
    // Unlock and return
    pthread_mutex_unlock(&DeviceListLock);
}

static int enumerateDevices(void) {
    DIR *inputDir;
    struct dirent *dirEnt;

    inputDir = opendir("/dev/input");
    if (!inputDir) {
        __android_log_print(ANDROID_LOG_ERROR, "EvdevReader", "Couldn't open /dev/input: %d", errno);
        return -1;
    }

    // Start polling each device in /dev/input
    while ((dirEnt = readdir(inputDir)) != NULL) {
        if (strcmp(dirEnt->d_name, ".") == 0 || strcmp(dirEnt->d_name, "..") == 0) {
            // Skip these virtual directories
            continue;
        }

        if (strstr(dirEnt->d_name, "event") == NULL) {
            // Skip non-event devices
            continue;
        }

        startPollForDevice(dirEnt->d_name);
    }

    closedir(inputDir);
    return 0;
}

static int connectSocket(const char* socketName, uid_t appUid) {
    struct sockaddr_un saddr;
    int ret;
    size_t nameLength = strlen(socketName);
    if (nameLength == 0 || nameLength >= sizeof(saddr.sun_path)) {
        return -1;
    }

    sock = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (sock < 0) {
        __android_log_print(ANDROID_LOG_ERROR, "EvdevReader", "socket() failed: %d", errno);
        return -1;
    }

    memset(&saddr, 0, sizeof(saddr));
    saddr.sun_family = AF_UNIX;
    // Android LocalServerSocket uses Linux's abstract namespace (a leading NUL).
    memcpy(saddr.sun_path + 1, socketName, nameLength);
    ret = connect(sock, (struct sockaddr*)&saddr,
                  offsetof(struct sockaddr_un, sun_path) + 1 + nameLength);
    if (ret < 0) {
        __android_log_print(ANDROID_LOG_ERROR, "EvdevReader", "connect() failed: %d", errno);
        close(sock);
        return -1;
    }

    if (!evdevPeerUidMatches(sock, appUid)) {
        __android_log_print(ANDROID_LOG_ERROR, "EvdevReader", "Rejected unexpected app socket owner");
        close(sock);
        return -1;
    }

    __android_log_print(ANDROID_LOG_INFO, "EvdevReader", "Authenticated app connection established");

    return 0;
}

#define UNGRAB_REQ 1
#define REGRAB_REQ 2

int main(int argc, char* argv[]) {
    int ret;
    int pollres;
    struct pollfd pollinfo;
    char* uidEnd;
    unsigned long appUid;

    __android_log_print(ANDROID_LOG_INFO, "EvdevReader", "Entered main()");

    if (argc != 3 || geteuid() != 0) {
        __android_log_print(ANDROID_LOG_ERROR, "EvdevReader", "Expected root and socket name/app UID arguments");
        return -1;
    }
    errno = 0;
    appUid = strtoul(argv[2], &uidEnd, 10);
    if (errno != 0 || uidEnd == argv[2] || *uidEnd != '\0' ||
            appUid == 0 || (uid_t) appUid != appUid) {
        __android_log_print(ANDROID_LOG_ERROR, "EvdevReader", "Invalid app UID");
        return -1;
    }

    // Authenticate the app before opening or exclusively grabbing any input device.
    ret = connectSocket(argv[1], (uid_t) appUid);
    if (ret < 0) {
        return ret;
    }

    // Perform initial enumeration
    ret = enumerateDevices();
    if (ret < 0) {
        return ret;
    }

    // Wait for requests from the client
    for (;;) {
        unsigned char requestId;

        do {
            // Every second we poll again for new devices if
            // we haven't received any new events
            pollinfo.fd = sock;
            pollinfo.events = POLLIN;
            pollinfo.revents = 0;
            pollres = poll(&pollinfo, 1, 1000);
            if (pollres == 0) {
                // Timeout, re-enumerate devices
                enumerateDevices();
            }
        }
        while (pollres == 0);

        if (pollres > 0 && (pollinfo.revents & POLLIN)) {
            // We'll have data available now
            ret = recv(sock, &requestId, sizeof(requestId), 0);
            if (ret < sizeof(requestId)) {
                __android_log_print(ANDROID_LOG_ERROR, "EvdevReader", "Short read on socket");
                return errno;
            }

            if (requestId != UNGRAB_REQ && requestId != REGRAB_REQ) {
                __android_log_print(ANDROID_LOG_ERROR, "EvdevReader", "Unknown request");
                return requestId;
            }

            {
                struct DeviceEntry *currentEntry;

                pthread_mutex_lock(&DeviceListLock);

                // Update state for future devices
                grabbing = (requestId == REGRAB_REQ);

                // Carry out the requested action on each device
                currentEntry = DeviceListHead;
                while (currentEntry != NULL) {
                    ioctl(currentEntry->fd, EVIOCGRAB, grabbing);
                    currentEntry = currentEntry->next;
                }

                pthread_mutex_unlock(&DeviceListLock);

                __android_log_print(ANDROID_LOG_INFO, "EvdevReader", "New grab status is: %s",
                    grabbing ? "enabled" : "disabled");
            }
        }
        else {
            // Terminate this thread
            if (pollres < 0) {
                __android_log_print(ANDROID_LOG_ERROR, "EvdevReader",
                                    "Socket recv poll() failed: %d", errno);
            }
            else {
                __android_log_print(ANDROID_LOG_ERROR, "EvdevReader",
                                    "Socket poll unexpected revents: %d", pollinfo.revents);
            }

            return -1;
        }
    }
}
