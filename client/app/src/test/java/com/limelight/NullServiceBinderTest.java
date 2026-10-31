package com.limelight;

import android.app.Activity;
import android.app.Application;
import android.content.ComponentName;
import android.content.ServiceConnection;
import com.limelight.computers.ComputerManagerService;
import com.limelight.files.FileTransferActivity;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import java.lang.reflect.Constructor;
import java.util.Arrays;
import static org.junit.Assert.*;

/**
 * Robolectric binds services with no binder. A screen that waited on it anyway threw on its waiter
 * thread after the test had ended, and the crash handler then ended the whole test JVM (exit 10).
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NullServiceBinderTest {
    @Test
    public void noBinderStartsNoWaiter() throws Exception {
        for (Class<? extends Activity> type : Arrays.asList(PcView.class, AppView.class, ShortcutTrampoline.class)) {
            Activity screen = Robolectric.buildActivity(type).get();
            connect(ReflectionHelpers.getField(screen, "serviceConnection"), screen);
            assertNull(type.getSimpleName(), ReflectionHelpers.getField(screen, "serviceWaitThread"));
        }

        // Before onCreate it has no worker pool: a wait would throw right here.
        FileTransferActivity files = Robolectric.buildActivity(FileTransferActivity.class).get();
        connect(ReflectionHelpers.getField(files, "service"), files);

        Class<?> feedType = Class.forName("com.limelight.console.HostFeed");
        Constructor<?> constructor = feedType.getDeclaredConstructor(Activity.class,
                Class.forName("com.limelight.console.HostFeed$Listener"));
        constructor.setAccessible(true);
        Object feed = constructor.newInstance(files, null);
        connect(ReflectionHelpers.getField(feed, "connection"), files);
        assertNull("HostFeed", ReflectionHelpers.getField(feed, "waiter"));
    }

    private static void connect(ServiceConnection connection, Activity screen) {
        connection.onServiceConnected(new ComponentName(screen, ComputerManagerService.class), null);
    }
}
