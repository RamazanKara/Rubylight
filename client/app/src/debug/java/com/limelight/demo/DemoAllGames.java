package com.limelight.demo;

import android.content.Context;

import com.limelight.console.AllGamesActivity;
import com.limelight.console.GameDetailActivity;
import com.limelight.nativeapi.ConsoleRepository;

/** All games on the demo hosts: the canned library, and a game opens the demo's detail. */
public final class DemoAllGames extends AllGamesActivity {
    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override protected ConsoleRepository createRepository(String uniqueId) {
        return DemoConsole.repository(this);
    }

    @Override protected Class<? extends GameDetailActivity> detailActivity() {
        return DemoGameDetail.class;
    }
}
