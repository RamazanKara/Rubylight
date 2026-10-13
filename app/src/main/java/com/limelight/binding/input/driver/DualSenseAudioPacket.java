package com.limelight.binding.input.driver;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;

/** Versioned, little-endian controller audio messages. */
final class DualSenseAudioPacket {
    static final int SPEAKER = 0x5533, MICROPHONE = 0x5534, CONTROL = 0x5535;
    static int id(byte[] p) { return p == null || p.length < 4 ? -1 : (p[2] & 255) | ((p[3] & 255) << 8); }
    static boolean validPcm(byte[] p) {
        if (p == null || p.length < 12 || p[0] != 1 || p[1] != 1 || id(p) >= 16 || p[10] != 0 || p[11] != 0) return false;
        int frames = (p[8] & 255) | ((p[9] & 255) << 8);
        return frames > 0 && frames <= 240 && p.length == 12 + frames * 4;
    }
    static int sequence(byte[] p) { return ByteBuffer.wrap(p, 4, 4).order(ByteOrder.LITTLE_ENDIAN).getInt(); }
    static byte[] pcm(int id, int sequence, byte[] pcm, int length) {
        if (id < 0 || id >= 16 || pcm == null || length <= 0 || length > pcm.length || length > 960 || length % 4 != 0) return null;
        return ByteBuffer.allocate(12 + length).order(ByteOrder.LITTLE_ENDIAN)
                .put((byte)1).put((byte)1).putShort((short)id).putInt(sequence)
                .putShort((short)(length / 4)).putShort((short)0).put(pcm, 0, length).array();
    }
    static byte[] status(int id, int flags) { return new byte[] {1,1,(byte)id,0,(byte)flags,0,0,0}; }
    static boolean validControl(byte[] p) {
        return p != null && id(p) >= 0 && id(p) < 16 && p[0] == 1 &&
                ((p[1] == 2 && p.length == 12 && (p[4] & 15) == 0 && (p[5] & ~3) == 0) ||
                 (p[1] == 3 && p.length == 8 && (p[4] & ~15) == 0 && p[5] == 0 && p[6] == 0 && p[7] == 0));
    }
}
