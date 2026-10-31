package com.limelight.grid.assets;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;

import android.app.Application;
import android.graphics.Bitmap;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;

/** The real decoder on a real 3840x1080 PNG: the bitmap the cache holds is the banner's middle 800x450. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
public class DiskAssetLoaderDecodeTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    @Test
    public void aWideBannerIsDecodedToItsMiddle800By450() {
        Bitmap banner = Bitmap.createBitmap(3840, 1080, Bitmap.Config.ARGB_8888);
        // Red sides, a green middle a little wider than the kept 960..2880: a crop from either edge would show red.
        banner.eraseColor(0xffff0000);
        android.graphics.Canvas middle = new android.graphics.Canvas(banner);
        middle.clipRect(900, 0, 2940, 1080);
        middle.drawColor(0xff00ff00);
        ByteArrayOutputStream png = new ByteArrayOutputStream();
        banner.compress(Bitmap.CompressFormat.PNG, 100, png);

        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "test-pc";
        CachedAppAssetLoader.LoaderTuple tuple = new CachedAppAssetLoader.LoaderTuple(computer, new NvApp("Banner", 1, false));
        DiskAssetLoader loader = new DiskAssetLoader(temporary.getRoot(), false);
        loader.populateCacheWithStream(tuple, new ByteArrayInputStream(png.toByteArray()));

        ScaledBitmap decoded = loader.loadBitmapFromCache(tuple, 1);
        assertNotNull(decoded);
        assertEquals(3840, decoded.originalWidth);
        assertEquals(1080, decoded.originalHeight);
        assertEquals(800, decoded.bitmap.getWidth());
        assertEquals(450, decoded.bitmap.getHeight());
        Bitmap readable = decoded.bitmap.getConfig() == Bitmap.Config.HARDWARE
                ? decoded.bitmap.copy(Bitmap.Config.ARGB_8888, false) : decoded.bitmap;
        assertEquals(0xff00ff00, readable.getPixel(2, 225));
        assertEquals(0xff00ff00, readable.getPixel(797, 225));
    }
}
