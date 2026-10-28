package com.limelight.preferences;

import android.app.Application;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.preference.Preference;
import android.view.View;
import android.widget.ListAdapter;
import android.widget.ListView;

import org.junit.After;
import org.junit.Assume;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

/**
 * Optional previews of the section folds: Video, Controls and Advanced on their main rows, and Advanced with its More
 * row open at the Experimental and Diagnostics groups. Set NIGHTJAR_PREVIEW_OUTPUT to a directory to capture PNGs.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class SettingsTiersPreviewTest {
    @After
    public void clear() {
        android.preference.PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
    }

    @Test @Config(qualifiers = "en-w360dp-h800dp-port-notnight-xhdpi")
    public void phone() throws IOException { capture("tiers-phone", true); }

    @Test @Config(qualifiers = "de-w360dp-h800dp-port-night-xhdpi")
    public void phoneGerman() throws IOException { capture("tiers-phone-de", true); }

    @Test @Config(qualifiers = "en-w960dp-h540dp-land-television-night-mdpi")
    public void tv() throws IOException { capture("tiers-tv", false); }

    private void capture(String name, boolean touch) throws IOException {
        File directory = outputDirectory();
        Application app = RuntimeEnvironment.getApplication();
        shadowOf(app.getPackageManager()).setSystemFeature(PackageManager.FEATURE_TOUCHSCREEN, touch);
        StreamSettings screen = Robolectric.buildActivity(StreamSettings.class).setup().visible().get();
        int width = Math.round(screen.getResources().getConfiguration().screenWidthDp * screen.getResources().getDisplayMetrics().density);
        int height = Math.round(screen.getResources().getConfiguration().screenHeightDp * screen.getResources().getDisplayMetrics().density);
        int shot = 1;
        for (String section : new String[] {"video", "input", "advanced"}) {
            screen.moreOpen.remove(section);
            screen.showSection(section);
            SettingsStructureTest.settle(screen);
            StreamSettings.SettingsFragment fragment = (StreamSettings.SettingsFragment) SettingsStructureTest.shown(screen);
            // The bottom of the list: the last main rows and the More row, whatever the pane height.
            scrollTo(screen, fragment, StreamSettings.MORE_PREFIX + section, width, height, -1);
            write(screen.getWindow().getDecorView(), width, height, new File(directory, name + "-" + shot++ + "-" + section + ".png"));
        }
        StreamSettings.SettingsFragment advanced = (StreamSettings.SettingsFragment) SettingsStructureTest.shown(screen);
        Preference more = advanced.findPreference("more_advanced");
        more.getOnPreferenceClickListener().onPreferenceClick(more);
        SettingsStructureTest.settle(screen);
        scrollTo(screen, advanced, "checkbox_rate_change_bitrate", width, height, 1);
        write(screen.getWindow().getDecorView(), width, height, new File(directory, name + "-" + shot + "-advanced-open.png"));
    }

    /** Scrolls the section's list so {@code key} sits {@code above} rows below the top, or at the bottom for -1. */
    private static void scrollTo(StreamSettings screen, StreamSettings.SettingsFragment fragment, String key, int width,
                                 int height, int above) {
        layout(screen.getWindow().getDecorView(), width, height);
        ListView list = fragment.getView().findViewById(android.R.id.list);
        Preference target = fragment.findPreference(key);
        assertNotNull(key, target);
        ListAdapter adapter = list.getAdapter();
        int position = -1;
        for (int i = 0; i < adapter.getCount(); i++) {
            if (adapter.getItem(i) == target) position = i;
        }
        assertTrue(key + " is listed", position >= 0);
        list.setSelectionFromTop(above < 0 ? position : Math.max(0, position - above), 0);
        layout(screen.getWindow().getDecorView(), width, height);
        if (above < 0) {
            list.setSelectionFromTop(position, list.getHeight() - list.getChildAt(0).getHeight());
            layout(screen.getWindow().getDecorView(), width, height);
        }
    }

    private static File outputDirectory() {
        String output = System.getenv("NIGHTJAR_PREVIEW_OUTPUT");
        Assume.assumeTrue("Native previews are an explicitly requested local capture", output != null && !output.isBlank());
        File directory = new File(output);
        assertTrue(directory.isDirectory() || directory.mkdirs());
        return directory;
    }

    private static void layout(View view, int width, int height) {
        ShadowLooper.idleMainLooper();
        view.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
        view.layout(0, 0, width, height);
        ShadowLooper.idleMainLooper();
    }

    private static void write(View view, int width, int height, File destination) throws IOException {
        layout(view, width, height);
        Bitmap bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
        try (FileOutputStream output = new FileOutputStream(destination)) {
            view.draw(new Canvas(bitmap));
            assertTrue(bitmap.compress(Bitmap.CompressFormat.PNG, 100, output));
        } finally {
            bitmap.recycle();
        }
        assertTrue("A nonempty PNG was written", destination.length() > 1000);
    }
}
