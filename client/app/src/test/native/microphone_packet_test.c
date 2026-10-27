// Microphone datagrams against the host's test vectors (Rubylight rust/core/src/mic.rs, whose
// ciphertexts come from moonlight-common-c's own PltEncryptMessage with OpenSSL).
//
// Build from the repository root (needs the OpenSSL headers):
//   m=app/src/main/jni/moonlight-core
//   cc -std=gnu11 -DHAS_SOCKLEN_T=1 -DHAVE_CLOCK_GETTIME=1 -I $m -I $m/moonlight-common-c/src
//      -I $m/moonlight-common-c/enet/include app/src/test/native/microphone_packet_test.c
//      $m/moonlight-common-c/src/PlatformCrypto.c -lcrypto -o /tmp/microphone-packet-test
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <openssl/evp.h>

#include "Limelight-internal.h"
#include "android_microphone_packet.h"

// Platform.h turns CHECK() off, so checks are explicit.
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__, __LINE__, #x); exit(1); } } while (0)

static const uint8_t KEY[16] = {7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7};

static void toHex(const uint8_t* data, int length, char* out) {
    for (int i = 0; i < length; i++) {
        sprintf(out + 2 * i, "%02x", data[i]);
    }
    out[2 * length] = 0;
}

// Raw AES-128-CBC decryption with no padding removed, to see both padding layers.
static int decryptRaw(const uint8_t* key, const uint8_t* iv, const uint8_t* in, int length, uint8_t* out) {
    EVP_CIPHER_CTX* ctx = EVP_CIPHER_CTX_new();
    int written = 0, final = 0;
    CHECK(ctx != NULL);
    CHECK(EVP_DecryptInit_ex(ctx, EVP_aes_128_cbc(), NULL, key, iv) == 1);
    EVP_CIPHER_CTX_set_padding(ctx, 0);
    CHECK(EVP_DecryptUpdate(ctx, out, &written, in, length) == 1);
    CHECK(EVP_DecryptFinal_ex(ctx, out + written, &final) == 1);
    EVP_CIPHER_CTX_free(ctx);
    return written + final;
}

static void headerAndPadding(void) {
    PPLT_CRYPTO_CONTEXT ctx = PltCreateCryptoContext();
    uint8_t packet[MAX_MIC_PACKET_SIZE];
    uint8_t plain[64];
    uint8_t iv[16] = {0};
    static const uint8_t header[12] = {0, 0x61, 3, 0, 4, 3, 2, 1, 0x78, 0x56, 0x34, 0x12};
    int length = micBuildPacket(ctx, KEY, 0xfffffffeu, 3, 0x01020304u,
                                (const uint8_t*)"opus frame", 10, packet);

    // flags, type, LE sequence, LE timestamp, LE magic
    CHECK(length == MIC_PACKET_HEADER_SIZE + 32);
    CHECK(memcmp(packet, header, sizeof(header)) == 0);

    // Key ID + sequence wraps to 1; the frame padded to a block, then a full block of padding.
    iv[3] = 1;
    CHECK(decryptRaw(KEY, iv, packet + MIC_PACKET_HEADER_SIZE, 32, plain) == 32);
    CHECK(memcmp(plain, "opus frame", 10) == 0);
    for (int i = 10; i < 16; i++) CHECK(plain[i] == 6);
    for (int i = 16; i < 32; i++) CHECK(plain[i] == 16);
    PltDestroyCryptoContext(ctx);
}

static void referenceCiphertexts(void) {
    static const struct {
        int length;
        const char* hex;
    } vectors[] = {
        {4, "306a36d0512bb803ccc69a69ae7e2aed501d20abae2a575017bdb0ca2cf32fc3"},
        {16, "1e10689ae2b276383d9f36756341ba398f6060d44c12333163d5a710f9c2c20e"},
        {33, "1e10689ae2b276383d9f36756341ba3996287332b3430f8ba7119c451d482a13"
             "c19f4f2afd4574a761afc77c968ba6025e91b8ea56f889d013870cd329a41bd6"},
    };
    // One context for every packet, as in a session: the key stays, the IV is reset each time.
    PPLT_CRYPTO_CONTEXT ctx = PltCreateCryptoContext();
    for (int round = 0; round < 2; round++) {
        for (size_t v = 0; v < sizeof(vectors) / sizeof(vectors[0]); v++) {
            uint8_t opus[64];
            uint8_t packet[MAX_MIC_PACKET_SIZE];
            char hex[2 * MAX_MIC_PACKET_SIZE + 1];
            for (int i = 0; i < vectors[v].length; i++) opus[i] = (uint8_t)(0x20 + i);
            int length = micBuildPacket(ctx, KEY, 9, 70, 0, opus, vectors[v].length, packet);
            CHECK(length > MIC_PACKET_HEADER_SIZE);
            toHex(packet + MIC_PACKET_HEADER_SIZE, length - MIC_PACKET_HEADER_SIZE, hex);
            if (strcmp(hex, vectors[v].hex) != 0) {
                fprintf(stderr, "length %d: got %s\n         want %s\n", vectors[v].length, hex, vectors[v].hex);
                exit(1);
            }
            // The caller's buffer is never padded in place.
            for (int i = 0; i < vectors[v].length; i++) CHECK(opus[i] == (uint8_t)(0x20 + i));
        }
    }
    PltDestroyCryptoContext(ctx);
}

static void sizeLimits(void) {
    PPLT_CRYPTO_CONTEXT ctx = PltCreateCryptoContext();
    static uint8_t opus[MAX_MIC_PACKET_SIZE];
    uint8_t packet[MAX_MIC_PACKET_SIZE];
    // 12 + 1360 + a padding block = 1388 fits; one byte more needs 1404.
    CHECK(micBuildPacket(ctx, KEY, 1, 0, 0, opus, 1360, packet) == 1388);
    CHECK(micBuildPacket(ctx, KEY, 1, 1, 0, opus, 1361, packet) == -1);
    CHECK(micBuildPacket(ctx, KEY, 1, 2, 0, opus, MAX_MIC_PACKET_SIZE, packet) == -1);
    CHECK(micBuildPacket(ctx, KEY, 1, 3, 0, opus, 0, packet) == -1);
    CHECK(micBuildPacket(NULL, KEY, 1, 4, 0, opus, 10, packet) == -1);
    PltDestroyCryptoContext(ctx);
}

static void offers(void) {
    uint16_t port = 1;
    // A Rubylight DESCRIBE answer ends with the microphone section.
    const char* offered =
        "v=0\r\no=android 0 14 IN IPv4 0.0.0.0\r\ns=NVIDIA Streaming Client\r\n"
        "a=x-ss-general.encryptionSupported:15\r\na=x-ss-general.encryptionRequested:13\r\n"
        "m=video 47998 RTP/AVP 96\r\n"
        "m=audio 48001 RTP/AVP 96\r\na=rtpmap:96 opus/48000/1\r\na=fmtp:96 minptime=10;useinbandfec=1\r\n";
    CHECK(micParseOffer(offered, &port) == 1 && port == 48001);
    CHECK(micParseOffer("a=rtpmap:96 opus/48000/1\r\n", &port) == 1 && port == 0);
    CHECK(micParseOffer("m=audio 50001 RTP/AVP 96\r\na=rtpmap:96 opus/48000/2\r\n", &port) == 1 && port == 50001);
    CHECK(micParseOffer("m=audio 48001 RTP/AVP 96\r\na=fmtp:97 surround-params=21101\r\n", &port) == 0 && port == 0);
    CHECK(micParseOffer(NULL, &port) == 0);
}

int main(void) {
    headerAndPadding();
    referenceCiphertexts();
    sizeLimits();
    offers();
    puts("microphone packets match the host's vectors");
    return 0;
}
