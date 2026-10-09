package com.limelight.computers;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;

import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ServiceController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
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
