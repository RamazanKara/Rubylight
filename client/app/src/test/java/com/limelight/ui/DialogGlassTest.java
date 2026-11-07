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

    /**
     * On the real TV the pairing dialog stood over the PCs page's title and card with the page only a little darker, so they
     * read as cut behind it. A TV's night page goes to half brightness behind a dialog; elsewhere it keeps the sheet's dim.
     */
    @Test
    @Config(qualifiers = "w960dp-h540dp-land-television-xhdpi")
    public void aTvDialogDimsThePageToHalf() {
        AlertDialog dialog = confirm();
        assertEquals(0.5f, dialog.getWindow().getAttributes().dimAmount, 0.001f);
    }

    @Test
    public void offTvADialogKeepsTheSheetsDim() {
        AlertDialog dialog = confirm();
        assertEquals(GlassPanelDrawable.backdropDim(activity), dialog.getWindow().getAttributes().dimAmount, 0.001f);
    }

    @Test
    public void oneMoonOneControl() {
        AlertDialog dialog = confirm();
        assertEquals(R.drawable.dialog_button_moon, backgroundOf(dialog.getButton(DialogInterface.BUTTON_POSITIVE)));
        assertEquals(R.drawable.dialog_button_control, backgroundOf(dialog.getButton(DialogInterface.BUTTON_NEGATIVE)));
    }

    /** Day's Approve, off, passed for Deny beside it; night's nearly vanished. Off, the moon's ink gives way to the dimmed one. */
    @Test
    public void aMoonButtonThatIsOffLooksOff() {
        Button moon = confirm().getButton(DialogInterface.BUTTON_POSITIVE);
        assertEquals(activity.getColor(R.color.nj_color_on_moon), moon.getCurrentTextColor());
        moon.setEnabled(false);
        assertEquals("dimmed", 0x61, android.graphics.Color.alpha(moon.getCurrentTextColor()));
        assertTrue("the page's ink, not the moon's",
                (moon.getCurrentTextColor() & 0xFFFFFF) != (activity.getColor(R.color.nj_color_on_moon) & 0xFFFFFF));
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
    public void aCostlyConfirmationStartsOnCancelStillInMoon() {
        AlertDialog dialog = new NavigationDialogBuilder(activity).startOnCancel().setMessage("Message")
                .setPositiveButton("Go", null).setNegativeButton("Cancel", null).show();
        assertFalse(dialog.getButton(DialogInterface.BUTTON_POSITIVE).isFocusedByDefault());
        assertTrue(dialog.getButton(DialogInterface.BUTTON_NEGATIVE).isFocusedByDefault());
        assertEquals(R.drawable.dialog_button_moon, backgroundOf(dialog.getButton(DialogInterface.BUTTON_POSITIVE)));
    }

    /** Every confirmation that ends or loses something, by the words of its builder: none may lose the first focus to the moon. */
    @Test
    public void everyCostlyConfirmationAsksForCancelFirst() throws java.io.IOException {
        String[][] costly = {
                {"computers/HostActions.java", "R.string.host_sleep_title"},
                {"computers/HostActions.java", "R.string.host_unpair_title"},
                {"computers/HostActions.java", ".setPositiveButton(confirm,"},
                {"console/GameDetailActivity.java", ".setMessage(message)"},
                {"console/ReplaceRunningPrompt.java", ".setMessage(running == null"},
                {"GameMenuActions.java", "R.string.stream_command_confirm"},
                {"GameMenuActions.java", ".setTitle(title)"},
                {"AppView.java", "R.string.library_import_controls_replace"},
                {"AppView.java", "R.string.library_replace_message_named"},
                {"pin/PinLockSettingsActivity.java", "R.string.pin_remove_title"},
                {"preferences/StreamProfilesActivity.java", "R.string.profiles_delete_title"},
                {"preferences/StreamProfilesActivity.java", "R.string.profiles_home_remove_title"},
                {"preferences/HostStreamSettings.java", ".setTitle(reset)"},
                {"preferences/KeyboardShortcutsActivity.java", "R.string.keyboard_shortcuts_import"},
                {"ui/DesktopControlsPanel.java", "R.string.desktop_controls_reset_title"},
                {"ui/GameTouchControls.java", ".setTitle(R.string.touch_layout_import)"},
        };
        for (String[] site : costly) {
            String source = new String(java.nio.file.Files.readAllBytes(
                    java.nio.file.Paths.get("src/main/java/com/limelight", site[0])), java.nio.charset.StandardCharsets.UTF_8);
            int at = source.indexOf(site[1]);
            assertTrue(site[1] + " in " + site[0], at >= 0);
            int builder = source.lastIndexOf("NavigationDialogBuilder(", at);
            assertTrue(site[1] + " in " + site[0] + " is built here", builder >= 0);
            String head = source.substring(builder, source.indexOf('\n', builder));
            assertTrue(site[0] + ": " + site[1] + " starts on Cancel", head.contains(".startOnCancel()"));
        }
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
