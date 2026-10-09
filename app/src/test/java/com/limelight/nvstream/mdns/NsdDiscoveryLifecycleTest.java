package com.limelight.nvstream.mdns;

import android.app.Application;
import android.net.nsd.NsdManager;
import android.net.nsd.NsdServiceInfo;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadow.api.Shadow;

import java.net.InetAddress;
import java.util.Collections;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.Executor;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, shadows = NsdDiscoveryLifecycleTest.RecordingNsdManager.class)
public class NsdDiscoveryLifecycleTest {
    @Implements(NsdManager.class)
    public static class RecordingNsdManager {
        NsdManager.DiscoveryListener discovery;
        NsdManager.ServiceInfoCallback callback;

        @Implementation protected void discoverServices(String type, int protocol, NsdManager.DiscoveryListener listener) {
            discovery = listener;
        }

        @Implementation protected void stopServiceDiscovery(NsdManager.DiscoveryListener listener) { }

        @Implementation protected void registerServiceInfoCallback(NsdServiceInfo info, Executor executor,
                                                                    NsdManager.ServiceInfoCallback callback) {
            this.callback = callback;
        }

        @Implementation protected void unregisterServiceInfoCallback(NsdManager.ServiceInfoCallback callback) { }
    }

    @Test
    public void stoppingDiscoveryDoesNotWaitForTheHostNetworkRequest() throws Exception {
        Application application = RuntimeEnvironment.getApplication();
        RecordingNsdManager manager = Shadow.extract(application.getSystemService(NsdManager.class));
        CountDownLatch entered = new CountDownLatch(1);
        CountDownLatch release = new CountDownLatch(1);
        NsdManagerDiscoveryAgent agent = new NsdManagerDiscoveryAgent(application, new MdnsDiscoveryListener() {
            @Override public void notifyComputerAdded(MdnsComputer computer) {
                entered.countDown();
                try {
                    release.await();
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
            }
            @Override public void notifyDiscoveryFailure(Exception e) { throw new AssertionError(e); }
        });
        agent.startDiscovery(1000);
        manager.discovery.onDiscoveryStarted("_nvstream._tcp");
        NsdServiceInfo info = new NsdServiceInfo();
        info.setServiceName("PC");
        info.setPort(47989);
        info.setHostAddresses(Collections.singletonList(InetAddress.getByName("192.0.2.1")));
        manager.discovery.onServiceFound(info);
        ExecutorService threads = Executors.newFixedThreadPool(2);
        try {
            Future<?> update = threads.submit(() -> manager.callback.onServiceUpdated(info));
            assertTrue(entered.await(2, TimeUnit.SECONDS));
            threads.submit(agent::stopDiscovery).get(1, TimeUnit.SECONDS);
            release.countDown();
            update.get(2, TimeUnit.SECONDS);
        } finally {
            release.countDown();
            threads.shutdownNow();
            assertTrue(threads.awaitTermination(2, TimeUnit.SECONDS));
            agent.stopDiscovery();
        }
    }
}
