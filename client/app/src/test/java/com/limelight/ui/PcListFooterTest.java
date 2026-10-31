package com.limelight.ui;

import static org.junit.Assert.*;

import android.app.Activity;
import android.app.Application;
import android.graphics.Color;
import android.view.LayoutInflater;
import android.widget.FrameLayout;

import com.google.android.material.progressindicator.CircularProgressIndicator;
import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class PcListFooterTest {
    @Test
    public void theSearchingSpinnerAlwaysShowsAWholeRingNeverADot() {
        try (var controller = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible()) {
            Activity activity = controller.get();
            FrameLayout host = new FrameLayout(activity);
            CircularProgressIndicator spinner = LayoutInflater.from(activity).inflate(R.layout.activity_pc_view, host, false)
                    .findViewById(R.id.pc_list_spinner);
            float dp = activity.getResources().getDisplayMetrics().density;
            assertTrue("at least 16dp across", spinner.getIndicatorSize() >= Math.round(16 * dp));
            // The running arc can be caught at its shortest; the track under it is drawn the whole time.
            assertNotEquals("the track is drawn", Color.TRANSPARENT, spinner.getTrackColor());
            assertTrue("the track is visible", Color.alpha(spinner.getTrackColor()) > 0);
            assertEquals("no gap cuts the ring", 0, spinner.getIndicatorTrackGapSize());
        }
    }
}
