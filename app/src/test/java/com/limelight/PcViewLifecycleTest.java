package com.limelight;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.GlPreferences;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class PcViewLifecycleTest {
    @Implements(value = MoonBridge.class, isInAndroidSdk = false)
    public static class NoNativeMoonBridge {
        @Implementation protected static void __staticInitializer__() { }
    }

    public static class Library extends PcView {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) {
            return false;
        }
    }

    @Test
    public void frontendExportRetainsItsHostAndFirstFolderAcrossRecreation() {
        Application application = RuntimeEnvironment.getApplication();
        GlPreferences gl = GlPreferences.readPreferences(application);
        gl.savedFingerprint = Build.FINGERPRINT;
        gl.glRenderer = "Test renderer";
        gl.writePreferences();
        application.getSharedPreferences("FirstRun", 0).edit().putBoolean("pairing_guide_shown", true).commit();
        ActivityController<Library> first = Robolectric.buildActivity(Library.class).create().start().resume();
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "host-uuid";
        computer.name = "PC";
        Uri folder = Uri.parse("content://documents/tree/roms");
        ReflectionHelpers.setField(first.get(), "exportComputer", computer);
        ReflectionHelpers.setField(first.get(), "exportRomsTree", folder);
        Bundle saved = new Bundle();
        first.pause().saveInstanceState(saved).stop().destroy();

        ActivityController<Library> restored = Robolectric.buildActivity(Library.class).create(saved).start().resume();
        try {
            ComputerDetails restoredComputer = ReflectionHelpers.getField(restored.get(), "exportComputer");
            assertNotNull(restoredComputer);
            assertEquals(computer.uuid, restoredComputer.uuid);
            assertEquals(computer.name, restoredComputer.name);
            assertEquals(folder, ReflectionHelpers.getField(restored.get(), "exportRomsTree"));
        } finally {
            restored.pause().stop().destroy();
        }
    }
}
