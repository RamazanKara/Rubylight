package com.limelight.binding.input.keyboard;

import com.limelight.nvstream.input.KeyboardPacket;

/** The keys of the on-screen keyboard that stay down for the keys after them. */
public enum PcModifier {
    SHIFT(0xA0, KeyboardPacket.MODIFIER_SHIFT),
    CTRL(0xA2, KeyboardPacket.MODIFIER_CTRL),
    ALT(0xA4, KeyboardPacket.MODIFIER_ALT),
    WIN(0x5B, KeyboardPacket.MODIFIER_META),
    /** The right Alt key, which a German or other Windows layout turns into AltGr. */
    ALTGR(0xA5, KeyboardPacket.MODIFIER_ALT);

    /** The Windows virtual-key code sent while the modifier is down. */
    public final int vk;
    /** The bit this modifier adds to the modifier byte of every keyboard packet. */
    public final byte bit;

    PcModifier(int vk, byte bit) {
        this.vk = vk;
        this.bit = bit;
    }
}
