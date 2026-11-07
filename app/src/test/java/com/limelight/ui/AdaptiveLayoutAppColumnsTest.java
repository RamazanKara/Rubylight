package com.limelight.ui;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public class AdaptiveLayoutAppColumnsTest {
    @Test
    public void aPhoneHasThreeColumnsAndATvSeven() {
        for (float width : new float[] {320, 360, 393, 411, 480}) assertEquals(3, AdaptiveLayout.appColumns(GlassForm.PHONE, width, false));
        for (float width : new float[] {960, 1280, 1920}) assertEquals(7, AdaptiveLayout.appColumns(GlassForm.TELEVISION, width, false));
    }

    @Test
    public void aHandheldAndAnOpenFoldKeepTheCoversSizeAndTakeMore() {
        assertEquals(5, AdaptiveLayout.appColumns(GlassForm.HANDHELD, 640, false));
        assertEquals(6, AdaptiveLayout.appColumns(GlassForm.HANDHELD, 960, false));
        assertEquals(5, AdaptiveLayout.appColumns(GlassForm.PHONE, 673, false));
    }

    @Test
    public void smallIconsAddColumns() {
        assertEquals(4, AdaptiveLayout.appColumns(GlassForm.PHONE, 393, true));
        assertEquals(9, AdaptiveLayout.appColumns(GlassForm.TELEVISION, 960, true));
        assertEquals(8, AdaptiveLayout.appColumns(GlassForm.HANDHELD, 960, true));
    }
}
