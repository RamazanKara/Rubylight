package com.limelight.nvstream;

import android.app.AppOpsManager;
import android.app.Application;
import android.os.Build;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowAppOpsManager;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = {com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
                NvConnectionMicrophoneAppOpsTest.LollipopAppOps.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NvConnectionMicrophoneAppOpsTest {
    private final int realSdk = Build.VERSION.SDK_INT;

    /** Android 5.x has no op string for record audio: looking it up throws, as on those devices. */
    @Implements(AppOpsManager.class)
    public static class LollipopAppOps extends ShadowAppOpsManager {
        static boolean unknownRecordAudio;

        private static void lookup(String op) {
            if (unknownRecordAudio && AppOpsManager.OPSTR_RECORD_AUDIO.equals(op)) {
                throw new IllegalArgumentException("Unknown operation string: " + op);
            }
        }

        @Override @Implementation
        protected int checkOpNoThrow(String op, int uid, String packageName) {
            lookup(op);
            return super.checkOpNoThrow(op, uid, packageName);
        }

        @Implementation
        protected int startOpNoThrow(String op, int uid, String packageName) {
            lookup(op);
            return AppOpsManager.MODE_ALLOWED;
        }

        @Implementation
        protected void finishOp(String op, int uid, String packageName) {
            lookup(op);
        }
    }

    @After
    public void restorePlatform() {
        ReflectionHelpers.setStaticField(Build.VERSION.class, "SDK_INT", realSdk);
        LollipopAppOps.unknownRecordAudio = false;
    }

    @Test
    public void unmutingOnAndroid5DoesNotLookUpTheMarshmallowRecordAudioOp() {
        for (int sdk : new int[] {Build.VERSION_CODES.LOLLIPOP, Build.VERSION_CODES.LOLLIPOP_MR1}) {
            ReflectionHelpers.setStaticField(Build.VERSION.class, "SDK_INT", sdk);
            LollipopAppOps.unknownRecordAudio = true;
            NvConnection connection = connection();
            connection.setMicrophoneMuted(true);
            // Unmuting re-checks microphone access before the phone capture restarts.
            connection.setMicrophoneMuted(false);
            assertTrue("RECORD_AUDIO is an install-time grant before Android 6.0",
                    connection.controllerMicrophonePermitted());
            assertTrue(connection.beginControllerMicrophone());
            connection.endControllerMicrophone();
        }
    }

    @Test
    public void anUnknownRecordAudioOpDeniesTheMicrophoneInsteadOfCrashing() {
        LollipopAppOps.unknownRecordAudio = true;
        NvConnection connection = connection();
        connection.setMicrophoneMuted(false);
        assertFalse(connection.controllerMicrophonePermitted());
        assertFalse(connection.beginControllerMicrophone());
        connection.endControllerMicrophone();
    }

    private static NvConnection connection() {
        NvConnection connection = new NvConnection(RuntimeEnvironment.getApplication(),
                new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                new StreamConfiguration.Builder().setApp(new NvApp("Test", 73, false)).build(), null, null);
        // The stream started with the microphone setting on and RECORD_AUDIO granted.
        ReflectionHelpers.setField(connection, "micEnabled", true);
        ReflectionHelpers.setField(connection, "micPermitted", true);
        ReflectionHelpers.setField(connection, "nativeConnected", true);
        return connection;
    }
}
