package com.limelight.binding.input;

import java.util.Arrays;
import java.util.function.Consumer;

public final class InputBatcher {
    public static final class MouseMotion {
        public int x, y;
        public short width, height;
        public int kind;
    }

    private final Consumer<MouseMotion> sendMouse;
    private final MouseMotion mouse = new MouseMotion();
    private boolean mousePending;
    private final Runnable[] controllers = new Runnable[16];
    private final int[] buttons = new int[16];
    private final int[] triggerEdges = new int[16];
    private final boolean[] known = new boolean[16];
    private short activeMask;

    public InputBatcher(Consumer<MouseMotion> sendMouse) {
        this.sendMouse = sendMouse;
    }

    public static long intervalNs(float panelHz) {
        if (!Float.isFinite(panelHz) || panelHz <= 0) panelHz = 60;
        return Math.round(1_000_000_000.0 / Math.max(1, Math.min(1000, panelHz)));
    }

    // kind: relative, absolute, or relative mapped into an absolute coordinate space.
    public synchronized void mouse(int kind, short x, short y, short width, short height) {
        if (mousePending && (mouse.kind != kind || mouse.width != width || mouse.height != height ||
                (kind != 1 && ((long) mouse.x + x > Short.MAX_VALUE || (long) mouse.x + x < Short.MIN_VALUE ||
                        (long) mouse.y + y > Short.MAX_VALUE || (long) mouse.y + y < Short.MIN_VALUE)))) {
            flushMouse();
        }
        mouse.kind = kind;
        mouse.width = width;
        mouse.height = height;
        mouse.x = kind == 1 ? x : mouse.x + x;
        mouse.y = kind == 1 ? y : mouse.y + y;
        mousePending = true;
    }

    public synchronized void controller(short number, short mask, int flags, byte leftTrigger,
                                        byte rightTrigger, Runnable send) {
        int index = number & 15;
        int edges = (leftTrigger == 0 ? 0 : 1) | (rightTrigger == 0 ? 0 : 2);
        if (!known[index] || mask != activeMask || buttons[index] != flags || triggerEdges[index] != edges) {
            // Flush motion before an edge so clicks, short presses, releases and detach are never erased.
            flush();
            send.run();
        } else {
            controllers[index] = send;
        }
        known[index] = (mask & (1 << index)) != 0;
        activeMask = mask;
        buttons[index] = flags;
        triggerEdges[index] = edges;
    }

    private void flushMouse() {
        if (mousePending) {
            sendMouse.accept(mouse);
            mousePending = false;
            mouse.x = mouse.y = 0;
        }
    }

    public synchronized void flush() {
        flushMouse();
        for (int i = 0; i < controllers.length; i++) {
            Runnable send = controllers[i];
            controllers[i] = null;
            if (send != null) send.run();
        }
    }

    public synchronized void boundary(Runnable send) {
        flush();
        send.run();
    }

    public synchronized void clear() {
        mousePending = false;
        mouse.x = mouse.y = 0;
        Arrays.fill(controllers, null);
        Arrays.fill(known, false);
        activeMask = 0;
    }
}
