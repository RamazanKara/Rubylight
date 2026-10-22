package com.limelight.demo;

import android.content.Context;
import android.content.Intent;
import android.widget.Toast;

import com.limelight.R;
import com.limelight.console.GameDetailActivity;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

/** The game detail on the demo hosts: Play opens the demo stream, ending a game only says it is a demo. */
public final class DemoGameDetail extends GameDetailActivity {
    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override protected ConsoleRepository createRepository(String uniqueId) {
        return DemoConsole.repository(this);
    }

    @Override protected void play(NvApp app, ComputerDetails computer) {
        startActivity(new Intent(this, DemoGameActivity.class).putExtra("state", "compact"));
    }

    @Override protected void end(NvApp app, ComputerDetails computer, Runnable onComplete) {
        runOnUiThread(() -> Toast.makeText(this, R.string.demo_offline, Toast.LENGTH_SHORT).show());
        if (onComplete != null) onComplete.run();
    }
}
