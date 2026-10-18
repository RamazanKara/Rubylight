package com.limelight.utils;

import android.app.Application;
import android.net.Uri;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class FrontendLauncherTest {
    @Test public void commandsUseEachLaunchersPlaceholderAndCurrentPackage() {
        for (String packageName : new String[] {"com.butterpollo.client", "com.butterpollo.client.root",
                "com.butterpollo.client.debug"}) {
            String beacon = FrontendLauncher.BEACON.command(packageName);
            assertTrue(beacon.startsWith("am start -n " + packageName + "/com.limelight.ShortcutTrampoline "));
            assertTrue(beacon.contains("--grant-read-uri-permission"));
            assertTrue(beacon.endsWith("-d {file_uri}"));
            assertTrue(FrontendLauncher.DAIJISHO.command(packageName).startsWith("-n " + packageName));
            assertTrue(FrontendLauncher.DAIJISHO.command(packageName).endsWith("{file.uri}"));
            assertTrue(FrontendLauncher.PEGASUS.command(packageName).startsWith("am start -n " + packageName));
            assertNull(FrontendLauncher.ES_DE.command(packageName));
        }
    }

    @Test public void rememberedDestinationsSurviveReopeningAndAreScopedToPcAndLauncher() {
        Application context = RuntimeEnvironment.getApplication();
        Uri roms = Uri.parse("content://folders/tree/roms");
        Uri esde = Uri.parse("content://folders/tree/esde");
        new FrontendExportStore(context, "one", FrontendLauncher.ES_DE).save(roms, esde);
        FrontendExportStore reopened = new FrontendExportStore(context, "one", FrontendLauncher.ES_DE);
        assertEquals(roms, reopened.romsTree());
        assertEquals(esde, reopened.esdeTree());
        assertNull(new FrontendExportStore(context, "one", FrontendLauncher.BEACON).romsTree());
        assertNull(new FrontendExportStore(context, "two", FrontendLauncher.ES_DE).romsTree());
        reopened.save(roms, null);
        assertNull(new FrontendExportStore(context, "one", FrontendLauncher.ES_DE).esdeTree());
    }
}
