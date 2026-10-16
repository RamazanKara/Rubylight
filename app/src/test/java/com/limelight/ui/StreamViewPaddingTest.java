package com.limelight.ui;

import android.app.Application;
import android.graphics.Rect;
import android.view.View;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class StreamViewPaddingTest {
    private static StreamView laidOut(int width, int height) {
        StreamView view = new StreamView(RuntimeEnvironment.getApplication());
        view.initializeSurface(true);
        view.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        view.layout(0, 0, width, height);
        return view;
    }

    @Test
    public void uncroppedPaddingHangsOffTheRightAndIsClipped() {
        StreamView view = laidOut(2184, 1968);
        // AV1 from an AMD encoder: 2240 decoded columns carry a 2184-wide stream.
        view.setDecodedPadding(2240, 1968, 2184, 1968);
        view.measure(View.MeasureSpec.makeMeasureSpec(2184, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1968, View.MeasureSpec.EXACTLY));
        view.layout(0, 0, 2184, 1968);

        View video = view.getTextureView();
        assertEquals(2184, view.getWidth());
        assertEquals(1968, view.getHeight());
        assertEquals(0, video.getLeft());
        assertEquals(0, video.getTop());
        assertEquals(2240, video.getWidth());
        assertEquals(1968, video.getHeight());
        assertEquals(new Rect(0, 0, 2184, 1968), video.getClipBounds());
    }

    @Test
    public void croppedOutputFillsTheViewWithoutAClip() {
        StreamView view = laidOut(1080, 972);
        view.setDecodedPadding(2240, 1968, 2184, 1968);
        view.setDecodedPadding(2184, 1968, 2184, 1968);
        view.measure(View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(972, View.MeasureSpec.EXACTLY));
        view.layout(0, 0, 1080, 972);

        View video = view.getTextureView();
        assertEquals(1080, video.getWidth());
        assertEquals(972, video.getHeight());
        assertNull(video.getClipBounds());
    }
}
