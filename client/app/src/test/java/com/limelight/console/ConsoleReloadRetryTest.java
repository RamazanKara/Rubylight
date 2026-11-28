package com.limelight.console;

import static com.limelight.console.HomeLibraryRowsTest.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.os.Build;
import android.os.Looper;
import android.preference.PreferenceManager;

import com.limelight.PcViewLifecycleTest;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.GlPreferences;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.io.File;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.AbstractExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Regression tests for AND-07: GameDetailActivity and AllGamesActivity must clear their
 * loaded/refreshed signatures when a refresh fails or is interrupted so the next host poll with an
 * unchanged signature retries instead of stalling forever.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, qualifiers = "w960dp-h540dp-television-xhdpi",
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
public class ConsoleReloadRetryTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    static ConsoleRepository repository;

    public static class TestGrid extends AllGamesActivity {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) { return false; }
        @Override protected ConsoleRepository createRepository(String uniqueId) { return repository; }
    }

    public static class TestDetail extends GameDetailActivity {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) { return false; }
        @Override protected ConsoleRepository createRepository(String uniqueId) { return repository; }
    }

    private static final class Direct extends AbstractExecutorService {
        private final AtomicBoolean interruptFirst;
        final AtomicBoolean preservedInterrupt = new AtomicBoolean();
        private boolean shut;

        Direct(boolean interruptFirst) {
            this.interruptFirst = new AtomicBoolean(interruptFirst);
        }

        @Override public void shutdown() { shut = true; }
        @Override public List<Runnable> shutdownNow() { shut = true; return new ArrayList<>(); }
        @Override public boolean isShutdown() { return shut; }
        @Override public boolean isTerminated() { return shut; }
        @Override public boolean awaitTermination(long timeout, TimeUnit unit) { return true; }
        @Override public void execute(Runnable command) {
            boolean interrupt = interruptFirst.compareAndSet(true, false);
            if (!interrupt) {
                command.run();
                return;
            }
            // Run on a worker thread so show() posts to the main looper instead of running UI work
            // inline on the interrupted thread before repo.refresh() checks Thread.isInterrupted().
            Thread bg = new Thread(() -> {
                Thread.currentThread().interrupt();
                command.run();
                preservedInterrupt.set(Thread.interrupted());
            }, "console-interrupt-test");
            bg.start();
            try {
                bg.join(5000);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                throw new RuntimeException(e);
            }
        }
    }

    private Context context;
    private File repoDir;
    private NativeApiFixtures.FakeTransport transport;
    private final ComputerDetails tower = nativeHost("tower", "Tower", true);

    @Before
    public void setUp() throws Exception {
        context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
        GlPreferences gl = GlPreferences.readPreferences(context);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        repoDir = temporary.newFolder();
        transport = new NativeApiFixtures.FakeTransport();
        transport.reply("tower", "capabilities", ok("{\"api_version\":1,\"features\":[\"library\"],"
                + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true}}"));
        transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
        transport.reply("tower", "library", ok(library("t", game(1, "Portal", "steam", true, null))));
    }

    @After
    public void tearDown() {
        Thread.interrupted();
        ConsoleRepository.closeForTests(repoDir);
        repository = null;
        PreferenceManager.getDefaultSharedPreferences(context).edit().clear().commit();
    }

    private ComputerManagerService.ComputerManagerBinder binder() {
        return new ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean isPersistent() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) {}
            @Override public void stopPolling() {}
            @Override public void invalidateStateForComputer(String uuid) {}
            @Override public String getUniqueId() { return "unique"; }
            @Override public ComputerDetails getComputer(String uuid) { return "tower".equals(uuid) ? tower : null; }
            @Override public void waitForPollingStopped() {}
        };
    }

    private static void settle() {
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(300));
        shadowOf(Looper.getMainLooper()).idle();
    }

    private ConsoleRepository repositoryCountingRefresh(AtomicInteger endpointCalls, boolean throwFirst) {
        AtomicBoolean failFirst = new AtomicBoolean(throwFirst);
        return new ConsoleRepository(repoDir, transport, computer -> {
            endpointCalls.incrementAndGet();
            if (failFirst.compareAndSet(true, false)) {
                throw new RuntimeException("transient failure");
            }
            return HostEndpoint.of(computer, "unique", NativeApiFixtures.crypto());
        }, computer -> null, () -> 5000L, null);
    }

    @Test
    public void allGamesClearsRefreshedSignatureOnRuntimeFailureAndRetriesSameHostSignature() {
        AtomicInteger endpointCalls = new AtomicInteger();
        repository = repositoryCountingRefresh(endpointCalls, true);
        ActivityController<TestGrid> controller = Robolectric.buildActivity(TestGrid.class).create();
        try {
            TestGrid grid = controller.get();
            ReflectionHelpers.setField(grid, "worker", new Direct(false));
            HostFeed feed = ReflectionHelpers.getField(grid, "feed");
            ReflectionHelpers.setField(feed, "binder", binder());
            feed.update(tower);
            controller.start().resume().visible();
            settle();

            assertEquals(1, endpointCalls.get());
            assertNull("refreshedSignature must be cleared after load failure",
                    ReflectionHelpers.getField(grid, "refreshedSignature"));

            // Next poll reports the identical host signature: AllGames must retry refresh().
            feed.update(nativeHost("tower", "Tower", true));
            settle();
            assertTrue("unchanged host poll must retry repo.refresh()", endpointCalls.get() > 1);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void allGamesClearsRefreshedSignatureOnInterruptAndRetries() {
        AtomicInteger endpointCalls = new AtomicInteger();
        repository = repositoryCountingRefresh(endpointCalls, false);
        ActivityController<TestGrid> controller = Robolectric.buildActivity(TestGrid.class).create();
        try {
            TestGrid grid = controller.get();
            Direct direct = new Direct(true);
            ReflectionHelpers.setField(grid, "worker", direct);
            HostFeed feed = ReflectionHelpers.getField(grid, "feed");
            ReflectionHelpers.setField(feed, "binder", binder());
            feed.update(tower);
            controller.start().resume().visible();
            settle();
            assertTrue("InterruptedException handler must re-assert interrupt flag", direct.preservedInterrupt.get());

            assertEquals("first refresh threw InterruptedException before contacting endpoint", 0, endpointCalls.get());
            assertNull("refreshedSignature must be cleared after InterruptedException",
                    ReflectionHelpers.getField(grid, "refreshedSignature"));

            feed.update(nativeHost("tower", "Tower", true));
            settle();
            assertTrue("unchanged host poll must retry repo.refresh() after interrupt", endpointCalls.get() > 0);
        } finally {
            Thread.interrupted();
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void gameDetailClearsLoadedSignatureOnRuntimeFailureAndRetriesSameHostSignature() {
        AtomicInteger endpointCalls = new AtomicInteger();
        repository = repositoryCountingRefresh(endpointCalls, true);
        Intent intent = new Intent(context, TestDetail.class)
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, "tower")
                .putExtra(GameDetailActivity.EXTRA_APP_ID, 1)
                .putExtra(GameDetailActivity.EXTRA_TITLE, "Portal")
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, "Tower");
        ActivityController<TestDetail> controller = Robolectric.buildActivity(TestDetail.class, intent).create();
        try {
            TestDetail page = controller.get();
            ReflectionHelpers.setField(page, "worker", new Direct(false));
            HostFeed feed = ReflectionHelpers.getField(page, "feed");
            ReflectionHelpers.setField(feed, "binder", binder());
            feed.update(tower);
            controller.start().resume().visible();
            settle();

            assertEquals(1, endpointCalls.get());
            assertNull("loadedSignature must be cleared after load failure",
                    ReflectionHelpers.getField(page, "loadedSignature"));

            feed.update(nativeHost("tower", "Tower", true));
            settle();
            assertTrue("unchanged host poll must retry repo.refresh()", endpointCalls.get() > 1);
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void gameDetailClearsLoadedSignatureOnInterruptAndRetries() {
        AtomicInteger endpointCalls = new AtomicInteger();
        repository = repositoryCountingRefresh(endpointCalls, false);
        Intent intent = new Intent(context, TestDetail.class)
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, "tower")
                .putExtra(GameDetailActivity.EXTRA_APP_ID, 1)
                .putExtra(GameDetailActivity.EXTRA_TITLE, "Portal")
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, "Tower");
        ActivityController<TestDetail> controller = Robolectric.buildActivity(TestDetail.class, intent).create();
        try {
            TestDetail page = controller.get();
            Direct direct = new Direct(true);
            ReflectionHelpers.setField(page, "worker", direct);
            HostFeed feed = ReflectionHelpers.getField(page, "feed");
            ReflectionHelpers.setField(feed, "binder", binder());
            feed.update(tower);
            controller.start().resume().visible();
            settle();
            assertTrue("InterruptedException handler must re-assert interrupt flag", direct.preservedInterrupt.get());

            assertEquals("first refresh threw InterruptedException before contacting endpoint", 0, endpointCalls.get());
            assertNull("loadedSignature must be cleared after InterruptedException",
                    ReflectionHelpers.getField(page, "loadedSignature"));

            feed.update(nativeHost("tower", "Tower", true));
            settle();
            assertTrue("unchanged host poll must retry repo.refresh() after interrupt", endpointCalls.get() > 0);
        } finally {
            Thread.interrupted();
            controller.pause().stop().destroy();
        }
    }
}
