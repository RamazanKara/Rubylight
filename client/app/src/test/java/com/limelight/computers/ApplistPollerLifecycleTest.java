package com.limelight.computers;

import android.app.Application;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.utils.CacheHelper;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ServiceController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;

import java.nio.file.Files;
import java.security.cert.X509Certificate;
import java.util.LinkedList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = ApplistPollerLifecycleTest.DelayedHttp.class)
public class ApplistPollerLifecycleTest {
    @Before public void resetRequests() { DelayedHttp.requests.clear(); }

    @Test public void stoppedRequestCannotReplaceTheSavedLibraryOrNotifyTheScreen() throws Exception {
        assertLateResponseDiscarded(false);
    }

    @Test public void previousRequestCannotPublishToTheResumedLibraryScreen() throws Exception {
        assertLateResponseDiscarded(true);
    }

    private void assertLateResponseDiscarded(boolean restart) throws Exception {
        ServiceController<ComputerManagerLifecycleTest.Manager> controller =
                Robolectric.buildService(ComputerManagerLifecycleTest.Manager.class).create();
        ComputerManagerService service = controller.get();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "delayed-library";
        computer.name = "PC";
        computer.state = ComputerDetails.State.ONLINE;
        computer.pairState = PairingManager.PairState.PAIRED;
        computer.activeAddress = new ComputerDetails.AddressTuple("192.0.2.1", 47989);
        computer.rawAppList = appList("Saved app");
        LinkedList<PollingTuple> tuples = ReflectionHelpers.getField(service, "pollingTuples");
        tuples.add(new PollingTuple(computer, null));
        java.nio.file.Path cache = CacheHelper.openPath(true, service.getCacheDir(), "applist", computer.uuid).toPath();
        Files.writeString(cache, computer.rawAppList);
        AtomicInteger updates = new AtomicInteger();
        CountDownLatch currentUpdated = new CountDownLatch(1);
        ReflectionHelpers.setField(service, "listener", new ComputerManagerListener() {
            @Override public void notifyComputerUpdated(ComputerDetails details) {}
            @Override public void notifyAppListUpdated(ComputerDetails details, boolean success) {
                updates.incrementAndGet();
                if (appList("Current app").equals(details.rawAppList)) currentUpdated.countDown();
            }
        });
        ComputerManagerService.ApplistPoller poller = service.new ApplistPoller(computer);
        Request previous = new Request(appList("Stale app"));
        Request current = new Request(appList("Current app"));
        DelayedHttp.requests.add(previous);
        DelayedHttp.requests.add(current);
        Thread oldWorker = null, newWorker = null;
        try {
            poller.start();
            oldWorker = ReflectionHelpers.getField(poller, "thread");
            assertTrue(previous.entered.await(2, TimeUnit.SECONDS));
            poller.stop();
            if (restart) {
                poller.start();
                newWorker = ReflectionHelpers.getField(poller, "thread");
            }
            // Even cancellation can race a response that has already been buffered.
            previous.release.countDown();
            oldWorker.join(2000);
            assertFalse("Stopped worker did not exit", oldWorker.isAlive());
            if (restart) assertTrue(current.entered.await(2, TimeUnit.SECONDS));
            assertEquals("A stopped response replaced the current library", appList("Saved app"), computer.rawAppList);
            assertEquals(appList("Saved app"), Files.readString(cache));
            assertEquals("A stopped response notified the resumed screen", 0, updates.get());
            assertEquals("Stopping the poller did not cancel its host request", 1, previous.cancellations.get());
            if (restart) {
                assertEquals(0, current.cancellations.get());
                current.release.countDown();
                assertTrue(currentUpdated.await(2, TimeUnit.SECONDS));
                assertEquals(1, updates.get());
                assertEquals(appList("Current app"), computer.rawAppList);
                assertEquals(appList("Current app"), Files.readString(cache));
            }
        } finally {
            previous.release.countDown();
            current.release.countDown();
            poller.stop();
            if (oldWorker != null) oldWorker.join(2000);
            if (newWorker != null) newWorker.join(2000);
            controller.destroy();
        }
    }

    private static String appList(String title) {
        return "<root status_code=\"200\"><App><ID>73</ID><AppTitle>" + title + "</AppTitle></App></root>";
    }

    private static class Request {
        final String response;
        final CountDownLatch entered = new CountDownLatch(1);
        final CountDownLatch release = new CountDownLatch(1);
        final AtomicInteger cancellations = new AtomicInteger();
        Request(String response) { this.response = response; }
    }

    @Implements(value = NvHTTP.class, isInAndroidSdk = false)
    public static class DelayedHttp {
        static final LinkedBlockingQueue<Request> requests = new LinkedBlockingQueue<>();
        private volatile Request request;

        @Implementation public void __constructor__(ComputerDetails.AddressTuple address, int httpsPort,
                String uniqueId, X509Certificate cert, LimelightCryptoProvider crypto) {}

        @Implementation public String getAppListRaw() {
            request = requests.poll();
            if (request == null) throw new AssertionError("Unexpected library request");
            request.entered.countDown();
            boolean interrupted = false;
            try {
                while (true) {
                    try {
                        if (!request.release.await(5, TimeUnit.SECONDS)) throw new AssertionError("Host fixture timed out");
                        return request.response;
                    } catch (InterruptedException e) {
                        interrupted = true;
                    }
                }
            } finally {
                if (interrupted) Thread.currentThread().interrupt();
            }
        }

        @Implementation public void cancelPendingRequests() {
            if (request != null) request.cancellations.incrementAndGet();
        }
    }
}
