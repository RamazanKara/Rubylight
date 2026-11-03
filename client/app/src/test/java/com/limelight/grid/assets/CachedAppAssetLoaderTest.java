package com.limelight.grid.assets;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.drawable.Drawable;
import android.widget.ImageView;
import android.widget.TextView;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class CachedAppAssetLoaderTest {
    private CachedAppAssetLoader loader(CountDownLatch started, CountDownLatch release) {
        Application context = RuntimeEnvironment.getApplication();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "artwork-regression";
        Bitmap bitmap = Bitmap.createBitmap(2, 2, Bitmap.Config.ARGB_8888);
        DiskAssetLoader disk = new DiskAssetLoader(context) {
            @Override public ScaledBitmap loadBitmapFromCache(CachedAppAssetLoader.LoaderTuple tuple, int sampleSize) {
                started.countDown();
                try {
                    if (!release.await(5, TimeUnit.SECONDS)) throw new AssertionError("Artwork test timed out");
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
                return new ScaledBitmap(2, 2, bitmap);
            }
        };
        MemoryAssetLoader memory = new MemoryAssetLoader();
        memory.clearCache();
        return new CachedAppAssetLoader(computer, 1, null, memory, disk, bitmap);
    }

    private void close(CachedAppAssetLoader loader) throws InterruptedException {
        for (String name : new String[] {"foregroundExecutor", "networkExecutor", "cacheExecutor"}) {
            ThreadPoolExecutor executor = ReflectionHelpers.getField(loader, name);
            executor.shutdownNow();
            assertTrue(executor.awaitTermination(5, TimeUnit.SECONDS));
        }
        loader.freeCacheMemory();
    }

    @Test public void renamedGameUpdatesItsLabelWhileKeepingTheInFlightArtworkRequest() throws Exception {
        CountDownLatch started = new CountDownLatch(1);
        CountDownLatch release = new CountDownLatch(1);
        CachedAppAssetLoader loader = loader(started, release);
        try {
            ImageView image = new ImageView(RuntimeEnvironment.getApplication());
            TextView name = new TextView(RuntimeEnvironment.getApplication());
            NvApp app = new NvApp("Old name", 10, false);
            loader.populateImageView(app, image, name);
            assertTrue(started.await(5, TimeUnit.SECONDS));
            Drawable pending = image.getDrawable();

            app.setAppName("New name");
            loader.populateImageView(app, image, name);

            assertSame("A title change does not need another artwork download", pending, image.getDrawable());
            assertEquals("New name", name.getText().toString());
        } finally {
            release.countDown();
            close(loader);
        }
    }

    @Test public void queuedArtworkRemovedOnPauseIsScheduledAgainOnRebind() throws Exception {
        CountDownLatch release = new CountDownLatch(1);
        CachedAppAssetLoader loader = loader(new CountDownLatch(0), new CountDownLatch(0));
        ThreadPoolExecutor executor = ReflectionHelpers.getField(loader, "foregroundExecutor");
        executor.setCorePoolSize(1);
        executor.setMaximumPoolSize(1);
        CountDownLatch busy = new CountDownLatch(1);
        executor.execute(() -> {
            busy.countDown();
            try { release.await(); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
        });
        try {
            assertTrue(busy.await(5, TimeUnit.SECONDS));
            ImageView image = new ImageView(RuntimeEnvironment.getApplication());
            TextView name = new TextView(RuntimeEnvironment.getApplication());
            NvApp app = new NvApp("Queued game", 20, false);
            loader.populateImageView(app, image, name);
            Drawable pending = image.getDrawable();
            assertEquals(1, executor.getQueue().size());

            loader.cancelForegroundLoads();
            assertTrue(executor.getQueue().isEmpty());
            loader.populateImageView(app, image, name);

            assertNotSame(pending, image.getDrawable());
            assertEquals("Rebinding after pause must queue a replacement load", 1, executor.getQueue().size());
        } finally {
            release.countDown();
            close(loader);
        }
    }

    @Test public void artworkTakesTheTitlesPlace() throws Exception {
        CachedAppAssetLoader loader = loader(new CountDownLatch(0), new CountDownLatch(0));
        try {
            ImageView image = new ImageView(RuntimeEnvironment.getApplication());
            TextView name = new TextView(RuntimeEnvironment.getApplication());
            loader.populateImageView(new NvApp("Cached game", 30, false), image, name);
            assertEquals("The title waits for the disk cache", android.view.View.INVISIBLE, name.getVisibility());
            assertFalse(CachedAppAssetLoader.hasArtwork(image));
            for (int i = 0; i < 250 && name.getVisibility() != android.view.View.GONE; i++) {
                org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
                Thread.sleep(20);
            }
            assertEquals(android.view.View.GONE, name.getVisibility());
            assertTrue(CachedAppAssetLoader.hasArtwork(image));
        } finally {
            close(loader);
        }
    }

    /** A host that is off: a cover saved on the disk still shows, and nothing asks the network for it. */
    // Robolectric cannot decode a file with ImageDecoder (Android 9+); the disk loader uses BitmapFactory below that.
    @Config(sdk = 25)
    @Test public void savedCoverShowsWithTheHostOffline() throws Exception {
        Application context = RuntimeEnvironment.getApplication();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "offline-host";
        NvApp app = new NvApp("Saved game", 40, false);
        DiskAssetLoader disk = new DiskAssetLoader(context);
        java.io.File file = disk.getFile(computer.uuid, app.getAssetCacheKey());
        assertTrue(file.getParentFile().isDirectory() || file.getParentFile().mkdirs());
        try (java.io.FileOutputStream out = new java.io.FileOutputStream(file)) {
            Bitmap.createBitmap(30, 40, Bitmap.Config.ARGB_8888).compress(Bitmap.CompressFormat.PNG, 100, out);
        }
        java.util.concurrent.atomic.AtomicInteger asked = new java.util.concurrent.atomic.AtomicInteger();
        NetworkAssetLoader offline = new NetworkAssetLoader(context, "unique") {
            @Override public java.io.InputStream getBitmapStream(CachedAppAssetLoader.LoaderTuple tuple) {
                asked.incrementAndGet();
                return null;
            }
        };
        MemoryAssetLoader memory = new MemoryAssetLoader();
        memory.clearCache();
        CachedAppAssetLoader loader = new CachedAppAssetLoader(computer, 1, offline, memory, disk,
                Bitmap.createBitmap(2, 2, Bitmap.Config.ARGB_8888));
        try {
            ImageView image = new ImageView(context);
            TextView name = new TextView(context);
            loader.populateImageView(app, image, name);
            // Drain the loader's threads instead of sleeping: when they have ended, the result is posted to the main thread.
            for (String executorName : new String[] {"foregroundExecutor", "networkExecutor"}) {
                ThreadPoolExecutor executor = ReflectionHelpers.getField(loader, executorName);
                executor.shutdown();
                assertTrue(executor.awaitTermination(5, TimeUnit.SECONDS));
            }
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
            assertEquals("The saved cover replaces the title", android.view.View.GONE, name.getVisibility());
            assertTrue(CachedAppAssetLoader.hasArtwork(image));
            assertEquals("A saved cover never goes to the network", 0, asked.get());
        } finally {
            close(loader);
        }
    }

    // The grey box.png a Rubylight or Sunshine-family host sends from /appasset for a game with no cover.
    private static byte[] hostPlaceholder() throws java.io.IOException {
        return java.nio.file.Files.readAllBytes(
                new java.io.File("../protocol/client/tests/fixtures/box-placeholder.png").toPath());
    }

    /** A game without a cover of its own: the host answers with its placeholder, and the tile shows the title. */
    @Config(sdk = 25)
    @Test public void theHostsPlaceholderShowsTheTitleNotAGreyTile() throws Exception {
        byte[] placeholder = hostPlaceholder();
        java.util.concurrent.atomic.AtomicInteger asked = new java.util.concurrent.atomic.AtomicInteger();
        CachedAppAssetLoader loader = placeholderLoader("placeholder-host", () -> {
            asked.incrementAndGet();
            return new java.io.ByteArrayInputStream(placeholder);
        });
        try {
            ImageView image = new ImageView(RuntimeEnvironment.getApplication());
            TextView name = new TextView(RuntimeEnvironment.getApplication());
            NvApp app = new NvApp("Game without art", 50, false);
            loader.populateImageView(app, image, name);
            drain(loader);
            assertEquals(android.view.View.VISIBLE, name.getVisibility());
            assertEquals("Game without art", name.getText().toString());
            assertFalse("The placeholder is not shown as the game's art", CachedAppAssetLoader.hasArtwork(image));
            assertTrue(loader.hasNoArt(app));
            assertEquals("The placeholder is not asked for again", 1, asked.get());

            // Next time it comes from the disk cache, and still shows the title without asking the host.
            loader.freeCacheMemory();
            ImageView again = new ImageView(RuntimeEnvironment.getApplication());
            TextView againName = new TextView(RuntimeEnvironment.getApplication());
            CachedAppAssetLoader next = placeholderLoader("placeholder-host", () -> {
                asked.incrementAndGet();
                return null;
            });
            try {
                next.populateImageView(app, again, againName);
                drain(next);
                assertEquals(android.view.View.VISIBLE, againName.getVisibility());
                assertFalse(CachedAppAssetLoader.hasArtwork(again));
                assertTrue(next.hasNoArt(app));
                assertEquals(1, asked.get());
            } finally {
                close(next);
            }
        } finally {
            close(loader);
        }
    }

    @Test public void onlyTheHostsOwnPlaceholderIsTakenForOne() throws Exception {
        byte[] placeholder = hostPlaceholder();
        assertEquals("d9164ebd069b5f735eb8efc557801778498da37f572ef70e3d35604739e6c613", PlaceholderArt.sha256(placeholder));
        java.io.File file = java.io.File.createTempFile("cover", ".png");
        try {
            java.nio.file.Files.write(file.toPath(), placeholder);
            assertTrue(PlaceholderArt.matches(file));
            // A cover of the same size with other bytes is a cover.
            byte[] other = placeholder.clone();
            other[other.length - 20] ^= 1;
            java.nio.file.Files.write(file.toPath(), other);
            assertFalse(PlaceholderArt.matches(file));
        } finally {
            file.delete();
        }
    }

    private static CachedAppAssetLoader placeholderLoader(String uuid, java.util.function.Supplier<java.io.InputStream> host) {
        Application context = RuntimeEnvironment.getApplication();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = uuid;
        NetworkAssetLoader network = new NetworkAssetLoader(context, "unique") {
            @Override public java.io.InputStream getBitmapStream(CachedAppAssetLoader.LoaderTuple tuple) {
                return host.get();
            }
        };
        MemoryAssetLoader memory = new MemoryAssetLoader();
        memory.clearCache();
        return new CachedAppAssetLoader(computer, 1, network, memory, new DiskAssetLoader(context),
                Bitmap.createBitmap(1, 1, Bitmap.Config.ARGB_8888));
    }

    // Runs the disk load, the network load it hands over to on the main thread, and their results.
    private static void drain(CachedAppAssetLoader loader) throws InterruptedException {
        for (String executorName : new String[] {"foregroundExecutor", "networkExecutor"}) {
            ThreadPoolExecutor executor = ReflectionHelpers.getField(loader, executorName);
            executor.shutdown();
            assertTrue(executor.awaitTermination(5, TimeUnit.SECONDS));
            org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle();
        }
    }
}
