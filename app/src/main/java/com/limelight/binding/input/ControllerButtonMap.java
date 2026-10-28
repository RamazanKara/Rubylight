package com.limelight.binding.input;

import android.content.Context;
import static com.limelight.nvstream.input.ControllerPacket.*;

import java.util.Locale;
import java.util.Map;
import java.util.TreeMap;

/**
 * User-defined button remapping for one controller model.
 * Keys and values are Android key codes; DISABLED swallows the button.
 */
public final class ControllerButtonMap {
    private static final String PREFERENCES = "ControllerButtonMaps";

    public static final int UNMAPPED = Integer.MIN_VALUE;
    public static final int DISABLED = 0; // KeyEvent.KEYCODE_UNKNOWN

    // KeyEvent constants, kept literal so this class stays testable on the JVM
    static final int BUTTON_A = 96, BUTTON_B = 97, BUTTON_C = 98, BUTTON_X = 99, BUTTON_Y = 100,
            BUTTON_Z = 101, BUTTON_L1 = 102, BUTTON_R1 = 103, BUTTON_L2 = 104, BUTTON_R2 = 105,
            BUTTON_THUMBL = 106, BUTTON_THUMBR = 107, BUTTON_START = 108, BUTTON_SELECT = 109,
            BUTTON_MODE = 110, BUTTON_1 = 188, BUTTON_16 = 203, MEDIA_RECORD = 130;

    /**
     * Extended buttons the host passes through. They have no Android key code, so they are stored
     * as codes above every key code; an older Nightjar ignores them as unsupported entries.
     */
    public static final int TARGET_TOUCHPAD = 0x10001, TARGET_MISC = 0x10002,
            TARGET_PADDLE1 = 0x10003, TARGET_PADDLE2 = 0x10004, TARGET_PADDLE3 = 0x10005, TARGET_PADDLE4 = 0x10006;

    /** Buttons that may be sent to the host, in display order. */
    public static final int[] TARGETS = {
            BUTTON_A, BUTTON_B, BUTTON_X, BUTTON_Y, BUTTON_L1, BUTTON_R1,
            BUTTON_THUMBL, BUTTON_THUMBR, BUTTON_START, BUTTON_SELECT, BUTTON_MODE,
            TARGET_TOUCHPAD, TARGET_MISC, TARGET_PADDLE1, TARGET_PADDLE2, TARGET_PADDLE3, TARGET_PADDLE4,
            DISABLED
    };

    /**
     * The buttons a USB driver report can remap, as the key codes of their flags. MEDIA_RECORD
     * stands for the report's Share (MISC_FLAG), as it does for the key Android gives an Xbox pad's
     * Share, so one stored mapping works over both paths.
     */
    private static final int[] FLAG_SOURCES = {
            BUTTON_A, BUTTON_B, BUTTON_X, BUTTON_Y, BUTTON_L1, BUTTON_R1,
            BUTTON_THUMBL, BUTTON_THUMBR, BUTTON_START, BUTTON_SELECT, BUTTON_MODE, MEDIA_RECORD
    };

    private final TreeMap<Integer, Integer> mappings = new TreeMap<>();

    /**
     * Digital buttons only. D-pad and trigger keys are left alone because their
     * hat/axis events would still arrive alongside a remapped key event. Extra buttons such as
     * C, Z, 1-16 and an Xbox pad's Share (MEDIA_RECORD) are sources too.
     */
    public static boolean isRemappableSource(int keyCode) {
        return (keyCode >= BUTTON_A && keyCode <= BUTTON_MODE && keyCode != BUTTON_L2 && keyCode != BUTTON_R2) ||
                (keyCode >= BUTTON_1 && keyCode <= BUTTON_16) || keyCode == MEDIA_RECORD;
    }

    public static boolean isTarget(int keyCode) {
        for (int target : TARGETS) {
            if (target == keyCode) {
                return true;
            }
        }
        return false;
    }

    /** Groups identical controller models; falls back to the device name when IDs are missing. */
    public static String deviceKey(int vendorId, int productId, String name) {
        if (vendorId != 0 || productId != 0) {
            return String.format(Locale.ROOT, "usb:%04x:%04x", vendorId & 0xFFFF, productId & 0xFFFF);
        }
        return "name:" + (name == null ? "" : name.trim());
    }

    public int map(int keyCode) {
        Integer target = mappings.get(keyCode);
        return target == null ? UNMAPPED : target;
    }

    public int mapButtonFlags(int flags, boolean flipFaceButtons) {
        int result = flags;
        for (int source : FLAG_SOURCES) result &= ~buttonFlag(source);
        for (int source : FLAG_SOURCES) {
            if ((flags & buttonFlag(source)) == 0) continue;
            int target = map(source);
            if (target == UNMAPPED) {
                target = source;
                if (flipFaceButtons) {
                    if (source == BUTTON_A) target = BUTTON_B;
                    else if (source == BUTTON_B) target = BUTTON_A;
                    else if (source == BUTTON_X) target = BUTTON_Y;
                    else if (source == BUTTON_Y) target = BUTTON_X;
                }
            }
            // Map the original report once; chained and many-to-one bindings must not release held targets.
            result |= buttonFlag(target);
        }
        return result;
    }

    /**
     * The extended buttons the mappings target, for the controller arrival: a pad sends one
     * once a mapping targets it, so the host must hear that it has one.
     */
    public int targetButtonFlags() {
        int flags = 0;
        for (int target : mappings.values()) {
            if (target >= TARGET_TOUCHPAD && target <= TARGET_PADDLE4) flags |= buttonFlag(target);
        }
        return flags;
    }

    static int buttonFlag(int keyCode) {
        switch (keyCode) {
            case TARGET_TOUCHPAD: return TOUCHPAD_FLAG;
            case TARGET_MISC:
            case MEDIA_RECORD: return MISC_FLAG;
            case TARGET_PADDLE1: return PADDLE1_FLAG;
            case TARGET_PADDLE2: return PADDLE2_FLAG;
            case TARGET_PADDLE3: return PADDLE3_FLAG;
            case TARGET_PADDLE4: return PADDLE4_FLAG;
            case BUTTON_A: return A_FLAG;
            case BUTTON_B: return B_FLAG;
            case BUTTON_X: return X_FLAG;
            case BUTTON_Y: return Y_FLAG;
            case BUTTON_L1: return LB_FLAG;
            case BUTTON_R1: return RB_FLAG;
            case BUTTON_THUMBL: return LS_CLK_FLAG;
            case BUTTON_THUMBR: return RS_CLK_FLAG;
            case BUTTON_START: return PLAY_FLAG;
            case BUTTON_SELECT: return BACK_FLAG;
            case BUTTON_MODE: return SPECIAL_BUTTON_FLAG;
            default: return 0;
        }
    }

    public void put(int source, int target) {
        if (!isRemappableSource(source) || !isTarget(target)) {
            throw new IllegalArgumentException("Unsupported button mapping");
        }
        if (source == target) {
            mappings.remove(source);
        } else {
            mappings.put(source, target);
        }
    }

    public void remove(int source) {
        mappings.remove(source);
    }

    public boolean isEmpty() {
        return mappings.isEmpty();
    }

    public Map<Integer, Integer> entries() {
        return new TreeMap<>(mappings);
    }

    public String serialize() {
        StringBuilder builder = new StringBuilder();
        for (Map.Entry<Integer, Integer> entry : mappings.entrySet()) {
            if (builder.length() > 0) {
                builder.append(',');
            }
            builder.append(entry.getKey()).append(':').append(entry.getValue());
        }
        return builder.toString();
    }

    /** Ignores malformed or unsupported entries so a bad value can never block input. */
    public static ControllerButtonMap deserialize(String value) {
        ControllerButtonMap map = new ControllerButtonMap();
        if (value == null || value.isEmpty()) {
            return map;
        }
        for (String entry : value.split(",")) {
            String[] parts = entry.split(":");
            if (parts.length != 2) {
                continue;
            }
            try {
                map.put(Integer.parseInt(parts[0].trim()), Integer.parseInt(parts[1].trim()));
            } catch (IllegalArgumentException ignored) {
            }
        }
        return map;
    }

    public static ControllerButtonMap load(Context context, String deviceKey) {
        return deserialize(context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
                .getString(deviceKey, null));
    }

    public void save(Context context, String deviceKey) {
        if (mappings.isEmpty()) {
            context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE).edit().remove(deviceKey).apply();
        } else {
            context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE).edit()
                    .putString(deviceKey, serialize()).apply();
        }
    }
}
