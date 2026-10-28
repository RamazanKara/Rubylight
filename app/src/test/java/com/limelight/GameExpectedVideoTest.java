package com.limelight;

import static org.junit.Assert.*;

import android.app.Application;
import android.content.Intent;
import android.os.Handler;

import com.limelight.binding.video.HdrPolicy;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamMessages;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.List;

/**
 * The Game-level wiring of the refresh switch monitor and the HDR status line. Neither has run on a
 * phone with several refresh rates or an HDR panel; see docs/expected-refresh-switch.md and
 * docs/expected-hdr10.md.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class GameExpectedVideoTest {
    private static Game game() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "pc"));
        return game;
    }

    private static int count(List<String> notes, String text) {
        int n = 0;
        for (String note : notes) if (note.equals(text)) n++;
        return n;
    }

    @Test public void aRefusedHdrRequestShowsOnTheStatusLineExactlyOnce() {
        Game game = game();
        StreamMessages messages = ReflectionHelpers.getField(game, "messages");
        ReflectionHelpers.setField(game, "streamHdr", false);

        ReflectionHelpers.callInstanceMethod(game, "refuseHdr",
                ReflectionHelpers.ClassParameter.from(HdrPolicy.Refusal.class, HdrPolicy.Refusal.DISPLAY_WITHOUT_HDR10));
        // The poll runs every second for the rest of the stream; it must not add the refusal again.
        for (int i = 0; i < 3; i++) ReflectionHelpers.callInstanceMethod(game, "checkHdrStatus");

        String text = game.getString(R.string.stream_hdr_display_unsupported);
        assertEquals(1, count(messages.notes(), text));
        assertEquals(1, messages.notes().size());
    }

    @Test public void everyRefusalReasonNotesItsOwnTextOnce() {
        for (HdrPolicy.Refusal refusal : new HdrPolicy.Refusal[] {HdrPolicy.Refusal.ANDROID_TOO_OLD,
                HdrPolicy.Refusal.MODE_WITHOUT_HDR10, HdrPolicy.Refusal.DECODER_WITHOUT_HDR10,
                HdrPolicy.Refusal.DISPLAY_WITHOUT_HDR10, HdrPolicy.Refusal.HLG_NEEDS_DIRECT_OUTPUT}) {
            Game game = game();
            StreamMessages messages = ReflectionHelpers.getField(game, "messages");
            ReflectionHelpers.callInstanceMethod(game, "refuseHdr",
                    ReflectionHelpers.ClassParameter.from(HdrPolicy.Refusal.class, refusal));
            ReflectionHelpers.callInstanceMethod(game, "checkHdrStatus");
            assertEquals(refusal.toString(), 1, messages.notes().size());
        }
    }

    @Test public void anHlgOnlyScreenNamesItsOwnRefusalAndAPqStream() {
        Game game = game();
        StreamMessages messages = ReflectionHelpers.getField(game, "messages");
        ReflectionHelpers.callInstanceMethod(game, "refuseHdr", ReflectionHelpers.ClassParameter.from(
                HdrPolicy.Refusal.class, HdrPolicy.Refusal.HLG_NEEDS_DIRECT_OUTPUT));
        assertEquals(1, count(messages.notes(), game.getString(R.string.stream_hdr_hlg_needs_direct_output)));
        assertEquals(R.string.stream_hdr_pq_on_hlg_screen, Game.hdrStatusText(HdrPolicy.Status.PQ_ON_HLG_SCREEN));
    }

    @Test public void theVulkanRendererNamesHdrItShowsAsSdrAndWhatItLeavesOut() {
        Game game = game();
        assertEquals(R.string.stream_hdr_vulkan_shown_as_sdr, Game.hdrStatusText(HdrPolicy.Status.SHOWN_AS_SDR));
        assertEquals("HLG shown as SDR: the display refused HDR from “Render through Vulkan”.",
                game.getString(Game.hdrStatusText(HdrPolicy.Status.SHOWN_AS_SDR), "HLG"));
        assertEquals(R.string.stream_hdr_vulkan_no_dolby_vision,
                Game.vulkanHdrLimitText(com.limelight.binding.video.HdrFormatPolicy.DrawnLimit.DOLBY_VISION));
        // HDR10+ is left out only when the window refuses it (Game.checkHdrStatus).
        assertEquals("HDR10+ is not passed on with “Render through Vulkan” on this Android version, so the stream is "
                + "HDR10 without it.", game.getString(R.string.stream_hdr_vulkan_no_hdr10_plus));
        assertEquals(0, Game.vulkanHdrLimitText(com.limelight.binding.video.HdrFormatPolicy.DrawnLimit.NONE));
    }

    @Test public void theRefreshSwitchPollIsPostedOnConnectAndRemovedOnTeardown() {
        Game game = game();
        Handler handler = ReflectionHelpers.getField(game, "bitrateHandler");
        Runnable poll = ReflectionHelpers.getField(game, "pollRefreshSwitch");
        assertFalse(handler.hasCallbacks(poll));

        ReflectionHelpers.callInstanceMethod(game, "startRefreshSwitchPolling");
        assertTrue("Posted when the stream connects", handler.hasCallbacks(poll));
        // A second connect (recovery) must not leave two polls running.
        ReflectionHelpers.callInstanceMethod(game, "startRefreshSwitchPolling");
        assertTrue(handler.hasCallbacks(poll));

        ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());
        ReflectionHelpers.setField(game, "app", new NvApp("Game", 73, false));
        ReflectionHelpers.setField(game, "conn", new NvConnection(null,
                new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                new StreamConfiguration.Builder().build(), null, null) {
            @Override public void stop() {}
        });
        ReflectionHelpers.callInstanceMethod(game, "stopConnection");
        assertFalse("Removed when the stream stops", handler.hasCallbacks(poll));
    }

    @Test public void tearingDownClearsTheRefreshAndHdrStatusLines() {
        Game game = game();
        StreamMessages messages = ReflectionHelpers.getField(game, "messages");
        messages.status("refresh", "stays at 60");
        messages.status("hdr", "PC reports SDR");
        ReflectionHelpers.callInstanceMethod(game, "stopRefreshSwitchPolling");
        assertTrue(messages.notes().isEmpty());
    }
}
