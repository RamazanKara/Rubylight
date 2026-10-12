package com.limelight.computers;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;

import com.limelight.AppView;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.utils.CacheHelper;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ServiceController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.nio.file.Files;
import java.util.Collections;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class ComputerManagerLifecycleTest {
    public static class Manager extends ComputerManagerService {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }
    }

    @Test
    public void deletingComputerRemovesItsCachedLibraryArtworkAndHiddenAppsOnly() throws Exception {
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "removed-host";
        try {
            for (String host : new String[] {computer.uuid, "another-host"}) {
                Files.writeString(CacheHelper.openPath(true, service.getCacheDir(), "applist", host).toPath(), "library");
                Files.writeString(CacheHelper.openPath(true, service.getCacheDir(), "boxart", host, "42.png").toPath(), "cover");
                service.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, 0).edit()
                        .putStringSet(host, Collections.singleton("42")).commit();
            }

            service.removeComputer(computer);

            assertFalse(CacheHelper.openPath(false, service.getCacheDir(), "applist", computer.uuid).exists());
            assertFalse(CacheHelper.openPath(false, service.getCacheDir(), "boxart", computer.uuid).exists());
            assertFalse(service.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, 0).contains(computer.uuid));
            assertTrue(CacheHelper.openPath(false, service.getCacheDir(), "applist", "another-host").isFile());
            assertTrue(CacheHelper.openPath(false, service.getCacheDir(), "boxart", "another-host", "42.png").isFile());
            assertTrue(service.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, 0).contains("another-host"));
        } finally {
            controller.destroy();
        }
    }

    @Test
    public void deletingComputerRemovesOnlyItsSavedStreamSettings() {
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "removed-host";
        String[] files = {"HostStreamProfiles", "HostDisplayChoices", "AdaptiveBitrate", "PyroWaveBitrate"};
        try {
            for (String file : files) {
                service.getSharedPreferences(file, 0).edit()
                        .putString(computer.uuid, "saved").putString("another-host", "retained").commit();
            }
            service.removeComputer(computer);
            for (String file : files) {
                assertFalse(service.getSharedPreferences(file, 0).contains(computer.uuid));
                assertEquals("retained", service.getSharedPreferences(file, 0).getString("another-host", null));
            }
        } finally {
            controller.destroy();
        }
    }

    @Test
    public void failingPollListenerStillReleasesTheDatabaseReference() throws Exception {
        ServiceController<Manager> controller = Robolectric.buildService(Manager.class).create();
        ComputerManagerService service = controller.get();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "pc";
        computer.state = ComputerDetails.State.OFFLINE;
        ReflectionHelpers.setField(service, "listener", (ComputerManagerListener) details -> {
            throw new IllegalStateException("Listener failed");
        });
        Method poll = ComputerManagerService.class.getDeclaredMethod("runPoll", ComputerDetails.class, boolean.class, int.class);
        poll.setAccessible(true);
        try {
            InvocationTargetException failure = assertThrows(InvocationTargetException.class,
                    () -> poll.invoke(service, computer, false, 3));
            assertEquals("Listener failed", failure.getCause().getMessage());
            AtomicInteger references = ReflectionHelpers.getField(service, "dbRefCount");
            assertEquals(1, references.get());
        } finally {
            controller.destroy();
        }
    }
}
