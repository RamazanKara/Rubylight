package com.limelight.demo;

import android.content.Context;
import android.preference.PreferenceManager;
import android.view.WindowManager;

import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.utils.CacheHelper;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

public final class DemoFixtures {
    public static final String PC_UUID = "d3e00000-0000-4000-8000-000000000001";
    public static final String[] TITLES = {"Desktop", "Racing Game", "Open World", "Space Sim",
            "Puzzle Night", "Retro Arcade", "Strategy", "Music Studio", "Ocean Explorer", "Skybound"};
    // Living-room PC's host: a Rubylight with every permission, so the stream menu and the PC settings
    // show their full layouts. It has no PyroWave, and nothing here is ever sent to or read from a host.
    private static final String HOST = "{\"v\":1,\"gameStream\":false,\"sunshineVersion\":true,\"paired\":true," +
            "\"rustHostVersion\":\"2.2.21\",\"permission\":4294967295,\"serverCommands\":1,\"frameLimiter\":true," +
            "\"virtualDisplayCapable\":true,\"pyroWaveFields\":false,\"pyroWaveProbeBytes\":0,\"pyroWaveCodecs\":0," +
            "\"encoderBitrate\":true,\"hardwareMetricsVersion\":1,\"networkPathVersion\":0," +
            "\"controllerPreparation\":true,\"currentGameUuid\":true}";
    // "--es host sunshine": a plain Sunshine without device permissions or Vibepollo's extensions, so it
    // lists no Input-only tile and a game's menu shows Input-only on any PC, which this turns on.
    private static final String SUNSHINE_HOST = "{\"v\":1,\"gameStream\":false,\"sunshineVersion\":true,\"paired\":true," +
            "\"virtualDisplayCapable\":false,\"pyroWaveFields\":false,\"pyroWaveProbeBytes\":0,\"pyroWaveCodecs\":0}";
    static boolean sunshineHost;
    // "--es host native": Living-room PC also has the native API, which Home and the game detail need.
    static boolean nativeApiHost;

    public static HostCapabilities hostCapabilities() {
        String json = sunshineHost ? SUNSHINE_HOST : nativeApiHost ? HOST.replace("{\"v\":1,", "{\"v\":1,\"nativeApiVersion\":1,") : HOST;
        return HostCapabilities.fromJson(json);
    }

    /** What Living-room PC's host reports, on any ComputerDetails. */
    public static void describeHost(ComputerDetails host) {
        host.caps = hostCapabilities();
        if (sunshineHost) {
            host.permission = -1;
            host.serverCommands.clear();
            host.encoderBitrateSupported = false;
            return;
        }
        host.permission = 0xFFFFFFFFL;
        if (!host.serverCommands.contains("Sleep")) host.serverCommands.add("Sleep");
        host.encoderBitrateSupported = true;
    }

    public static List<ComputerDetails> computers() {
        List<ComputerDetails> computers = new ArrayList<>();
        String[] names = {"Living-room PC", "Studio PC", "Laptop"};
        for (int i = 0; i < names.length; i++) {
            ComputerDetails pc = new ComputerDetails();
            pc.uuid = "d3e00000-0000-4000-8000-00000000000" + (i + 1);
            pc.name = names[i];
            pc.state = i == 2 ? ComputerDetails.State.OFFLINE : ComputerDetails.State.ONLINE;
            pc.pairState = i == 1 ? PairState.NOT_PAIRED : PairState.PAIRED;
            pc.macAddress = "02:00:00:00:00:0" + (i + 1);
            pc.runningGameId = i == 0 ? 2 : 0;
            pc.rawAppList = appList();
            if (i == 0) describeHost(pc);
            computers.add(pc);
        }
        return computers;
    }

    public static String appList() {
        StringBuilder xml = new StringBuilder("<root status_code=\"200\">");
        for (int i = 0; i < TITLES.length; i++) {
            xml.append("<App><AppTitle>").append(TITLES[i]).append("</AppTitle><ID>")
                    .append(i + 1).append("</ID><IsHdrSupported>1</IsHdrSupported></App>");
        }
        return xml.append("</root>").toString();
    }

    public static void prepare(Context context) throws IOException {
        context.getSharedPreferences("FirstRun", 0).edit().putBoolean("pairing_guide_shown", true).apply();
        WindowManager windows = (WindowManager) context.getSystemService(Context.WINDOW_SERVICE);
        int fps = DisplayFrameRatePolicy.streamFrameRate(DisplayFrameRatePolicy.maxRefreshRate(windows.getDefaultDisplay()));
        PreferenceManager.getDefaultSharedPreferences(context).edit()
                .putString("list_resolution", "1920x1080").putString("list_fps", Integer.toString(fps))
                .putInt("seekbar_bitrate_kbps", 40000).putString("video_format", "auto")
                .putString("frame_pacing", "balanced").putString("upscaling_mode", "fsr1")
                .putBoolean("checkbox_vrr", true).putBoolean("checkbox_enable_hdr", false)
                .putBoolean("checkbox_drop_late_frames", false).putBoolean("checkbox_reduce_refresh_rate", false)
                .putBoolean("checkbox_codec_performance", true).putBoolean("checkbox_phone_performance_hints", true)
                .putBoolean("checkbox_small_icon_mode", true)
                .putBoolean("checkbox_enable_pip", true).putBoolean("checkbox_show_onscreen_controls", true)
                .putBoolean("checkbox_enable_perf_overlay", true)
                .putBoolean("checkbox_usb_dualsense", true).putBoolean("checkbox_usb_dualsense_haptics", true)
                .putBoolean("checkbox_input_only_any_host", sunshineHost).apply();
        // As if Living-room PC had answered before: its PC settings know the host from the first frame.
        HostCapabilityCache.save(context, PC_UUID, hostCapabilities());
        try (OutputStream output = CacheHelper.openCacheFileForOutput(context.getCacheDir(), "applist", PC_UUID)) {
            output.write(appList().getBytes(StandardCharsets.UTF_8));
        }
        for (int i = 0; i < TITLES.length; i++) {
            NvApp app = new NvApp(TITLES[i], i + 1, true);
            try (InputStream input = context.getAssets().open("demo/art/" + (i + 1) + ".png");
                 OutputStream output = CacheHelper.openCacheFileForOutput(context.getCacheDir(),
                         "boxart", PC_UUID, app.getAssetCacheKey() + ".png")) {
                byte[] buffer = new byte[8192];
                for (int count; (count = input.read(buffer)) != -1;) output.write(buffer, 0, count);
            }
        }
    }
}
