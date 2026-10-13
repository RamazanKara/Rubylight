package com.limelight.binding.input;

import android.content.Context;
import com.limelight.nvstream.input.ControllerPacket;
import java.util.Arrays;

/** Remaps normalized reports, so Android key/hat duplicates and USB use the same held state. */
public final class ControllerAxisMap {
    public static final int LX = 0, LY = 1, RX = 2, RY = 3, LT = 4, RT = 5;
    public static final int[] DPAD_FLAGS = {ControllerPacket.UP_FLAG, ControllerPacket.DOWN_FLAG,
            ControllerPacket.LEFT_FLAG, ControllerPacket.RIGHT_FLAG};
    public static final int[] DPAD_TARGETS = {0, ControllerPacket.UP_FLAG, ControllerPacket.DOWN_FLAG,
            ControllerPacket.LEFT_FLAG, ControllerPacket.RIGHT_FLAG, ControllerPacket.A_FLAG,
            ControllerPacket.B_FLAG, ControllerPacket.X_FLAG, ControllerPacket.Y_FLAG,
            ControllerPacket.LB_FLAG, ControllerPacket.RB_FLAG, ControllerPacket.LS_CLK_FLAG,
            ControllerPacket.RS_CLK_FLAG, ControllerPacket.PLAY_FLAG, ControllerPacket.BACK_FLAG,
            ControllerPacket.SPECIAL_BUTTON_FLAG};
    // Each destination reads one physical source. 0 means disabled; sign encodes inversion.
    private final int[] axes = {1, 2, 3, 4, 5, 6};
    private final int[] dpad = DPAD_FLAGS.clone();

    public void setAxis(int destination, int source, boolean inverted) {
        if (destination < 0 || destination >= 6 || source < -1 || source >= 6 ||
                (source >= 0 && (destination < 4) != (source < 4)) || (destination >= 4 && inverted)) {
            throw new IllegalArgumentException("Unsupported axis mapping");
        }
        axes[destination] = source < 0 ? 0 : (source + 1) * (inverted ? -1 : 1);
    }
    public int source(int destination) { return Math.abs(axes[destination]) - 1; }
    public boolean inverted(int destination) { return axes[destination] < 0; }
    public void setDpad(int source, int target) {
        if (source < 0 || source >= 4) throw new IllegalArgumentException("Invalid direction");
        boolean valid = false;
        for (int flag : DPAD_TARGETS) if (flag == target) valid = true;
        if (!valid) throw new IllegalArgumentException("Invalid target");
        dpad[source] = target;
    }
    public int dpadTarget(int source) { return dpad[source]; }
    public boolean isEmpty() {
        return Arrays.equals(axes, new int[] {1,2,3,4,5,6}) && Arrays.equals(dpad, DPAD_FLAGS);
    }
    public static final class Report {
        public int buttons;
        public byte lt, rt;
        public short lx, ly, rx, ry;
    }
    public Report apply(int buttons, byte lt, byte rt, short lx, short ly, short rx, short ry) {
        Report result = new Report();
        apply(buttons, lt, rt, lx, ly, rx, ry, result);
        return result;
    }
    public void apply(int buttons, byte lt, byte rt, short lx, short ly, short rx, short ry, Report result) {
        int mask = ControllerPacket.UP_FLAG | ControllerPacket.DOWN_FLAG | ControllerPacket.LEFT_FLAG | ControllerPacket.RIGHT_FLAG;
        result.buttons = buttons & ~mask;
        for (int i = 0; i < 4; i++) if ((buttons & DPAD_FLAGS[i]) != 0) result.buttons |= dpad[i];
        for (int i = 0; i < 6; i++) {
            int source = source(i);
            int value;
            switch (source) {
                case LX: value = lx; break;
                case LY: value = ly; break;
                case RX: value = rx; break;
                case RY: value = ry; break;
                case LT: value = Byte.toUnsignedInt(lt); break;
                case RT: value = Byte.toUnsignedInt(rt); break;
                default: value = 0;
            }
            if (inverted(i)) value = -value;
            if (i < 4) value = Math.max(-32767, Math.min(32767, value));
            switch (i) {
                case LX: result.lx = (short)value; break;
                case LY: result.ly = (short)value; break;
                case RX: result.rx = (short)value; break;
                case RY: result.ry = (short)value; break;
                case LT: result.lt = (byte)value; break;
                case RT: result.rt = (byte)value; break;
            }
        }
    }
    public String serialize() {
        StringBuilder text = new StringBuilder("1");
        for (int value : axes) text.append(',').append(value);
        for (int value : dpad) text.append(',').append(value);
        return text.toString();
    }
    public static ControllerAxisMap deserialize(String value) {
        ControllerAxisMap map = new ControllerAxisMap();
        if (value == null) return map;
        try {
            String[] parts = value.split(",");
            if (parts.length != 11 || !parts[0].equals("1")) return map;
            ControllerAxisMap parsed = new ControllerAxisMap();
            for (int i = 0; i < 6; i++) {
                int entry = Integer.parseInt(parts[i + 1]);
                parsed.setAxis(i, Math.abs(entry) - 1, entry < 0);
            }
            for (int i = 0; i < 4; i++) parsed.setDpad(i, Integer.parseInt(parts[i + 7]));
            return parsed;
        } catch (IllegalArgumentException e) { return map; }
    }
    public static ControllerAxisMap load(Context context, String key) {
        return deserialize(context.getSharedPreferences("ControllerAxisMaps", Context.MODE_PRIVATE).getString(key, null));
    }
    public void save(Context context, String key) {
        context.getSharedPreferences("ControllerAxisMaps", Context.MODE_PRIVATE).edit().putString(key, serialize()).apply();
    }
}
