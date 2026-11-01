package com.limelight.files;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotSame;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageInfo;
import android.content.pm.ProviderInfo;
import android.net.Uri;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import com.limelight.PcViewLifecycleTest;
import com.limelight.R;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.FileTransferBridge;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.shadows.ShadowToast;
import org.robolectric.util.ReflectionHelpers;

import java.util.function.BooleanSupplier;

/**
 * The file screen through a configuration change (dark theme, font or display size, language), which recreates
 * it, and through the app being closed while the save dialog was open.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class FileTransferRecreationTest {
    private static final HostEndpoint HOST = new HostEndpoint("pc", "10.0.0.2", 47989, 47984, "pem", "client",
            new byte[0], new byte[0]);
    private static final Uri SHARED = Uri.parse("content://com.example.media/file/1");

    private final Application app = RuntimeEnvironment.getApplication();

    /** A transfer list with nothing in it that counts how it is ended. */
    private static final class Engine implements FileTransferEngine {
        int cancelAlls;
        boolean destroyed;

        @Override public long create() { return 7; }
        @Override public void destroy(long handle) { destroyed = true; }
        @Override public long queueUpload(long handle, String localName, long size) { return -1; }
        @Override public long queueDownload(long handle, String name, String localName, long size) { return -1; }
        @Override public boolean cancel(long handle, long id) { return false; }
        @Override public void cancelAll(long handle) { cancelAlls++; }
        @Override public boolean retry(long handle, long id) { return false; }
        @Override public void clearFinished(long handle) {}
        @Override public String snapshot(long handle) { return "[]"; }
        @Override public String startNext(long handle) { return "[]"; }
        @Override public String list(HostEndpoint host) { return "{}"; }
        @Override public String run(long handle, long id, HostEndpoint host, int fd,
                                    FileTransferBridge.Progress progress) { return "{}"; }
    }

    @Before
    public void anotherAppSharesFiles() {
        PackageInfo media = new PackageInfo();
        media.packageName = "com.example.media";
        media.applicationInfo = new ApplicationInfo();
        media.applicationInfo.packageName = media.packageName;
        Shadows.shadowOf(app.getPackageManager()).installPackage(media);
        ProviderInfo provider = new ProviderInfo();
        provider.authority = SHARED.getAuthority();
        provider.packageName = media.packageName;
        provider.name = "com.example.media.Files";
        provider.applicationInfo = media.applicationInfo;
        Shadows.shadowOf(app.getPackageManager()).addOrUpdateProvider(provider);
    }

    @Test
    public void aConfigurationChangeKeepsTheTransfersAndSendsAShareOnce() {
        Intent share = new Intent(app, FileTransferActivity.class).setAction(Intent.ACTION_SEND)
                .putExtra(Intent.EXTRA_STREAM, SHARED);
        ActivityController<FileTransferActivity> screen = Robolectric.buildActivity(FileTransferActivity.class, share)
                .setup();
        FileTransferActivity.TransferWork work = screen.get().work;
        assertEquals(1, work.waiting.size());
        Engine engine = new Engine();
        work.engine = engine;
        work.controller = new FileTransferController(engine, HOST, work.workers, work.handler::post, work);
        FileTransferActivity first = screen.get();

        screen.recreate();
        assertNotSame(first, screen.get());
        assertSame(work, screen.get().work);
        assertSame(screen.get(), work.screen);
        assertEquals("nothing was cancelled", 0, engine.cancelAlls);
        assertFalse(engine.destroyed);
        assertEquals("the share is not taken again", 1, work.waiting.size());

        // Leaving the screen for good cancels what has not finished.
        screen.get().finish();
        screen.pause().stop().destroy();
        assertEquals(1, engine.cancelAlls);
        assertTrue(engine.destroyed);
        assertTrue(work.workers.isShutdown());
    }

    @Test
    public void aSaveDialogThatOutlivedTheAppDeletesItsDocumentAndSaysSo() throws InterruptedException {
        ActivityController<FileTransferActivity> screen = Robolectric.buildActivity(FileTransferActivity.class,
                FileTransferActivity.forPc(app, "tower")).setup();
        FileTransferActivity activity = screen.get();
        // Recreated after the app died: no download waits for the document, and no PC is known yet.
        Uri document = Uri.parse("content://com.example.nowhere/document/9");
        activity.onCreated(document);
        assertEquals(app.getString(R.string.files_save_lost), ShadowToast.getTextOfLatestToast());
        // This provider cannot delete it: the screen names what is left instead of only logging it.
        settle(() -> !activity.work.leftovers.isEmpty());
        assertEquals(1, activity.work.leftovers.size());
        String left = app.getString(R.string.files_partial_left, activity.work.leftovers.get(0));
        assertTrue(left, shows(activity.getWindow().getDecorView(), left));
        // A cancelled dialog leaves nothing.
        ShadowToast.reset();
        activity.onCreated(null);
        assertEquals(null, ShadowToast.getTextOfLatestToast());
        screen.get().finish();
        screen.pause().stop().destroy();
    }

    @Test
    public void aFailedDownloadWhoseDocumentStaysIsNamed() throws InterruptedException {
        ActivityController<FileTransferActivity> screen = Robolectric.buildActivity(FileTransferActivity.class,
                FileTransferActivity.forPc(app, "tower")).setup();
        FileTransferActivity.TransferWork work = screen.get().work;
        Uri document = Uri.parse("content://com.example.nowhere/document/10");
        assertFalse(FileTransferActivity.TransferWork.deleteDocument(app.getContentResolver(), document));
        work.workers.execute(work.discard(document, "save.bin"));
        settle(() -> !work.leftovers.isEmpty());
        String left = app.getString(R.string.files_partial_left, "save.bin");
        assertTrue(shows(screen.get().getWindow().getDecorView(), left));
        // It goes with Clear finished.
        View clear = find(screen.get().getWindow().getDecorView(), app.getString(R.string.files_clear_finished));
        assertEquals(View.VISIBLE, clear.getVisibility());
        clear.performClick();
        assertTrue(work.leftovers.isEmpty());
        assertFalse(shows(screen.get().getWindow().getDecorView(), left));
        screen.get().finish();
        screen.pause().stop().destroy();
    }

    @Test
    public void aShareWithoutAPcListsTheOnlinePcsByName() {
        Intent share = new Intent(app, FileTransferActivity.class).setAction(Intent.ACTION_SEND)
                .putExtra(Intent.EXTRA_STREAM, SHARED);
        ActivityController<FileTransferActivity> screen = Robolectric.buildActivity(FileTransferActivity.class, share)
                .setup();
        screen.get().work.engine = new Engine();
        ComputerDetails tower = NativeApiFixtures.nativeHost("tower-uuid", "Tower", true);
        // Every share crashed here: the PC's button was labelled with the string resource 0.
        ReflectionHelpers.callInstanceMethod(screen.get(), "onComputer",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, tower));
        assertTrue(shows(screen.get().getWindow().getDecorView(), "Tower"));
        screen.get().finish();
        screen.pause().stop().destroy();
    }

    @Test
    public void aPollThatChangesNothingKeepsThePcButtonsSoAPressLands() {
        Intent share = new Intent(app, FileTransferActivity.class).setAction(Intent.ACTION_SEND)
                .putExtra(Intent.EXTRA_STREAM, SHARED);
        ActivityController<FileTransferActivity> screen = Robolectric.buildActivity(FileTransferActivity.class, share)
                .setup();
        FileTransferActivity activity = screen.get();
        activity.work.engine = new Engine();
        View root = activity.getWindow().getDecorView();
        poll(activity, NativeApiFixtures.nativeHost("tower-uuid", "Tower", true));
        View button = find(root, "Tower");
        button.setFocusableInTouchMode(true);
        assertTrue(button.requestFocus());
        button.setPressed(true);
        // Every PC is polled every 1.5 s: the buttons were made again each time, so a remote's OK or a tap that
        // was pressed on one was let go on a button no longer on the screen, and nothing happened (issue 56).
        poll(activity, NativeApiFixtures.nativeHost("tower-uuid", "Tower", true));
        assertSame(button, find(root, "Tower"));
        assertTrue(button.isAttachedToWindow());
        assertTrue(button.isPressed());
        assertTrue(button.isFocused());
        // Another PC coming online makes the buttons again; the focus stays on the PC it was on.
        poll(activity, NativeApiFixtures.nativeHost("desk-uuid", "Desk", true));
        assertTrue(shows(root, "Desk"));
        View again = find(root, "Tower");
        assertTrue(again.isFocused());
        again.performClick();
        assertEquals("tower-uuid", activity.work.uuid);
        screen.get().finish();
        screen.pause().stop().destroy();
    }

    @Test
    public void progressKeepsATransfersButtonsAndUpdatesItsText() {
        ActivityController<FileTransferActivity> screen = Robolectric.buildActivity(FileTransferActivity.class,
                FileTransferActivity.forPc(app, "tower")).setup();
        FileTransferActivity activity = screen.get();
        Engine engine = new Engine();
        activity.work.engine = engine;
        activity.work.controller = new FileTransferController(engine, HOST, activity.work.workers,
                activity.work.handler::post, activity.work);
        View root = activity.getWindow().getDecorView();
        String cancelLabel = app.getString(android.R.string.cancel);
        String head = "{\"id\":3,\"direction\":\"upload\",\"name\":\"log.txt\",\"local_name\":\"log.txt\",\"size\":100,";
        activity.onTransfersChanged(FileRow.parse("[" + head + "\"state\":\"running\",\"done\":10,\"total\":100}]"));
        View cancel = find(root, cancelLabel);
        assertTrue(cancel != null);
        // Progress comes many times a second; a Cancel made again each time could not be pressed.
        FileRow half = FileRow.parse("[" + head + "\"state\":\"running\",\"done\":50,\"total\":100}]").get(0);
        activity.onTransfersChanged(java.util.Collections.singletonList(half));
        assertSame(cancel, find(root, cancelLabel));
        assertTrue(cancel.isAttachedToWindow());
        assertTrue(shows(root, FileTransferActivity.describe(app, half)));
        // A finished transfer has no Cancel any more.
        activity.onTransfersChanged(FileRow.parse("[" + head + "\"state\":\"done\",\"saved_name\":\"log.txt\"}]"));
        assertEquals(null, find(root, cancelLabel));
        assertTrue(shows(root, app.getString(R.string.files_sent)));
        screen.get().finish();
        screen.pause().stop().destroy();
    }

    @Test
    public void theReportAliasTakesNightjarsOwnReportAndTheSharedScreenDoesNot() {
        Uri report = com.limelight.utils.ProblemReport.reportUri();
        Intent intent = com.limelight.utils.ProblemReport.sendToPcIntent(app, report);
        assertTrue(FileTransferActivity.forReport(intent));
        ActivityController<FileTransferActivity> screen = Robolectric.buildActivity(FileTransferActivity.class, intent)
                .setup();
        assertEquals("the report waits for a PC", 1, screen.get().work.waiting.size());
        assertEquals("a report also asks which PC", null, screen.get().work.uuid);
        screen.get().finish();
        screen.pause().stop().destroy();
        // The alias sends the report only, not the clipboard image its provider also serves.
        Uri clipboard = report.buildUpon().path("/clipboard/image.png").build();
        ActivityController<FileTransferActivity> image = Robolectric.buildActivity(FileTransferActivity.class,
                com.limelight.utils.ProblemReport.sendToPcIntent(app, clipboard)).setup();
        assertTrue(image.get().work.waiting.isEmpty());
        image.get().finish();
        image.pause().stop().destroy();
        // The same report shared to the exported screen is refused: another app could send Nightjar's files so.
        Intent shared = new Intent(app, FileTransferActivity.class).setAction(Intent.ACTION_SEND)
                .putExtra(Intent.EXTRA_STREAM, report);
        assertFalse(FileTransferActivity.forReport(shared));
        ActivityController<FileTransferActivity> other = Robolectric.buildActivity(FileTransferActivity.class, shared)
                .setup();
        assertTrue(other.get().work.waiting.isEmpty());
        other.get().finish();
        other.pause().stop().destroy();
    }

    @Test
    public void theReportAliasIsNotExportedAndOpensTheFileScreen() throws Exception {
        android.content.pm.ActivityInfo alias = app.getPackageManager().getActivityInfo(
                new android.content.ComponentName(app, FileTransferActivity.REPORT_ALIAS), 0);
        assertFalse("only Nightjar may open the alias that sends its own report", alias.exported);
        assertEquals(FileTransferActivity.class.getName(), alias.targetActivity);
    }

    @Test
    public void aFocusedButtonThatGoesAwayHandsTheFocusOnInsteadOfToTheUpButton() {
        ActivityController<FileTransferActivity> screen = Robolectric.buildActivity(FileTransferActivity.class,
                FileTransferActivity.forPc(app, "tower")).setup();
        FileTransferActivity activity = screen.get();
        Engine engine = new Engine();
        activity.work.engine = engine;
        activity.work.controller = new FileTransferController(engine, HOST, activity.work.workers,
                activity.work.handler::post, activity.work);
        View root = activity.getWindow().getDecorView();
        String a = "{\"id\":1,\"direction\":\"upload\",\"name\":\"a\",\"local_name\":\"a\",\"size\":100,";
        String b = "{\"id\":2,\"direction\":\"upload\",\"name\":\"b\",\"local_name\":\"b\",\"size\":100,";
        String running = "\"state\":\"running\",\"done\":10,\"total\":100}";
        String failed = "\"state\":\"failed\",\"problem\":\"network\",\"message\":\"x\",\"can_retry\":true}";
        String queued = "\"state\":\"queued\"}";
        String done = "\"state\":\"done\",\"saved_name\":\"a\"}";

        // Try again on a failed upload queues it: the row's Cancel takes the focus.
        activity.onTransfersChanged(FileRow.parse("[" + a + failed + "]"));
        View retry = root.findViewWithTag("retry:1");
        assertTrue(retry != null && retry.requestFocus());
        activity.onTransfersChanged(FileRow.parse("[" + a + queued + "]"));
        assertTrue(root.findViewWithTag("cancel:1").isFocused());

        // A focused Cancel whose upload finishes during a multi-file share: the next row's Cancel takes it.
        activity.onTransfersChanged(FileRow.parse("[" + a + running + "," + b + running + "]"));
        assertTrue(root.findViewWithTag("cancel:1").requestFocus());
        activity.onTransfersChanged(FileRow.parse("[" + a + done + "," + b + running + "]"));
        assertTrue(root.findViewWithTag("cancel:2").isFocused());

        // The last one finishing leaves no button in the list: Clear finished takes it, not the Up button.
        activity.onTransfersChanged(FileRow.parse("[" + a + done + "," + b + "\"state\":\"done\",\"saved_name\":\"b\"}]"));
        View clear = find(root, app.getString(R.string.files_clear_finished));
        assertTrue(clear.isFocused());
        screen.get().finish();
        screen.pause().stop().destroy();
    }

    @Test
    public void aFocusedPcThatGoesAwayHandsTheFocusToItsNeighbour() {
        Intent share = new Intent(app, FileTransferActivity.class).setAction(Intent.ACTION_SEND)
                .putExtra(Intent.EXTRA_STREAM, SHARED);
        ActivityController<FileTransferActivity> screen = Robolectric.buildActivity(FileTransferActivity.class, share)
                .setup();
        FileTransferActivity activity = screen.get();
        activity.work.engine = new Engine();
        View root = activity.getWindow().getDecorView();
        poll(activity, NativeApiFixtures.nativeHost("tower-uuid", "Tower", true));
        poll(activity, NativeApiFixtures.nativeHost("desk-uuid", "Desk", true));
        assertTrue(find(root, "Desk").requestFocus());
        poll(activity, NativeApiFixtures.nativeHost("desk-uuid", "Desk", false));
        assertFalse(shows(root, "Desk"));
        assertTrue(find(root, "Tower").isFocused());
        screen.get().finish();
        screen.pause().stop().destroy();
    }

    @Test
    public void theFolderReadAgainKeepsItsDownloadButtonsAndTheirFocus() {
        ActivityController<FileTransferActivity> screen = Robolectric.buildActivity(FileTransferActivity.class,
                FileTransferActivity.forPc(app, "tower")).setup();
        FileTransferActivity activity = screen.get();
        ReflectionHelpers.setField(activity, "downloadAccess", FileProblems.Access.READY);
        // Shown once the PC lets this device download; no PC answers here.
        ((View) ReflectionHelpers.getField(activity, "files")).setVisibility(View.VISIBLE);
        View root = activity.getWindow().getDecorView();
        String head = "{\"ok\":{\"folder\":\"Downloads\",\"free_bytes\":1000,\"max_file_bytes\":500,\"files\":[";
        String a = "{\"name\":\"a.txt\",\"size\":3,\"modified_unix_ms\":2000}";
        String b = "{\"name\":\"b.txt\",\"size\":4,\"modified_unix_ms\":1000}";
        showFiles(activity, RemoteFiles.parse(head + a + "," + b + "]}}"));
        View download = root.findViewWithTag("download:b.txt");
        assertTrue(download.requestFocus());
        // Read again after every upload and on Check again: the same files keep their buttons.
        showFiles(activity, RemoteFiles.parse(head + a + "," + b + "]}}"));
        assertSame(download, root.findViewWithTag("download:b.txt"));
        assertTrue(download.isFocused());
        // A new file makes the rows again; the focus stays on the same file's Download.
        String c = "{\"name\":\"c.txt\",\"size\":5,\"modified_unix_ms\":3000}";
        showFiles(activity, RemoteFiles.parse(head + c + "," + a + "," + b + "]}}"));
        assertTrue(root.findViewWithTag("download:b.txt").isFocused());
        // The focused file deleted on the PC: its neighbour's Download takes the focus.
        showFiles(activity, RemoteFiles.parse(head + c + "," + a + "]}}"));
        assertTrue(root.findViewWithTag("download:a.txt").isFocused());
        screen.get().finish();
        screen.pause().stop().destroy();
    }

    private static void showFiles(FileTransferActivity activity, RemoteFiles list) {
        ReflectionHelpers.callInstanceMethod(activity, "showFiles",
                ReflectionHelpers.ClassParameter.from(RemoteFiles.class, list));
    }

    private static void poll(FileTransferActivity activity, ComputerDetails details) {
        ReflectionHelpers.callInstanceMethod(activity, "onComputer",
                ReflectionHelpers.ClassParameter.from(ComputerDetails.class, details));
    }

    private static boolean shows(View view, String text) {
        return find(view, text) != null;
    }

    private static View find(View view, String text) {
        if (view instanceof TextView && text.contentEquals(((TextView) view).getText())) return view;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                View found = find(group.getChildAt(i), text);
                if (found != null) return found;
            }
        }
        return null;
    }

    /** Runs the main looper until a worker's result has been posted. */
    private static void settle(BooleanSupplier done) throws InterruptedException {
        long until = System.currentTimeMillis() + 5000;
        while (System.currentTimeMillis() < until) {
            ShadowLooper.idleMainLooper();
            if (done.getAsBoolean()) return;
            Thread.sleep(10);
        }
    }
}
