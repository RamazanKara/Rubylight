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
