package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.ServiceConnection;
import android.content.Intent;
import android.os.Build;
import android.os.Looper;

import com.limelight.PcViewLifecycleTest;
import com.limelight.computers.ComputerManagerListener;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostActions;
import com.limelight.console.HostWaker;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.Executor;
import java.util.function.LongSupplier;

/** A wake started on a PC's settings page before the computer manager is bound still has the PCs polled. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class HostSettingsWakeTest {
    private final Application app = RuntimeEnvironment.getApplication();
    private final List<String> polls = new ArrayList<>();

    @Before
    public void setUp() {
        GlPreferences gl = GlPreferences.readPreferences(app);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        app.getSharedPreferences("FirstRun", 0).edit().putBoolean("pairing_guide_shown", true).commit();
    }

    private static ComputerDetails asleep() {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "tower";
        computer.name = "Tower";
        computer.state = ComputerDetails.State.OFFLINE;
        computer.pairState = PairingManager.PairState.PAIRED;
        computer.macAddress = "02:7c:16:b4:13:4f";
        return computer;
    }

    private ComputerManagerService.ComputerManagerBinder manager() {
        return new ComputerManagerService().new ComputerManagerBinder() {
            @Override public boolean waitForReady() { return false; }
            @Override public void startPolling(ComputerManagerListener listener) { polls.add("start"); }
            @Override public void stopPolling() { polls.add("stop"); }
            @Override public ComputerDetails getComputer(String uuid) { return asleep(); }
            @Override public void invalidateStateForComputer(String uuid) { }
        };
    }

    @Test
    public void aWakeStartedBeforeTheManagerWasBoundPollsOnceItIs() {
        Intent intent = new Intent(app, HostStreamSettings.class).putExtra("host_uuid", "tower").putExtra("host_name", "Tower");
        ActivityController<HostStreamSettings> controller = Robolectric.buildActivity(HostStreamSettings.class, intent).setup();
        try {
            HostStreamSettings screen = controller.get();
            ReflectionHelpers.setField(screen, "manager", null);
            HostActions actions = ReflectionHelpers.getField(screen, "hostActions");
            ReflectionHelpers.callInstanceMethod(actions, "startWake",
                    ClassParameter.from(ComputerDetails.class, asleep()),
                    ClassParameter.from(HostWaker.Sender.class, (HostWaker.Sender) host -> { }),
                    ClassParameter.from(Executor.class, (Executor) Runnable::run),
                    ClassParameter.from(LongSupplier.class, (LongSupplier) () -> 0L));
            assertTrue(actions.isWaking());
            assertTrue("nothing to poll with yet", polls.isEmpty());

            ServiceConnection connection = ReflectionHelpers.getField(screen, "managerConnection");
            connection.onServiceConnected(null, manager());
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(List.of("start"), polls);
        } finally {
            controller.pause().stop().destroy();
        }
        assertEquals("leaving the page stops the poll", List.of("start", "stop"), polls);
    }
}
