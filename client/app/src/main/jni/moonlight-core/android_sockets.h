#pragma once

int sendMtuSafe(SOCKET s, char* buffer, int size) {
    int bytesSent = 0;

    while (bytesSent < size) {
        int bytesToSend = size - bytesSent > TCPv4_MSS ?
                          TCPv4_MSS : size - bytesSent;

        int sent = send(s, &buffer[bytesSent], bytesToSend, 0);
        if (sent < 0) {
            if (LastSocketError() == EINTR) {
                continue;
            }
            return -1;
        }
        if (sent == 0) {
            SetLastSocketError(EPIPE);
            return -1;
        }

        bytesSent += sent;
    }

    return bytesSent;
}

bool isInSubnetV6(struct sockaddr_in6* sin6, unsigned char* subnet, int prefixLength) {
    int i;

    for (i = 0; i < prefixLength; i++) {
        unsigned char mask = 0x80 >> (i % 8);
        if ((sin6->sin6_addr.s6_addr[i / 8] & mask) != (subnet[i / 8] & mask)) {
            return false;
        }
    }

    return true;
}

bool isPrivateNetworkAddress(struct sockaddr_storage* address) {
    if (address->ss_family == AF_INET) {
        return isPrivateNetworkAddressV4((struct sockaddr_in*)address, false);
    }
#ifdef AF_INET6
    else if (address->ss_family == AF_INET6) {
        struct sockaddr_in6* sin6 = (struct sockaddr_in6*)address;
        static unsigned char linkLocalPrefix[] = {0xfe, 0x80};
        static unsigned char siteLocalPrefix[] = {0xfe, 0xc0};
        static unsigned char uniqueLocalPrefix[] = {0xfc, 0x00};

        // fe80::/10
        if (isInSubnetV6(sin6, linkLocalPrefix, 10)) {
            return true;
        }
        // fec0::/10
        else if (isInSubnetV6(sin6, siteLocalPrefix, 10)) {
            return true;
        }
        // fc00::/7
        else if (isInSubnetV6(sin6, uniqueLocalPrefix, 7)) {
            return true;
        }
    }
#endif

    return false;
}
