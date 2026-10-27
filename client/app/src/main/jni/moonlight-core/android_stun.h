#pragma once

#pragma pack(push, 1)

typedef struct _STUN_ATTRIBUTE_HEADER {
    unsigned short type;
    unsigned short length;
} STUN_ATTRIBUTE_HEADER, *PSTUN_ATTRIBUTE_HEADER;

typedef struct _STUN_MAPPED_IPV4_ADDRESS_ATTRIBUTE {
    STUN_ATTRIBUTE_HEADER hdr;
    unsigned char reserved;
    unsigned char addressFamily;
    unsigned short port;
    unsigned int address;
} STUN_MAPPED_IPV4_ADDRESS_ATTRIBUTE, *PSTUN_MAPPED_IPV4_ADDRESS_ATTRIBUTE;

typedef struct _STUN_MESSAGE {
    unsigned short messageType;
    unsigned short messageLength;
    unsigned int magicCookie;
    unsigned char transactionId[12];
} STUN_MESSAGE, *PSTUN_MESSAGE;

#pragma pack(pop)

static int parseStunAttributes(char* buffer, int bytesRead, unsigned int cookie, unsigned int* wanAddr) {
    PSTUN_ATTRIBUTE_HEADER attribute = (PSTUN_ATTRIBUTE_HEADER)buffer;
    PSTUN_MAPPED_IPV4_ADDRESS_ATTRIBUTE ipv4Attrib;
    while (bytesRead >= (int)sizeof(*attribute)) {
        int attributeLength = sizeof(*attribute) + ((htons(attribute->length) + 3) & ~3);
        if (bytesRead < attributeLength) {
            Limelog("STUN attribute out of bounds: %d\n", htons(attribute->length));
            return -5;
        }
        // Mask off the comprehension bit
        else if ((htons(attribute->type) & 0x7FFF) != STUN_ATTRIBUTE_XOR_MAPPED_ADDRESS) {
            // Continue searching if this wasn't our address
            bytesRead -= attributeLength;
            attribute = (PSTUN_ATTRIBUTE_HEADER)(((char*)attribute) + attributeLength);
            continue;
        }

        ipv4Attrib = (PSTUN_MAPPED_IPV4_ADDRESS_ATTRIBUTE)attribute;
        if (htons(ipv4Attrib->hdr.length) != 8) {
            Limelog("STUN address length mismatch: %d\n", htons(ipv4Attrib->hdr.length));
            return -5;
        }
        else if (ipv4Attrib->addressFamily != 1) {
            Limelog("STUN address family mismatch: %x\n", ipv4Attrib->addressFamily);
            return -5;
        }

        // The address is XORed with the cookie
        *wanAddr = ipv4Attrib->address ^ cookie;

        return 0;
    }

    Limelog("No XOR mapped address found in STUN response!\n");
    return -6;

}
