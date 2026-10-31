package com.limelight.grid.assets;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;

import static org.junit.Assert.*;

public class DiskAssetLoaderTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private CachedAppAssetLoader.LoaderTuple tuple() {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "test-pc";
        return new CachedAppAssetLoader.LoaderTuple(computer, new NvApp("App", 1, false));
    }

    @Test
    public void cacheBecomesVisibleOnlyAfterDownloadCompletes() {
        DiskAssetLoader loader = new DiskAssetLoader(temporary.getRoot(), false);
        CachedAppAssetLoader.LoaderTuple tuple = tuple();
        loader.populateCacheWithStream(tuple, new ByteArrayInputStream(new byte[] {1, 2, 3}) {
            @Override public synchronized int read(byte[] bytes, int offset, int length) {
                assertFalse(loader.checkCacheExists(tuple));
                return super.read(bytes, offset, length);
            }
        });
        assertTrue(loader.checkCacheExists(tuple));
    }

    @Test
    public void failedDownloadDoesNotDestroyAnExistingCachedImage() throws Exception {
        DiskAssetLoader loader = new DiskAssetLoader(temporary.getRoot(), false);
        CachedAppAssetLoader.LoaderTuple tuple = tuple();
        byte[] original = new byte[] {1, 2, 3};
        loader.populateCacheWithStream(tuple, new ByteArrayInputStream(original));
        loader.populateCacheWithStream(tuple, new InputStream() {
            @Override public int read() throws IOException { throw new IOException("Connection reset"); }
        });
        assertArrayEquals(original, Files.readAllBytes(loader.getFile(tuple.computer.uuid, tuple.cacheKey).toPath()));
        assertEquals(1, loader.getFile(tuple.computer.uuid, tuple.cacheKey).getParentFile().list().length);
    }

    @Test
    public void aCoverIsDecodedInItsOwnShapeLargeEnoughToCoverThePosterItIsCroppedTo() {
        assertArrayEquals("a banner stays a banner, as tall as the poster allows", new int[] {920, 430},
                DiskAssetLoader.targetSize(920, 430));
        assertArrayEquals("a wide banner keeps the poster's height", new int[] {1600, 450},
                DiskAssetLoader.targetSize(3840, 1080));
        assertArrayEquals("a square stays square and covers the box", new int[] {450, 450}, DiskAssetLoader.targetSize(600, 600));
        assertArrayEquals("2:3 fills the box", new int[] {300, 450}, DiskAssetLoader.targetSize(600, 900));
        assertArrayEquals("3:4 is limited by its height", new int[] {338, 450}, DiskAssetLoader.targetSize(600, 800));
        assertArrayEquals("a small cover is not enlarged", new int[] {200, 300}, DiskAssetLoader.targetSize(200, 300));
        assertArrayEquals("unknown sizes fall back to the box", new int[] {300, 450}, DiskAssetLoader.targetSize(0, 0));
    }

    /**
     * A 3840x1080 banner covers the poster box at 1600x450, about 2.9 MB, and shrank the byte-sized memory cache. Only its
     * middle 800x450 is kept: the poster's 300x450 crop and a 16:9 frame's 800x450 crop both stay at full resolution.
     */
    @Test
    public void aWideBannerKeepsOnlyTheMiddleAnyFrameShowsAtFullResolution() {
        int[] target = DiskAssetLoader.targetSize(3840, 1080);
        int[] kept = DiskAssetLoader.keptRegion(target[0], target[1]);
        assertArrayEquals(new int[] {400, 0, 1200, 450}, kept);
        int keptWidth = kept[2] - kept[0];
        int keptHeight = kept[3] - kept[1];
        assertEquals(DiskAssetLoader.MAX_DECODED_SIDE, keptWidth);
        assertTrue("half the bytes or less", keptWidth * keptHeight * 2 <= target[0] * target[1]);
        assertEquals("as sharp as the poster's crop needs", 450, keptHeight);
        assertTrue("a 16:9 frame's crop fits whole", keptWidth >= Math.round(keptHeight * 16f / 9));

        assertArrayEquals("a tall cover keeps its middle too", new int[] {0, 133, 300, 933},
                DiskAssetLoader.keptRegion(300, 1067));
        assertArrayEquals("a poster is kept whole", new int[] {0, 0, 300, 450}, DiskAssetLoader.keptRegion(300, 450));
        assertArrayEquals("a smaller banner is kept whole", new int[] {0, 0, 764, 430}, DiskAssetLoader.keptRegion(764, 430));
    }
}
