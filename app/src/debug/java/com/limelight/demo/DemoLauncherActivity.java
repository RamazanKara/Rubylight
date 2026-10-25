package com.limelight.demo;

import android.app.Activity;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Bundle;

import com.limelight.AppView;
import com.limelight.console.GameDetailActivity;
import com.limelight.ui.AppTheme;

import java.io.IOException;

public final class DemoLauncherActivity extends Activity {
    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        // "--es ui tv" or "--es ui handheld" sets the Interface layout setting (in the demo's own preferences) for this
        // run; without it the demo uses the automatic layout. "--ez tv true" is the older, TV-only form.
        String ui = getIntent().getStringExtra("ui");
        android.preference.PreferenceManager.getDefaultSharedPreferences(this).edit()
                .putString(AppTheme.UI_MODE, AppTheme.UI_MODE_TV.equals(ui) ? AppTheme.UI_MODE_TV
                        : AppTheme.UI_MODE_HANDHELD.equals(ui) ? AppTheme.UI_MODE_HANDHELD : AppTheme.UI_MODE_AUTO).commit();
        if ("input-workspace".equals(getIntent().getStringExtra("state"))) {
            Intent preview = new Intent(this, DemoInputWorkspaceActivity.class);
            if (getIntent().getExtras() != null) preview.putExtras(getIntent().getExtras());
            startActivity(preview.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK));
            finish();
            return;
        }
        String home = getIntent().getStringExtra("home");
        if (home != null) {
            // adb's shell may not change component state on current Android, but the app can change its own:
            // "--es home enable" / "--es home disable" toggles the stand-in home screen used for the PiP capture.
            getPackageManager().setComponentEnabledSetting(new ComponentName(this, DemoHomeActivity.class),
                    "enable".equals(home) ? PackageManager.COMPONENT_ENABLED_STATE_ENABLED
                            : PackageManager.COMPONENT_ENABLED_STATE_DEFAULT, PackageManager.DONT_KILL_APP);
            finish();
            return;
        }
        // The demo screens that are not NightjarActivities have no setting to read; they follow the wrapper.
        DemoContext.forceTelevision = getIntent().getBooleanExtra("tv", false) || AppTheme.UI_MODE_TV.equals(ui);
        DemoFixtures.smallTiles = getIntent().getBooleanExtra("small", true);
        DemoFixtures.roleTiles = getIntent().getBooleanExtra("roles", false);
        DemoFixtures.sunshineHost = "sunshine".equals(getIntent().getStringExtra("host"));
        DemoFixtures.nativeApiHost = "native".equals(getIntent().getStringExtra("host"));
        DemoConsole.hostControl = getIntent().getBooleanExtra("control", true);
        String stars = getIntent().getStringExtra("stars");
        DemoConsole.starred = stars == null || stars.isEmpty() ? new int[0]
                : java.util.Arrays.stream(stars.split(",")).mapToInt(Integer::parseInt).toArray();
        DemoRadeon.reset(getIntent().getStringExtra("radeon"));
        DemoConsole.racingRuns = !"idle".equals(getIntent().getStringExtra("session"));
        try {
            DemoFixtures.prepare(this);
        } catch (IOException e) {
            throw new IllegalStateException("Demo assets missing; run scripts/demo/generate-assets.ps1", e);
        }
        String screen = getIntent().getStringExtra("state");
        // "--es state pair" taps the Studio PC (not paired) to show the PIN; "--es state options" opens a PC card's
        // options on Home (use --es host native for both Home states).
        DemoFixtures.studioAddress = "pair".equals(screen);
        Class<?> target;
        DemoConsole.hostAdds = getIntent().getBooleanExtra("add", true);
        if ("home".equals(screen) || "options".equals(screen)) target = DemoConsoleHome.class;
        // "--es state allgames --es filter store:steam": All games under a filter (with --es host native).
        else if ("allgames".equals(screen)) target = DemoAllGames.class;
        else if ("detail".equals(screen)) target = DemoGameDetail.class;
        else if ("library".equals(screen)) target = DemoAppView.class;
        else if ("settings".equals(screen)) target = DemoStreamSettings.class;
        else if ("pc-settings".equals(screen)) target = DemoHostStreamSettings.class;
        else if ("controller".equals(screen)) target = DemoControllerMappingActivity.class;
        else if ("gpu-driver".equals(screen)) target = DemoGpuDriverSettings.class;
        else if ("stream".equals(screen) || "compact".equals(screen) || "statistics".equals(screen) ||
                "advanced".equals(screen) || "debug".equals(screen) || "touch".equals(screen) ||
                "pip".equals(screen)) target = DemoGameActivity.class;
        else target = DemoPcView.class;
        DemoGameActivity.forceH264 = "h264".equals(getIntent().getStringExtra("codec"));
        Intent intent = new Intent(this, target).putExtra("state", screen).putExtra("fit", getIntent().getStringExtra("fit"))
                .putExtra("align", getIntent().getStringExtra("align"))
                .putExtra("orientation", getIntent().getStringExtra("orientation"))
                .putExtra("hint", getIntent().getBooleanExtra("hint", false))
                .putExtra("touch", getIntent().getStringExtra("touch"))
                .putExtra("pckeyboard", getIntent().getBooleanExtra("pckeyboard", false))
                .putExtra("align", getIntent().getStringExtra("align"))
                .putExtra("orientation", getIntent().getStringExtra("orientation"))
                .putExtra("size", getIntent().getStringExtra("size"))
                .putExtra("page", getIntent().getStringExtra("page"))
                .putExtra(com.limelight.console.AllGamesActivity.EXTRA_FILTER, getIntent().getStringExtra("filter"))
                .putExtra("position", getIntent().getStringExtra("position"))
                .putExtra(AppView.UUID_EXTRA, DemoFixtures.PC_UUID).putExtra(AppView.NAME_EXTRA, "Living-room PC")
                .putExtra("host_uuid", DemoFixtures.PC_UUID).putExtra("host_name", "Living-room PC")
                // The game detail: "--ei app 2" opens Racing Game (the running one), any other id its own card.
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, DemoFixtures.PC_UUID)
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, "Living-room PC")
                .putExtra(GameDetailActivity.EXTRA_APP_ID, getIntent().getIntExtra("app", 2))
                .putExtra(GameDetailActivity.EXTRA_TITLE, DemoConsole.titleOf(getIntent().getIntExtra("app", 2)));
        startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK));
        finish();
    }
}
