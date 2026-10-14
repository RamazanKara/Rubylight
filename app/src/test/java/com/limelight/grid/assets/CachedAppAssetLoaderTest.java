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
}
