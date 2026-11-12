package com.limelight.update;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.DialogInterface;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.drawable.Drawable;
import android.os.Bundle;
import android.os.Looper;
import android.text.Layout;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowManager;
import android.widget.FrameLayout;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;
import androidx.core.graphics.ColorUtils;

import com.limelight.R;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.ui.NightSkyView;
import com.limelight.ui.NightjarActivity;
import com.limelight.utils.UiHelper;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

import java.time.Duration;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * nightjar#7: on a Shield the update's "11 MB di 30 MB" was black, at the platform's smallest size, on the night glass under the
 * dialog's headline: a bare TextView, which the app's screens give only a face. The label is a dialog message's text, the bar
 * keeps the dialog's padding, the buttons follow the bar, the title keeps to the panel, and on a TV the dialog stands on the sky.
 */
@RunWith(RobolectricTestRunner.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = 30, application = Application.class)
public class UpdateDownloadDialogTest {
    private static final String VERSION = "5.1.0";

    /** A Settings screen, where the download starts: compact on a TV, themed as every app screen. */
    public static class Settings extends NightjarActivity {
        @Override protected boolean compactOnTelevision() {
            return true;
        }

        @Override protected void onCreate(Bundle state) {
            super.onCreate(state);
            setContentView(new FrameLayout(this));
        }
    }

    @Test
    @Config(qualifiers = "it-w360dp-h740dp-port-night-xhdpi")
    public void onAPhoneTheProgressReadsAsADialogMessageOverItsPage() {
        try (ActivityController<Settings> controller = Robolectric.buildActivity(Settings.class).setup().visible()) {
            Settings screen = controller.get();
            ViewGroup content = screen.findViewById(android.R.id.content);
            AtomicInteger cancelled = new AtomicInteger();
            UpdateFlow.DownloadDialog download = show(screen, cancelled::incrementAndGet);
            assertEquals("over its dimmed page, as every phone dialog", 1, content.getChildCount());
            assertReadsAsADialog(screen, download);

            download.dialog.getButton(DialogInterface.BUTTON_NEGATIVE).performClick();
            frame();
            assertEquals("Cancel stops the download", 1, cancelled.get());
            assertFalse(download.dialog.isShowing());
        }
    }

    @Test
    @Config(qualifiers = "it-w960dp-h540dp-land-television-xhdpi")
    public void onATvTheProgressStandsOnTheSkyAndReadsAsADialogMessage() {
        try (ActivityController<Settings> controller = Robolectric.buildActivity(Settings.class).setup().visible()) {
            Settings screen = controller.get();
            ViewGroup content = screen.findViewById(android.R.id.content);
            View page = content.getChildAt(0);
            UpdateFlow.DownloadDialog download = show(screen, () -> { });
            assertEquals("the sky covers Settings", 2, content.getChildCount());
            assertTrue(content.getChildAt(1) instanceof NightSkyView);
            assertEquals(View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS, page.getImportantForAccessibility());
            assertEquals("no dim over the sky", 0,
                    download.dialog.getWindow().getAttributes().flags & WindowManager.LayoutParams.FLAG_DIM_BEHIND);
            assertReadsAsADialog(screen, download);

            download.dialog.dismiss();
            frame();
            assertEquals("Settings is back", 1, content.getChildCount());
        }
    }

    private static UpdateFlow.DownloadDialog show(Settings screen, Runnable cancel) {
        UpdateFlow.DownloadDialog download = UpdateFlow.downloadDialog(screen, VERSION, cancel);
        UiHelper.showDialog(screen, download.dialog);
        frame();
        download.status.setText(screen.getString(R.string.update_progress, "11 MB", "30 MB"));
        frame();
        return download;
    }

    private static void assertReadsAsADialog(Settings screen, UpdateFlow.DownloadDialog download) {
        AlertDialog dialog = download.dialog;
        TextView status = download.status;
        View bar = download.bar;
        View decor = dialog.getWindow().getDecorView();
        assertTrue("laid out", decor.getWidth() > 0 && bar.getWidth() > 0);

        // The offer before it, as every dialog is made: its message and headline are what this dialog's must be.
        AlertDialog offer = new NavigationDialogBuilder(screen).setTitle(R.string.update_available_title)
                .setMessage(screen.getString(R.string.update_available_message, VERSION, "5.0.0"))
                .setNegativeButton(R.string.update_action_later, null).create();
        TextView message = offer.findViewById(android.R.id.message);
        TextView headline = offer.findViewById(androidx.appcompat.R.id.alertTitle);

        // The label: a message's type and ink, readable on the glass behind it.
        assertEquals("a dialog message's size", message.getTextSize(), status.getTextSize(), 0.01f);
        assertEquals("a dialog message's ink", message.getCurrentTextColor(), status.getCurrentTextColor());
        int glass = panelBehind(decor, status);
        assertEquals("the panel is behind the label", 0xFF, Color.alpha(glass));
        double ratio = ColorUtils.calculateContrast(status.getCurrentTextColor(), glass);
        assertTrue(String.format("the label reads on the glass: %.2f:1", ratio), ratio >= 4.5);

        // The title: never above a dialog's headline, whole and inside the panel, on two lines where one is too short.
        TextView title = dialog.findViewById(androidx.appcompat.R.id.alertTitle);
        assertEquals(screen.getString(R.string.update_downloading_title, VERSION), title.getText().toString());
        assertTrue("a dialog's headline at most: " + title.getTextSize(), title.getTextSize() <= headline.getTextSize() + 0.01f);
        Layout lines = title.getLayout();
        assertEquals("the title is not cut off", 0, lines.getEllipsisCount(lines.getLineCount() - 1));
        float room = title.getWidth() - title.getTotalPaddingLeft() - title.getTotalPaddingRight();
        for (int line = 0; line < lines.getLineCount(); line++) {
            assertTrue("title line " + line + " fits", lines.getLineWidth(line) <= room + 0.5f);
        }
        View titlePanel = (View) title.getParent();
        assertTrue("the title keeps the panel's padding", title.getRight() <= titlePanel.getWidth() - titlePanel.getPaddingRight());

        // The label and the bar: on the dialog's padding, under the title's start.
        int edge = screen.getResources().getDimensionPixelSize(R.dimen.space_xl);
        View body = (View) bar.getParent();
        assertEquals("the bar starts on the padding", edge, bar.getLeft());
        assertEquals("and ends on it", body.getWidth() - edge, bar.getRight());
        assertEquals("the label starts with the bar", bar.getLeft(), status.getLeft());
        assertEquals("both under the title's start", inWindow(title)[0], inWindow(bar)[0]);

        // No empty field between the bar and the buttons.
        View cancel = dialog.getButton(DialogInterface.BUTTON_NEGATIVE);
        int gap = inWindow(cancel)[1] - (inWindow(bar)[1] + bar.getHeight());
        assertTrue("the buttons follow the bar: " + gap + "px", gap >= 0 && gap <= edge);
    }

    /** The window's glass at a view's centre, drawn as the window draws its background. */
    private static int panelBehind(View decor, View view) {
        Bitmap glass = Bitmap.createBitmap(decor.getWidth(), decor.getHeight(), Bitmap.Config.ARGB_8888);
        Drawable panel = decor.getBackground();
        panel.setBounds(0, 0, decor.getWidth(), decor.getHeight());
        panel.draw(new Canvas(glass));
        int[] at = inWindow(view);
        return glass.getPixel(at[0] + view.getWidth() / 2, at[1] + view.getHeight() / 2);
    }

    private static int[] inWindow(View view) {
        int[] at = new int[2];
        view.getLocationInWindow(at);
        return at;
    }

    /** A window's first traversal attaches its views; it waits for the next frame. */
    private static void frame() {
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(100));
    }
}
