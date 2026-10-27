package com.limelight;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.content.pm.FeatureInfo;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

/**
 * Nightjar's place in a TV's app row: the launcher entry a Leanback launcher lists, the app banner it shows there, and
 * the 320x180 picture behind it. A missing banner makes the TV crop the round icon; a missing entry hides the app.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "television-xhdpi",
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class LeanbackBannerTest {
    private final Context context = RuntimeEnvironment.getApplication();

    @Test
    public void theLeanbackLauncherOpensPcView() {
        Intent main = new Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LEANBACK_LAUNCHER).setPackage(context.getPackageName());
        boolean found = false;
        for (ResolveInfo info : context.getPackageManager().queryIntentActivities(main, 0)) {
            ActivityInfo activity = info.activityInfo;
            if (activity.name.equals(PcView.class.getName())) {
                found = true;
                assertTrue("the TV launcher can start it", activity.exported);
            }
        }
        assertTrue("PcView is in the Leanback launcher", found);
    }

    @Test
    public void theAppDeclaresTheBannerAndTvFeaturesItCanRunWithout() throws Exception {
        assertEquals("the app's banner", R.drawable.atv_banner, context.getApplicationInfo().banner);
        PackageInfo info = context.getPackageManager().getPackageInfo(context.getPackageName(), PackageManager.GET_CONFIGURATIONS);
        assertNotNull(info.reqFeatures);
        boolean leanback = false;
        for (FeatureInfo feature : info.reqFeatures) {
            if (PackageManager.FEATURE_LEANBACK.equals(feature.name)) {
                leanback = true;
                assertFalse("phones install it too", (feature.flags & FeatureInfo.FLAG_REQUIRED) != 0);
            }
            // A TV has no touchscreen: requiring one would hide the app from TV stores.
            assertFalse(feature.name + " is not required",
                    PackageManager.FEATURE_TOUCHSCREEN.equals(feature.name) && (feature.flags & FeatureInfo.FLAG_REQUIRED) != 0);
        }
        assertTrue("the app says it runs on Leanback", leanback);
    }

    @Test
    public void theBannerIs320By180() {
        BitmapFactory.Options options = new BitmapFactory.Options();
        options.inScaled = false;
        Bitmap banner = BitmapFactory.decodeResource(context.getResources(), R.drawable.atv_banner, options);
        assertNotNull("the banner decodes", banner);
        assertEquals(320, banner.getWidth());
        assertEquals(180, banner.getHeight());
        // At xhdpi the TV draws it at its own size, 160x90 dp: never stretched from another density.
        assertEquals(160, Math.round(banner.getWidth() / context.getResources().getDisplayMetrics().density));
        banner.recycle();
    }
}
