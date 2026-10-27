package com.limelight;

import android.view.KeyEvent;
import org.junit.Test;

import java.lang.reflect.Method;

import static org.junit.Assert.*;

public class GameInputTest {
    @Test
    public void releasingOneSideOfAModifierKeepsTheOtherSideHeld() throws Exception {
        Method mask = Game.class.getDeclaredMethod("getModifierMask", int.class);
        mask.setAccessible(true);
        int[][] pairs = {{KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_CTRL_RIGHT},
                {KeyEvent.KEYCODE_SHIFT_LEFT, KeyEvent.KEYCODE_SHIFT_RIGHT},
                {KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_ALT_RIGHT},
                {KeyEvent.KEYCODE_META_LEFT, KeyEvent.KEYCODE_META_RIGHT}};
        for (int[] pair : pairs) {
            int left = (int) mask.invoke(null, pair[0]);
            int right = (int) mask.invoke(null, pair[1]);
            int held = left | right;
            assertEquals(right, held & ~left);
            assertEquals(left, held & ~right);
            assertEquals(left, (right >> 8));
        }
        assertEquals(0, mask.invoke(null, KeyEvent.KEYCODE_A));
    }
}
