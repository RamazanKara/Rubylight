package com.limelight.preferences;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.content.Context;
import android.content.DialogInterface;
import android.os.Looper;
import android.view.View;
import android.view.ViewGroup;

import androidx.appcompat.app.AlertDialog;
import androidx.appcompat.app.AppCompatActivity;

import com.google.android.material.button.MaterialButtonToggleGroup;
import com.google.android.material.slider.Slider;
import com.google.android.material.textfield.TextInputEditText;
import com.limelight.R;
import com.limelight.binding.input.GyroMappingPreferences;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;

import java.util.ArrayList;
import java.util.List;

/** The gyroscope profile editor builds on the app's dialog theme and saves only a valid profile. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = android.app.Application.class)
public class GyroProfileEditorTest {
    public static final class Screen extends AppCompatActivity {
        @Override protected void onCreate(android.os.Bundle saved) {
            setTheme(R.style.AppTheme);
            super.onCreate(saved);
        }
    }

    private Context context;

    @After public void clear() {
        if (context != null) {
            context.getSharedPreferences("nightjar_gyro_mapping", Context.MODE_PRIVATE).edit().clear().commit();
        }
    }

    private static <T extends View> List<T> find(View root, Class<T> type) {
        List<T> found = new ArrayList<>();
        if (type.isInstance(root)) found.add(type.cast(root));
        if (root instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) root;
            for (int i = 0; i < group.getChildCount(); i++) found.addAll(find(group.getChildAt(i), type));
        }
        return found;
    }

    private AlertDialog openEditor(ActivityController<Screen> controller) {
        context = controller.get();
        GyroProfileEditor.editProfile(controller.get(), null, null);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
        assertTrue(dialog.isShowing());
        return dialog;
    }

    private static void save(AlertDialog dialog) {
        dialog.getButton(DialogInterface.BUTTON_POSITIVE).performClick();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
    }

    @Test public void aNewProfileStartsAsStandardAndSavesWithItsName() {
        try (ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup()) {
            AlertDialog dialog = openEditor(controller);
            View root = dialog.getWindow().getDecorView();
            assertEquals(3, find(root, MaterialButtonToggleGroup.class).size());
            List<Slider> sliders = find(root, Slider.class);
            assertEquals(2, sliders.size());
            assertEquals(100f, sliders.get(0).getValue(), 0f);

            save(dialog);
            assertTrue("a profile needs a name", dialog.isShowing());
            assertTrue(GyroMappingPreferences.profiles(context).isEmpty());

            find(root, TextInputEditText.class).get(0).setText("Mine");
            sliders.get(0).setValue(150f);
            save(dialog);
            assertFalse(dialog.isShowing());
            GyroMappingPreferences.Profile saved = GyroMappingPreferences.profiles(context).get(0);
            assertEquals("Mine", saved.name);
            assertArrayEquals(new int[] {0, 1, 2}, saved.axes);
            assertArrayEquals(new int[] {1, 1, 1}, saved.signs);
            assertEquals(1.5f, saved.horizontalSensitivity, 0f);
            assertEquals(1f, saved.verticalSensitivity, 0f);
        }
    }

    @Test public void anAxisUsedTwiceIsNotSaved() {
        try (ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup()) {
            AlertDialog dialog = openEditor(controller);
            View root = dialog.getWindow().getDecorView();
            find(root, TextInputEditText.class).get(0).setText("Twice");
            List<MaterialButtonToggleGroup> axes = find(root, MaterialButtonToggleGroup.class);
            // Up and down (X) takes Y, which Left and right still has.
            axes.get(0).check(axes.get(0).getChildAt(1).getId());
            save(dialog);
            assertTrue(dialog.isShowing());
            assertTrue(GyroMappingPreferences.profiles(context).isEmpty());
        }
    }
}
