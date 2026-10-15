package com.limelight.binding.input;

import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.Map;

/** Reference counts shared chord keys and mouse buttons; each source releases only its own input. */
public final class HeldInputState {
    public interface Sink {
        void key(int key, boolean down);
        void mouse(int button, boolean down);
        /** The combined state of the layout's gamepad controls, sent as the on-screen gamepad. */
        default void gamepad(int buttons, short leftX, short leftY, short rightX, short rightY, byte leftTrigger, byte rightTrigger) { }
    }
    private final Sink sink;
    private final Map<String, TouchLayout.Button> held = new LinkedHashMap<>();
    private final Map<Integer, Integer> keys = new HashMap<>(), mouse = new HashMap<>();
    public HeldInputState(Sink sink) { this.sink = sink; }
    public boolean isHeld(String id) { return held.containsKey(id); }
    public void press(TouchLayout.Button button) {
        if (button.kind != TouchLayout.Kind.KEYBOARD && button.kind != TouchLayout.Kind.MOUSE) {
            throw new IllegalArgumentException("Not a keyboard or mouse control");
        }
        if (held.putIfAbsent(button.id, button) != null) return;
        if (button.mouseButton != 0) change(mouse, button.mouseButton, true, false);
        else for (int key : button.keys()) change(keys, key, true, true);
    }
    public void release(String id) {
        TouchLayout.Button button = held.remove(id);
        if (button == null) return;
        if (button.mouseButton != 0) change(mouse, button.mouseButton, false, false);
        else {
            int[] codes = button.keys();
            for (int i = codes.length - 1; i >= 0; i--) change(keys, codes[i], false, true);
        }
    }
    public void releaseAll() {
        String[] ids = held.keySet().toArray(new String[0]);
        for (int i = ids.length - 1; i >= 0; i--) release(ids[i]);
    }
    private void change(Map<Integer, Integer> state, int code, boolean down, boolean keyboard) {
        int previous = state.getOrDefault(code, 0);
        int next = previous + (down ? 1 : -1);
        if (next <= 0) state.remove(code); else state.put(code, next);
        if (down ? previous == 0 : next == 0) {
            if (keyboard) sink.key(code, down); else sink.mouse(code, down);
        }
    }
}
