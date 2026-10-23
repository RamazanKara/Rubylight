package com.limelight.binding.input.keyboard;

/**
 * One key of the on-screen keyboard. A key is a physical position: {@link #vk} is the code a US
 * keyboard sends from that position, and the host's own layout decides the character. The labels
 * only say what a host with the matching layout types.
 */
public final class PcKey {
    public final String id;
    public final int vk;
    public final String label;
    /** What the key types with Shift down, or null when the label is also the shifted text. */
    public final String shiftLabel;
    /** What the key types with AltGr down, or null when it types nothing there. */
    public final String altGrLabel;
    public final PcModifier modifier;
    /** Left edge and width in key units; the keyboard is {@link PcKeyboardLayout#widthUnits()} wide. */
    public final float x, width;
    /** Whether holding the key repeats it, as a PC keyboard does. */
    public final boolean repeats;

    PcKey(String id, int vk, String label, String shiftLabel, String altGrLabel, PcModifier modifier,
          float x, float width, boolean repeats) {
        this.id = id;
        this.vk = vk;
        this.label = label;
        this.shiftLabel = shiftLabel;
        this.altGrLabel = altGrLabel;
        this.modifier = modifier;
        this.x = x;
        this.width = width;
        this.repeats = repeats;
    }

    public float centerX() {
        return x + width / 2;
    }

    /** The text drawn on the key while the given modifiers are active. */
    public String labelFor(boolean shift, boolean altGr) {
        if (altGr && altGrLabel != null) return altGrLabel;
        if (shift && shiftLabel != null) return shiftLabel;
        return label;
    }

    @Override
    public String toString() {
        return id;
    }
}
