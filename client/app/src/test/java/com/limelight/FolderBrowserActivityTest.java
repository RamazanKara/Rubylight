package com.limelight;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;

import android.app.Application;

import com.limelight.nvstream.http.SharedFolders;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;

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
    }

    @Test
    public void aDownloadGetsTheTypeOfItsExtension() {
        Shadows.shadowOf(android.webkit.MimeTypeMap.getSingleton()).addExtensionMimeTypeMapping("png", "image/png");
        assertEquals("image/png", FolderBrowserActivity.mimeType("Shot.PNG"));
        assertEquals("application/octet-stream", FolderBrowserActivity.mimeType("save.unknownext"));
        assertEquals("application/octet-stream", FolderBrowserActivity.mimeType("README"));
    }
}
