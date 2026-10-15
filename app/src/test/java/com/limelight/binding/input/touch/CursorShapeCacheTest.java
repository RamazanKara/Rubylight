package com.limelight.binding.input.touch;

import org.junit.Test;

import static org.junit.Assert.*;

public class CursorShapeCacheTest {
    @Test
    public void keepsTheMostRecentlyDefinedOrShownIds() {
        CursorShapeCache<String> cache = new CursorShapeCache<>();
        for (int id = 1; id <= CursorShapeCache.CAPACITY; id++) {
            cache.put(id, "shape " + id);
        }
        assertEquals(CursorShapeCache.CAPACITY, cache.size());
        // Showing id 1 makes it recent, so the next definition drops id 2 instead.
        assertEquals("shape 1", cache.get(1));
        cache.put(100, "shape 100");
        assertEquals(CursorShapeCache.CAPACITY, cache.size());
        assertTrue(cache.contains(1));
        assertFalse(cache.contains(2));
        assertNull(cache.get(2));
        assertTrue(cache.contains(3));
    }

    @Test
    public void redefiningAnIdReplacesItAndMakesItRecent() {
        CursorShapeCache<String> cache = new CursorShapeCache<>();
        cache.put(-1, "old");
        for (int id = 1; id < CursorShapeCache.CAPACITY; id++) {
            cache.put(id, "shape " + id);
        }
        cache.put(-1, "new");
        cache.put(1000, "shape 1000");
        assertEquals("new", cache.get(-1));
        assertFalse(cache.contains(1));
    }

    @Test
    public void removeAndClearForgetShapes() {
        CursorShapeCache<String> cache = new CursorShapeCache<>();
        cache.put(5, "five");
        cache.put(6, "six");
        cache.remove(5);
        assertNull(cache.get(5));
        assertEquals("six", cache.get(6));
        cache.clear();
        assertEquals(0, cache.size());
        assertNull(cache.get(6));
    }
}
