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
    public void aCoverIsDecodedInItsOwnShapeNotSqueezedIntoAPoster() {
        assertArrayEquals("a banner stays a banner", new int[] {360, 168}, DiskAssetLoader.targetSize(920, 430));
        assertArrayEquals("a square stays square", new int[] {360, 360}, DiskAssetLoader.targetSize(600, 600));
        assertArrayEquals("2:3 fills the box", new int[] {360, 540}, DiskAssetLoader.targetSize(600, 900));
        assertArrayEquals("3:4 is limited by its width", new int[] {360, 480}, DiskAssetLoader.targetSize(600, 800));
        assertArrayEquals("a small cover is not enlarged", new int[] {200, 300}, DiskAssetLoader.targetSize(200, 300));
        assertArrayEquals("unknown sizes fall back to the box", new int[] {360, 540}, DiskAssetLoader.targetSize(0, 0));
    }
}
