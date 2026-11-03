package com.limelight;

import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.graphics.Bitmap;
import android.net.Uri;
import android.os.ParcelFileDescriptor;

import com.limelight.grid.assets.DiskAssetLoader;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.io.File;
import java.io.FileNotFoundException;
import java.io.FileOutputStream;
import java.nio.file.Files;
import java.nio.file.StandardCopyOption;

/** The posters the TV home screen's channel shows for a PC's games. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class)
public class PosterContentProviderTest {
    private static final String PC = "0123abcd-0000-0000-0000-000000000001";

    private static File cached(String appId) {
        File file = new DiskAssetLoader(RuntimeEnvironment.getApplication()).getFile(PC, appId);
        assertTrue(file.getParentFile().isDirectory() || file.getParentFile().mkdirs());
        return file;
    }

    @Test
    public void aGameWithItsOwnCoverHasAPosterAndTheHostsPlaceholderIsNone() throws Exception {
        PosterContentProvider provider = Robolectric.buildContentProvider(PosterContentProvider.class).create().get();

        Bitmap cover = Bitmap.createBitmap(60, 90, Bitmap.Config.ARGB_8888);
        try (FileOutputStream out = new FileOutputStream(cached("11"))) {
            cover.compress(Bitmap.CompressFormat.PNG, 100, out);
        }
        try (ParcelFileDescriptor poster = provider.openBoxArtFile(PosterContentProvider.createBoxArtUri(PC, "11"), "r")) {
            assertNotNull(poster);
        }

        Files.copy(new File("../protocol/client/tests/fixtures/box-placeholder.png").toPath(), cached("12").toPath(),
                StandardCopyOption.REPLACE_EXISTING);
        Uri grey = PosterContentProvider.createBoxArtUri(PC, "12");
        assertThrows(FileNotFoundException.class, () -> provider.openBoxArtFile(grey, "r"));
        assertThrows(FileNotFoundException.class,
                () -> provider.openBoxArtFile(PosterContentProvider.createBoxArtUri(PC, "13"), "r"));
    }
}
