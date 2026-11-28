package com.limelight.preferences;

import android.app.Application;
import android.app.Dialog;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.preference.Preference;
import android.preference.PreferenceManager;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;
import com.limelight.ui.AppTheme;
import com.limelight.ui.StageView;
import com.limelight.ui.VideoScaling;

import org.junit.After;
import org.junit.Assume;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.assertTrue;

/**
 * Optional pictures of the settings preview cards in each look: every picture with two of its choices as rows, the
 * pickers, and the Display &amp; image page with its cards. Set NIGHTJAR_PREVIEW_OUTPUT to capture PNGs.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, qualifiers = "en-w360dp-h800dp-port-night-xhdpi",
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class SettingsPreviewShotsTest {
    private static final int TALL = 6000;

    @After
    public void clear() {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
    }

    @Test
    public void night() throws IOException { capture("night"); }

    @Test @Config(qualifiers = "en-w360dp-h800dp-port-notnight-xhdpi")
    public void day() throws IOException { capture("day"); }

    @Test
    public void oled() throws IOException {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putString(AppTheme.THEME, AppTheme.THEME_OLED_BLACK).commit();
        capture("oled");
    }

    @Test
    public void highContrast() throws IOException {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putBoolean(AppTheme.HIGH_CONTRAST, true).commit();
        capture("hc");
    }

    @Test @Config(qualifiers = "de-w360dp-h800dp-port-night-xhdpi")
    public void german() throws IOException { capture("de"); }

    @Test @Config(qualifiers = "en-w960dp-h540dp-land-television-night-mdpi")
    public void tv() throws IOException { capture("tv"); }

    /** The preset chips where their names are longest, and on a phone held sideways. */
    @Test @Config(qualifiers = "fr-w360dp-h800dp-port-night-xhdpi")
    public void chipsFrench() throws IOException { capture("chips-fr"); }

    @Test @Config(qualifiers = "es-w360dp-h800dp-port-notnight-xhdpi")
    public void chipsSpanish() throws IOException { capture("chips-es"); }

    @Test @Config(qualifiers = "en-w720dp-h360dp-land-night-xhdpi")
    public void chipsSideways() throws IOException { capture("chips-land"); }

    @Test @Config(qualifiers = "en-w800dp-h360dp-land-notnight-xhdpi")
    public void phoneLandscapeVideoSize() throws IOException {
        String output = System.getenv("NIGHTJAR_PREVIEW_OUTPUT");
        Assume.assumeTrue("previews are an explicitly requested local capture", output != null && !output.isBlank());
        File directory = new File(output, "day");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        StreamSettings screen = Robolectric.buildActivity(StreamSettings.class).setup().visible().get();
        float density = screen.getResources().getDisplayMetrics().density;
        int width = Math.round(screen.getResources().getConfiguration().screenWidthDp * density);
        int height = Math.round(screen.getResources().getConfiguration().screenHeightDp * density);
        String[] sizes = screen.getResources().getStringArray(R.array.video_scaling_modes);
        Dialog dialog = SettingsPreviews.pick(screen, screen.getString(R.string.video_scaling_title),
                SettingsPreviewCard.SCALING, sizes, new float[] {0, 1, 2, 3, 4}, 0, null, which -> { });
        ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
        View decor = dialog.getWindow().getDecorView();
        decor.measure(View.MeasureSpec.makeMeasureSpec(Math.min(width, Math.round(560f * density)), View.MeasureSpec.AT_MOST),
                View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.AT_MOST));
        decor.layout(0, 0, decor.getMeasuredWidth(), decor.getMeasuredHeight());
        draw(decor, decor.getWidth(), decor.getHeight(), new File(directory, "phone-land-video-size.png"));
        dialog.dismiss();
    }

    @Test @Config(qualifiers = "en-w360dp-h800dp-port-notnight-xhdpi")
    public void controlsPinchToZoomDay() throws IOException {
        String output = System.getenv("NIGHTJAR_PREVIEW_OUTPUT");
        Assume.assumeTrue("previews are an explicitly requested local capture", output != null && !output.isBlank());
        File directory = new File(output, "day");
        assertTrue(directory.isDirectory() || directory.mkdirs());
        org.robolectric.Shadows.shadowOf(RuntimeEnvironment.getApplication().getPackageManager())
                .setSystemFeature(android.content.pm.PackageManager.FEATURE_TOUCHSCREEN, true);
        StreamSettings.rememberRail(RuntimeEnvironment.getApplication(), true);
        StreamSettings screen = Robolectric.buildActivity(StreamSettings.class).setup().visible().get();
        SettingsStructureTest.open(screen, "input");
        float density = screen.getResources().getDisplayMetrics().density;
        int width = Math.round(screen.getResources().getConfiguration().screenWidthDp * density);
        int height = Math.round(screen.getResources().getConfiguration().screenHeightDp * density);
        View root = screen.getWindow().getDecorView();
        for (int pass = 0; pass < 2; pass++) {
            root.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
            root.layout(0, 0, width, height);
            ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
        }
        draw(root, width, height, new File(directory, "settings-pinch-to-zoom.png"));
    }

    private void capture(String name) throws IOException {
        String output = System.getenv("NIGHTJAR_PREVIEW_OUTPUT");
        Assume.assumeTrue("previews are an explicitly requested local capture", output != null && !output.isBlank());
        File directory = new File(output);
        assertTrue(directory.isDirectory() || directory.mkdirs());
        StreamSettings.rememberRail(RuntimeEnvironment.getApplication(), true);
        StreamSettings screen = Robolectric.buildActivity(StreamSettings.class).setup().visible().get();
        float density = screen.getResources().getDisplayMetrics().density;
        int width = Math.round(screen.getResources().getConfiguration().screenWidthDp * density);
        String[] pacing = screen.getResources().getStringArray(R.array.frame_pacing_choice_names);
        String[] upscaling = screen.getResources().getStringArray(R.array.upscaling_names);
        String[] sizes = screen.getResources().getStringArray(R.array.video_scaling_modes);

        // The overview, with the Presets row and its chips.
        View overview = screen.getWindow().getDecorView();
        int tall = Math.round(screen.getResources().getConfiguration().screenHeightDp * density);
        for (int pass = 0; pass < 2; pass++) {
            overview.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(tall, View.MeasureSpec.EXACTLY));
            overview.layout(0, 0, width, tall);
            ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
        }
        draw(overview, width, tall, new File(directory, name + "-0-overview.png"));
        if (name.startsWith("chips")) return;

        // Every picture with two of its choices, as the rows that carry them.
        LinearLayout sheet = new LinearLayout(screen);
        sheet.setOrientation(LinearLayout.VERTICAL);
        sheet.setBackgroundColor(MaterialColors.getColor(screen, R.attr.glassCanvas, 0xFF000000));
        int edge = Math.round(12 * density);
        sheet.setPadding(edge, edge, edge, edge);
        row(sheet, screen.getString(R.string.title_frame_pacing), pacing[0], SettingsPreviewCard.PACING, PreviewPacing.LATENCY);
        row(sheet, screen.getString(R.string.title_frame_pacing), pacing[4], SettingsPreviewCard.PACING, PreviewPacing.SMOOTHNESS);
        row(sheet, screen.getString(R.string.title_enable_hdr), null, SettingsPreviewCard.TONE, PreviewTone.SDR);
        row(sheet, screen.getString(R.string.title_enable_hdr), null, SettingsPreviewCard.TONE, PreviewTone.HDR);
        row(sheet, screen.getString(R.string.title_enable_hdr), "Tone mapped (desktop)", SettingsPreviewCard.TONE, PreviewTone.TONE_MAPPED);
        row(sheet, "Upscaling", upscaling[1], SettingsPreviewCard.EDGE, PreviewEdge.softness("bilinear", 50));
        row(sheet, "Upscaling", upscaling[2] + " 80%", SettingsPreviewCard.EDGE, PreviewEdge.softness("fsr1", 80));
        row(sheet, screen.getString(R.string.video_scaling_title), sizes[VideoScaling.FIT], SettingsPreviewCard.SCALING, VideoScaling.FIT);
        row(sheet, screen.getString(R.string.video_scaling_title), sizes[VideoScaling.FILL], SettingsPreviewCard.SCALING, VideoScaling.FILL);
        row(sheet, screen.getString(R.string.video_scaling_title), sizes[VideoScaling.STRETCH], SettingsPreviewCard.SCALING, VideoScaling.STRETCH);
        row(sheet, screen.getString(R.string.video_scaling_title), sizes[VideoScaling.NATIVE], SettingsPreviewCard.SCALING, VideoScaling.NATIVE);
        write(sheet, width, new File(directory, name + "-1-rows.png"));

        // The pickers: one card per choice.
        float[] six = {0, 1, 2, 3, 4, 5};
        picker(SettingsPreviews.pick(screen, screen.getString(R.string.title_frame_pacing), SettingsPreviewCard.PACING,
                pacing, six, 1, () -> { }, which -> { }), width, new File(directory, name + "-2-pick-pacing.png"));
        picker(SettingsPreviews.pick(screen, "Upscaling", SettingsPreviewCard.EDGE, upscaling,
                SettingsPreviews.states(SettingsPreviews.UPSCALING, new String[] {"off", "bilinear", "fsr1", "sgsr1"}, 4, null),
                2, () -> { }, which -> { }), width, new File(directory, name + "-3-pick-upscaling.png"));
        picker(SettingsPreviews.pick(screen, screen.getString(R.string.video_scaling_title), SettingsPreviewCard.SCALING,
                sizes, new float[] {0, 1, 2, 3, 4}, 0, null, which -> { }), width, new File(directory, name + "-4-pick-size.png"));

        // The page that carries two of them.
        StreamSettings.SettingsFragment display = SettingsStructureTest.open(screen, "display");
        Preference size = display.findPreference(VideoScaling.PREF);
        assertTrue(size != null);
        View root = screen.getWindow().getDecorView();
        int height = Math.round(screen.getResources().getConfiguration().screenHeightDp * density * (name.equals("tv") ? 1 : 2));
        for (int pass = 0; pass < 2; pass++) {
            root.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
            root.layout(0, 0, width, height);
            ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
        }
        draw(root, width, height, new File(directory, name + "-5-display.png"));
    }

    private static void row(LinearLayout sheet, String title, String value, int kind, float state) {
        FrameLayout group = new FrameLayout(sheet.getContext());
        group.setBackgroundColor(StageView.groupFill(sheet.getContext()));
        View row = LayoutInflater.from(sheet.getContext()).inflate(R.layout.settings_preference_preview, group, false);
        ((TextView) row.findViewById(android.R.id.title)).setText(title);
        TextView summary = row.findViewById(android.R.id.summary);
        summary.setText(value);
        summary.setVisibility(value == null ? View.GONE : View.VISIBLE);
        if (value == null) row.findViewById(R.id.settings_preview_chevron).setVisibility(View.GONE);
        ((SettingsPreviewCard) row.findViewById(R.id.settings_preview_card)).show(null, kind, state);
        group.addView(row);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        params.bottomMargin = 12;
        sheet.addView(group, params);
    }

    private static void picker(Dialog picker, int width, File destination) throws IOException {
        ShadowLooper.idleMainLooper(400, TimeUnit.MILLISECONDS);
        assertTrue(picker == ShadowDialog.getLatestDialog());
        write(picker.getWindow().getDecorView(), width, destination);
        picker.dismiss();
    }

    private static void write(View root, int width, File destination) throws IOException {
        root.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(TALL, View.MeasureSpec.AT_MOST));
        root.layout(0, 0, width, root.getMeasuredHeight());
        draw(root, width, root.getMeasuredHeight(), destination);
    }

    private static void draw(View root, int width, int height, File destination) throws IOException {
        Bitmap page = Bitmap.createBitmap(width, Math.max(1, height), Bitmap.Config.ARGB_8888);
        try (FileOutputStream out = new FileOutputStream(destination)) {
            root.draw(new Canvas(page));
            assertTrue(page.compress(Bitmap.CompressFormat.PNG, 100, out));
        } finally {
            page.recycle();
        }
    }
}
