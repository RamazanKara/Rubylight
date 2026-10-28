package com.limelight;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;

import android.app.Application;
import android.net.Uri;

import com.limelight.nvstream.http.SharedFolders;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;

import java.lang.reflect.Constructor;
import java.util.function.BooleanSupplier;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class FolderBrowserActivityTest {
    @Test
    public void everyRefusalHasAMessageAndAMissingWriteGrantSaysSo() {
        for (SharedFolders.Refusal refusal : SharedFolders.Refusal.values()) {
            FolderBrowserActivity.refusalMessage(refusal, false);
            FolderBrowserActivity.refusalMessage(refusal, true);
        }
        assertNotEquals(FolderBrowserActivity.refusalMessage(SharedFolders.Refusal.FORBIDDEN, false),
                FolderBrowserActivity.refusalMessage(SharedFolders.Refusal.FORBIDDEN, true));
        assertEquals(R.string.folders_busy, FolderBrowserActivity.refusalMessage(SharedFolders.Refusal.TRANSFER_BUSY, true));
        // A folder Windows denies the PC's user is named, not a generic host failure.
        assertEquals(R.string.folders_unreadable,
                FolderBrowserActivity.refusalMessage(SharedFolders.Refusal.FOLDER_UNREADABLE, false));
    }

    @Test
    public void aDownloadGetsTheTypeOfItsExtension() {
        Shadows.shadowOf(android.webkit.MimeTypeMap.getSingleton()).addExtensionMimeTypeMapping("png", "image/png");
        assertEquals("image/png", FolderBrowserActivity.mimeType("Shot.PNG"));
        assertEquals("application/octet-stream", FolderBrowserActivity.mimeType("save.unknownext"));
        assertEquals("application/octet-stream", FolderBrowserActivity.mimeType("README"));
    }

    @Test
    public void aPathThisSideRefusesIsShownNotACrash() throws Exception {
        Application app = RuntimeEnvironment.getApplication();
        FolderBrowserActivity.FolderWork work = new FolderBrowserActivity.FolderWork(app);
        // The path is refused before any request, so no connection is made.
        work.connector = () -> null;
        work.open(folder("games"), "saves//slot1");
        settle(() -> !work.loading && work.error != null);
        assertEquals(app.getString(R.string.folders_bad_path, "Not a shared folder path: \"saves//slot1\""), work.error);
        assertFalse(work.canRetry);
    }

    @Test
    public void aPickedDocumentThatCannotBeDeletedIsNamed() throws Exception {
        Application app = RuntimeEnvironment.getApplication();
        Uri document = Uri.parse("content://com.example.nowhere/document/7");
        String left = FolderBrowserActivity.FolderWork.discard(app.getContentResolver(), document, true);
        assertEquals(document.toString(), left);

        FolderBrowserActivity.FolderWork work = new FolderBrowserActivity.FolderWork(app);
        work.dropPicked(document, app.getString(R.string.folders_save_lost));
        settle(() -> !work.loading && work.error != null);
        assertEquals(app.getString(R.string.folders_partial_left, app.getString(R.string.folders_save_lost),
                document.toString()), work.error);
        assertFalse(work.canRetry);
    }

    @Test
    public void aSavePickedBeforeThePcIsKnownIsDroppedWithAWordIfThePcStaysUnknown() throws Exception {
        Application app = RuntimeEnvironment.getApplication();
        FolderBrowserActivity.FolderWork work = new FolderBrowserActivity.FolderWork(app);
        Uri document = Uri.parse("content://com.example.nowhere/document/8");
        work.download("games", "saves/slot1.sav", document);
        assertEquals(null, work.transfer);
        work.fail("The PC is not paired");
        work.dropDeferred();
        settle(() -> !"The PC is not paired".equals(work.error));
        assertEquals(app.getString(R.string.folders_partial_left, "The PC is not paired", document.toString()),
                work.error);
    }

    private static SharedFolders.Folder folder(String id) throws Exception {
        Constructor<SharedFolders.Folder> constructor =
                SharedFolders.Folder.class.getDeclaredConstructor(String.class, String.class, boolean.class);
        constructor.setAccessible(true);
        return constructor.newInstance(id, id, true);
    }

    /** Runs the main looper until the work's background step has posted its result. */
    private static void settle(BooleanSupplier done) throws InterruptedException {
        long until = System.currentTimeMillis() + 5000;
        while (System.currentTimeMillis() < until) {
            ShadowLooper.idleMainLooper();
            if (done.getAsBoolean()) return;
            Thread.sleep(10);
        }
    }
}
