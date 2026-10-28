package com.limelight.preferences;

import android.Manifest;
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
 * Optional previews of the settings rows for the modern controller and the microphone: Input with
 * the controller style, stick and dead zone rows, Audio with the microphone button and processing
 * rows saying "Not on this device". Set NIGHTJAR_PREVIEW_OUTPUT to a directory to capture PNGs.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class MicAndControllerSettingsPreviewTest {
    @After
    public void clear() {
        android.preference.PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
    }

    @Test @Config(qualifiers = "en-w360dp-h800dp-port-notnight-xhdpi")
    public void phoneLight() throws IOException { capture("settings-phone-light"); }

    @Test @Config(qualifiers = "en-w360dp-h800dp-port-night-xhdpi")
    public void phoneDark() throws IOException { capture("settings-phone-dark"); }

    @Test @Config(qualifiers = "en-w1280dp-h800dp-land-notnight-mdpi")
    public void tabletLight() throws IOException { capture("settings-tablet-light"); }

    @Test @Config(qualifiers = "en-w1280dp-h800dp-land-night-mdpi")
    public void tabletDark() throws IOException { capture("settings-tablet-dark"); }

    private void capture(String name) throws IOException {
        File directory = outputDirectory();
        Application app = RuntimeEnvironment.getApplication();
        shadowOf(app.getPackageManager()).setSystemFeature(PackageManager.FEATURE_TOUCHSCREEN, true);
        shadowOf(app).grantPermissions(Manifest.permission.RECORD_AUDIO);
        android.preference.PreferenceManager.getDefaultSharedPreferences(app).edit()
                .putBoolean("checkbox_show_onscreen_controls", true)
                .putString(PreferenceConfiguration.OSC_STYLE_PREF_STRING, PreferenceConfiguration.OSC_STYLE_MODERN)
                .putBoolean("checkbox_enable_mic", true)
                .putBoolean(PreferenceConfiguration.MIC_AUTO_GAIN_PREF_STRING, true)
                .commit();
        StreamSettings screen = Robolectric.buildActivity(StreamSettings.class).setup().visible().get();
        int width = Math.round(screen.getResources().getConfiguration().screenWidthDp * screen.getResources().getDisplayMetrics().density);
        int height = Math.round(screen.getResources().getConfiguration().screenHeightDp * screen.getResources().getDisplayMetrics().density);

        StreamSettings.SettingsFragment input = SettingsStructureTest.open(screen, "input");
        scrollTo(screen, input, PreferenceConfiguration.OSC_STYLE_PREF_STRING, width, height);
        write(screen.getWindow().getDecorView(), width, height, new File(directory, name + "-1-input.png"));

        StreamSettings.SettingsFragment audio = SettingsStructureTest.open(screen, "audio");
        scrollTo(screen, audio, "checkbox_enable_mic", width, height);
        Preference noise = audio.findPreference(PreferenceConfiguration.MIC_NOISE_PREF_STRING);
        if (!com.limelight.binding.audio.MicEffects.DEVICE.has(com.limelight.binding.audio.MicProcessing.Effect.NOISE_SUPPRESSOR)) {
            assertEquals("Not on this device", String.valueOf(noise.getSummary()));
        }
        write(screen.getWindow().getDecorView(), width, height, new File(directory, name + "-2-audio.png"));
    }

    /** Scrolls the section's list so {@code key}'s category header sits near the top. */
    private static void scrollTo(StreamSettings screen, StreamSettings.SettingsFragment fragment, String key, int width, int height) {
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
        list.setSelectionFromTop(Math.max(0, position - 1), 0);
        layout(screen.getWindow().getDecorView(), width, height);
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
