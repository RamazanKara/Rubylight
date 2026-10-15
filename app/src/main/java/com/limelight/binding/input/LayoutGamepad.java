package com.limelight.binding.input;

/**
 * Combines a layout's gamepad controls into one controller report. Each control owns a slot, so two
 * controls bound to the same button or stick release only their own input. Updates do not allocate.
 */
public final class LayoutGamepad {
    private final HeldInputState.Sink sink;
    private final int[] flags = new int[TouchLayout.MAX_BUTTONS];
    private final int[] stickSide = new int[TouchLayout.MAX_BUTTONS];
    private final float[] stickX = new float[TouchLayout.MAX_BUTTONS], stickY = new float[TouchLayout.MAX_BUTTONS];
    private final int[] triggerSide = new int[TouchLayout.MAX_BUTTONS];
    private final float[] trigger = new float[TouchLayout.MAX_BUTTONS];
    private int sentButtons;
    private short sentLx, sentLy, sentRx, sentRy;
    private byte sentLt, sentRt;
    private boolean sent;

    public LayoutGamepad(HeldInputState.Sink sink) {
        this.sink = sink;
        clearSlots();
    }

    private void clearSlots() {
        for (int i = 0; i < flags.length; i++) {
            flags[i] = 0; stickSide[i] = -1; stickX[i] = 0; stickY[i] = 0; triggerSide[i] = -1; trigger[i] = 0;
        }
    }

    /** Buttons this slot holds, as {@link com.limelight.nvstream.input.ControllerPacket} flags. */
    public void setButtons(int slot, int buttons) {
        if (flags[slot] == buttons) return;
        flags[slot] = buttons;
        report();
    }

    public boolean holds(int slot) {
        return flags[slot] != 0 || stickX[slot] != 0 || stickY[slot] != 0 || trigger[slot] != 0;
    }

    /** Stick position from -1 to 1, with up positive. */
    public void setStick(int slot, int side, float x, float y) {
        if (stickSide[slot] == side && stickX[slot] == x && stickY[slot] == y) return;
        stickSide[slot] = side; stickX[slot] = x; stickY[slot] = y;
        report();
    }

    /** Trigger pull from 0 to 1. */
    public void setTrigger(int slot, int side, float value) {
        if (triggerSide[slot] == side && trigger[slot] == value) return;
        triggerSide[slot] = side; trigger[slot] = value;
        report();
    }

    /** Recenters every stick, lets go of every button and trigger, and reports the neutral state once. */
    public void releaseAll() {
        clearSlots();
        if (sent && (sentButtons != 0 || sentLx != 0 || sentLy != 0 || sentRx != 0 || sentRy != 0 || sentLt != 0 || sentRt != 0)) {
            report();
        }
    }

    /** Sends the last report again; the caller schedules this after a change, as the fixed gamepad does. */
    public void resend() {
        if (sent) sink.gamepad(sentButtons, sentLx, sentLy, sentRx, sentRy, sentLt, sentRt);
    }

    private void report() {
        int buttons = 0;
        float lx = 0, ly = 0, rx = 0, ry = 0, lt = 0, rt = 0;
        for (int i = 0; i < flags.length; i++) {
            buttons |= flags[i];
            // Two sticks on one side: the one pushed further wins, like two pads sharing a slot.
            if (stickSide[i] == TouchLayout.LEFT && magnitude(stickX[i], stickY[i]) > magnitude(lx, ly)) {
                lx = stickX[i]; ly = stickY[i];
            } else if (stickSide[i] == TouchLayout.RIGHT && magnitude(stickX[i], stickY[i]) > magnitude(rx, ry)) {
                rx = stickX[i]; ry = stickY[i];
            }
            if (triggerSide[i] == TouchLayout.LEFT) lt = Math.max(lt, trigger[i]);
            else if (triggerSide[i] == TouchLayout.RIGHT) rt = Math.max(rt, trigger[i]);
        }
        short slx = axis(lx), sly = axis(ly), srx = axis(rx), sry = axis(ry);
        byte slt = triggerByte(lt), srt = triggerByte(rt);
        if (sent && buttons == sentButtons && slx == sentLx && sly == sentLy && srx == sentRx && sry == sentRy &&
                slt == sentLt && srt == sentRt) {
            return;
        }
        sent = true;
        sentButtons = buttons; sentLx = slx; sentLy = sly; sentRx = srx; sentRy = sry; sentLt = slt; sentRt = srt;
        sink.gamepad(buttons, slx, sly, srx, sry, slt, srt);
    }

    private static float magnitude(float x, float y) {
        return x * x + y * y;
    }

    static short axis(float value) {
        return (short) Math.round(Math.max(-1, Math.min(1, value)) * 0x7FFE);
    }

    static byte triggerByte(float value) {
        return (byte) Math.round(Math.max(0, Math.min(1, value)) * 0xFF);
    }
}
