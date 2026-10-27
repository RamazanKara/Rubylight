package com.limelight.ui;

import android.app.Application;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class)
public class LocalCursorViewTest {
    private static final class Square extends ColorDrawable {
        @Override public int getIntrinsicWidth() {
            return 16;
        }

        @Override public int getIntrinsicHeight() {
            return 16;
        }
    }

    @Test
    public void thePointerStaysOverItsSpotWhenTheViewIsLaidOutAfterItWasMoved() {
        LocalCursorView cursor = new LocalCursorView(RuntimeEnvironment.getApplication());
        Drawable square = new Square();
        cursor.setCursor(square, 2, 3);
        cursor.measure(0, 0);
        cursor.layout(0, 0, 16, 16);
        cursor.moveTo(100, 80);
        assertEquals(98, cursor.getTranslationX(), 0);
        assertEquals(77, cursor.getTranslationY(), 0);

        // A padding change moves the view; the pointer was already placed with the old left/top.
        cursor.layout(40, 24, 56, 40);
        assertEquals(100, cursor.getLeft() + cursor.getTranslationX() + 2, 0);
        assertEquals(80, cursor.getTop() + cursor.getTranslationY() + 3, 0);
    }
}
