package com.limelight.ui;

import android.app.Application;
import android.content.Context;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;
import android.os.Handler;
import android.view.View;

import com.limelight.binding.input.touch.CursorShapeMessage;
import com.limelight.binding.input.touch.TrackpadPointer;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class)
public class HostCursorShapesTest {
    private static final class CountingCursorView extends LocalCursorView {
        int shapes;
        float hotspotX;

        CountingCursorView(Context context) {
            super(context);
        }

        @Override public void setCursor(Drawable cursor, float hotspotX, float hotspotY) {
            shapes++;
            this.hotspotX = hotspotX;
            super.setCursor(cursor, hotspotX, hotspotY);
        }
    }

    // An RGBA IMAGE message (docs/protocol/cursor-shape.md) of a 16x16 cursor at 100% scale.
    private static byte[] image(int id, int hotspotX) {
        int width = 16, height = 16;
        byte[] message = new byte[CursorShapeMessage.HEADER_BYTES + CursorShapeMessage.IMAGE_HEADER_BYTES + width * height * 4];
        message[0] = 1;
        message[1] = 3;
        message[4] = (byte) id;
        int[] fields = {width, height, hotspotX, 0};
        for (int i = 0; i < fields.length; i++) message[8 + i * 2] = (byte) fields[i];
        message[16] = (byte) CursorShapeMessage.FORMAT_RGBA8888;
        message[18] = 100;
        return message;
    }

    @Test public void aBurstOfShapesReachesTheScreenAsItsNewestOnly() throws Exception {
        Context context = RuntimeEnvironment.getApplication();
        CountingCursorView cursor = new CountingCursorView(context);
        TrackpadPointer pointer = new TrackpadPointer(null, new View(context), cursor, false, null, null);
        HostCursorShapes shapes = new HostCursorShapes(context.getResources(), cursor, pointer, () -> fail("rejected"));
        try {
            cursor.shapes = 0;
            for (int i = 0; i < 10; i++) shapes.accept(image(i + 1, i));
            CountDownLatch decoded = new CountDownLatch(1);
            ((Handler) ReflectionHelpers.getField(shapes, "decoder")).post(decoded::countDown);
            assertTrue(decoded.await(5, TimeUnit.SECONDS));
            ShadowLooper.idleMainLooper();
            assertEquals(1, cursor.shapes);
            assertEquals(9, cursor.hotspotX, 1e-3);
        } finally {
            shapes.close();
        }
    }

    @Test public void onlyANewCursorSizeLaysTheViewOutAgain() {
        Context context = RuntimeEnvironment.getApplication();
        LocalCursorView cursor = new LocalCursorView(context);
        cursor.measure(0, 0);
        cursor.layout(0, 0, cursor.getMeasuredWidth(), cursor.getMeasuredHeight());
        ColorDrawable small = new ColorDrawable() {
            @Override public int getIntrinsicWidth() { return 16; }
            @Override public int getIntrinsicHeight() { return 16; }
        };
        cursor.setCursor(small, 0, 0);
        assertTrue(cursor.isLayoutRequested());
        cursor.measure(0, 0);
        cursor.layout(0, 0, 16, 16);
        assertFalse(cursor.isLayoutRequested());

        ColorDrawable sameSize = new ColorDrawable() {
            @Override public int getIntrinsicWidth() { return 16; }
            @Override public int getIntrinsicHeight() { return 16; }
        };
        cursor.setCursor(sameSize, 2, 2);
        assertFalse(cursor.isLayoutRequested());
    }
}
