package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowBuild;

/** A probe's verdict stands for this app on this OS build, not for ever. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 33, application = Application.class)
public class GlassTierStoreTest {
    private final Context context = RuntimeEnvironment.getApplication();

    @Test
    public void anOsUpdateProbesAgain() {
        ShadowBuild.setFingerprint("vendor/device:13/one");
        assertFalse(GlassTier.probed(context));
        GlassTier.remember(context, GlassTier.BAKED);
        assertTrue(GlassTier.probed(context));
        assertEquals(GlassTier.BAKED, GlassTier.land(context));

        ShadowBuild.setFingerprint("vendor/device:14/two");
        assertFalse("the driver may have changed", GlassTier.probed(context));
        assertEquals("nothing holds the new build back until its probe", GlassTier.LENS, GlassTier.land(context));
    }

    @Test
    public void onlyTheLatestVerdictIsKept() {
        ShadowBuild.setFingerprint("vendor/device:13/one");
        GlassTier.remember(context, GlassTier.BAKED);
        ShadowBuild.setFingerprint("vendor/device:14/two");
        GlassTier.remember(context, GlassTier.LIVE);
        assertEquals(1, context.getSharedPreferences("glass_tier", Context.MODE_PRIVATE).getAll().size());
    }
}
