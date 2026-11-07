package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import android.app.Activity;
import android.content.DialogInterface;
import android.graphics.drawable.ColorDrawable;
import android.os.Looper;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.utils.UiHelper;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;
import java.util.stream.Stream;

/**
 * A TV's Home options were the old dialog with a grey highlight, and every dialog's button bar turned grey when a pad opened it.
 * One builder, one panel, one moon, one red: on every form.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = android.app.Application.class, qualifiers = "w800dp-h400dp-land")
public class DialogGlassTest {
    private Activity activity;

    @Before
    public void theme() {
        activity = Robolectric.buildActivity(Activity.class).setup().get();
        activity.setTheme(R.style.AppTheme);
    }

    private AlertDialog confirm() {
        return new NavigationDialogBuilder(activity).setTitle("Title").setMessage("Message")
                .setPositiveButton("Go", null).setNegativeButton("Cancel", null).show();
    }

    private static int backgroundOf(Button button) {
        return Shadows.shadowOf(button.getBackground()).getCreatedFromResId();
    }

    @Test
    public void thePanelIsGlass() {
        AlertDialog dialog = confirm();
        View panel = dialog.getWindow().getDecorView();
        assertTrue("the window background carries the glass panel, inset or not",
                describes(dialog.getWindow().getDecorView().getBackground()));
        assertNotNull(panel);
    }

    private static boolean describes(android.graphics.drawable.Drawable background) {
        if (background instanceof GlassPanelDrawable) return true;
        if (background instanceof android.graphics.drawable.InsetDrawable) {
            return describes(((android.graphics.drawable.InsetDrawable) background).getDrawable());
        }
        return false;
    }

    @Test
    public void oneMoonOneControl() {
        AlertDialog dialog = confirm();
        assertEquals(R.drawable.dialog_button_moon, backgroundOf(dialog.getButton(DialogInterface.BUTTON_POSITIVE)));
        assertEquals(R.drawable.dialog_button_control, backgroundOf(dialog.getButton(DialogInterface.BUTTON_NEGATIVE)));
    }

    @Test
    public void aDestructiveDialogIsRedAndDoesNotStartOnTheRed() {
        AlertDialog dialog = confirm();
        UiHelper.markDestructive(dialog);
        Button confirm = dialog.getButton(DialogInterface.BUTTON_POSITIVE);
        assertEquals(R.drawable.dialog_button_danger, backgroundOf(confirm));
        assertFalse("a pad starts on the way out", confirm.isFocusedByDefault());
        assertTrue(dialog.getButton(DialogInterface.BUTTON_NEGATIVE).isFocusedByDefault());
    }

    @Test
    public void aPadStartsOnTheMoonNotOnTheBar() {
        AlertDialog dialog = confirm();
        Button positive = dialog.getButton(DialogInterface.BUTTON_POSITIVE);
        assertTrue(positive.isFocusedByDefault());
        View scroller = (View) positive.getParent().getParent();
        assertFalse("the bar's scroller must not take focus: its highlight greys the whole bar", scroller.isFocusable());
        assertFalse(scroller.getDefaultFocusHighlightEnabled());
    }

    @Test
    public void aListHasTheRowFocusAndKeepsTheFirstFocus() {
        AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle("Title").setItems(new CharSequence[] {"One", "Two"}, null)
                .setNegativeButton("Cancel", null).show();
        assertEquals(R.drawable.dialog_row_focus, Shadows.shadowOf(dialog.getListView().getSelector()).getCreatedFromResId());
        assertFalse(dialog.getButton(DialogInterface.BUTTON_NEGATIVE).isFocusedByDefault());
    }

    @Test
    public void aDialogSetAfterItsBuilderStillHasItsButtons() {
        // The builder creates the views for the capsule bar, so a later setButton would be lost: nothing may do that.
        com.limelight.utils.Dialog.displayDialog(activity, "Title", "Message", false);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        android.app.Dialog shown = org.robolectric.shadows.ShadowDialog.getLatestDialog();
        Button ok = ((AlertDialog) shown).getButton(DialogInterface.BUTTON_POSITIVE);
        assertEquals(View.VISIBLE, ok.getVisibility());
        assertEquals(activity.getString(android.R.string.ok), ok.getText().toString());
        assertEquals(View.VISIBLE, ((AlertDialog) shown).getButton(DialogInterface.BUTTON_NEUTRAL).getVisibility());
        com.limelight.utils.Dialog.closeDialogs(activity);
    }

    @Test
    public void aSheetIsGlassWithAGrabberOffTv() {
        ActionSheet sheet = new ActionSheet(activity, "Sheet");
        sheet.addAction(R.drawable.ic_close, "Row");
        sheet.show();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        ViewGroup sheetView = sheet.findViewById(com.google.android.material.R.id.design_bottom_sheet);
        sheetView.layout(0, 0, 800, 400);
        assertTrue(sheetView.getBackground() instanceof GlassPanelDrawable);
        ViewGroup body = (ViewGroup) sheetView.getChildAt(0);
        assertFalse("not a colour", body.getChildAt(0).getBackground() instanceof ColorDrawable);
        assertEquals(Shadows.shadowOf(body.getChildAt(0).getBackground()).getCreatedFromResId(), R.drawable.glass_grabber);
    }

    @Test
    @Config(qualifiers = "w800dp-h400dp-land-notnight")
    public void aDialogOverAStreamIsNightOnADaySystem() {
        Activity stream = Robolectric.buildActivity(Activity.class).setup().get();
        stream.setTheme(R.style.StreamTheme);
        assertFalse(GlassPanelDrawable.isDay(stream));
        assertTrue(GlassPanelDrawable.isDay(activity));
        // Inflating resolves every theme attribute the capsules and rows use: the stream's theme has to carry them too.
        AlertDialog dialog = new NavigationDialogBuilder(stream).setTitle("Stream").setMessage("Ended")
                .setPositiveButton("Reconnect", null).setNegativeButton("Disconnect", null).show();
        assertEquals(R.drawable.dialog_button_moon, backgroundOf(dialog.getButton(DialogInterface.BUTTON_POSITIVE)));
        new NavigationDialogBuilder(stream).setItems(new CharSequence[] {"One"}, null).show();
    }

    @Test
    public void everyDialogComesFromTheOneBuilder() throws Exception {
        File java = null;
        for (String path : new String[] {"src/main/java", "app/src/main/java", "client/app/src/main/java"}) {
            if (new File(path).isDirectory()) java = new File(path);
        }
        assertNotNull(java);
        List<String> raw = new ArrayList<>();
        try (Stream<java.nio.file.Path> files = Files.walk(java.toPath())) {
            files.filter(p -> p.toString().endsWith(".java") && !p.endsWith("NavigationDialogBuilder.java")).forEach(p -> {
                try {
                    String text = new String(Files.readAllBytes(p), StandardCharsets.UTF_8);
                    if (text.contains("new MaterialAlertDialogBuilder(") || text.contains("new AlertDialog.Builder(")) raw.add(p.toString());
                } catch (java.io.IOException e) {
                    throw new AssertionError(e);
                }
            });
        }
        assertEquals("dialogs built past the glass panel: " + raw, 0, raw.size());
    }
}
