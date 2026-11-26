package com.limelight;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.preference.PreferenceManager;
import android.view.Display;
import android.view.WindowManager;

import com.limelight.computers.HostCapabilityCache;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

/** "HDR format" as the real stream screen applies it. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameHdrFormatTest {
    private static final String REFUSED = "HDR requested but not used: DISPLAY_WITHOUT_HDR10";

    /** The stream screen's onCreate on an HLG-only screen, with a PC that offers HLG. */
    @SuppressWarnings("deprecation")
    private static Game created(String hdrFormat) {
        Context context = RuntimeEnvironment.getApplication();
        Display display = context.getSystemService(WindowManager.class).getDefaultDisplay();
        Shadows.shadowOf(display).setDisplayHdrCapabilities(display.getDisplayId(), 750, 750, 0.5f,
                Display.HdrCapabilities.HDR_TYPE_HLG);
        PreferenceManager.getDefaultSharedPreferences(context).edit()
                .putBoolean("checkbox_enable_hdr", true).putString("list_hdr_format", hdrFormat).commit();
        HostCapabilityCache.save(context, "pc", TestHosts.caps(TestHosts.serverInfo("rubylight-30-serverinfo.xml")));
        Intent intent = new Intent(context, Game.class)
                .putExtra(Game.EXTRA_HOST, "192.0.2.1")
                .putExtra(Game.EXTRA_UNIQUEID, "device")
                .putExtra(Game.EXTRA_PC_UUID, "pc")
                .putExtra(Game.EXTRA_APP_ID, 11);
        Game game = Robolectric.buildActivity(Game.class, intent).create().get();
        assertNotNull("the screen got as far as its connection", game.conn);
        return game;
    }

    private static int refusals() {
        return LimeLog.getRedactedLog().split(REFUSED, -1).length - 1;
    }

    private static boolean staticOnly(Game game) {
        StreamConfiguration config =
                ReflectionHelpers.getField(ReflectionHelpers.<Object>getField(game.conn, "context"), "streamConfig");
        return config.getHdrStaticOnly();
    }

    @Test
    public void hdr10OnlyRefusesHdrOnAnHlgOnlyScreen() {
        int before = refusals();
        Game game = created("hdr10");
        assertEquals(before + 1, refusals());
        assertFalse(game.streamHdr);
        assertTrue(staticOnly(game));
    }

    @Test
    public void automaticLetsHlgStandInAndAsksForNothingLess() {
        int before = refusals();
        Game game = created("auto");
        assertEquals(before, refusals());
        assertFalse(staticOnly(game));
    }
}
