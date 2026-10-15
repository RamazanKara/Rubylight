package com.limelight.binding.input.touch;

import java.util.LinkedHashMap;
import java.util.Map;

/**
 * Decoded host cursor shapes by id. Defining and showing an id both make it the most recently
 * used, and only the {@link #CAPACITY} most recently used stay: the host keeps the same order, so
 * it knows which ids it must send again (docs/protocol/cursor-shape.md).
 */
public final class CursorShapeCache<T> {
    public static final int CAPACITY = 32;

    private final LinkedHashMap<Integer, T> shapes = new LinkedHashMap<Integer, T>(CAPACITY * 2, 0.75f, true) {
        @Override
        protected boolean removeEldestEntry(Map.Entry<Integer, T> eldest) {
            return size() > CAPACITY;
        }
    };

    public void put(int id, T shape) {
        shapes.remove(id);
        shapes.put(id, shape);
    }

    /** The shape, now the most recently used, or null when it was never defined or has been dropped. */
    public T get(int id) {
        return shapes.get(id);
    }

    public void remove(int id) {
        shapes.remove(id);
    }

    public void clear() {
        shapes.clear();
    }

    public int size() {
        return shapes.size();
    }

    boolean contains(int id) {
        return shapes.containsKey(id);
    }
}
