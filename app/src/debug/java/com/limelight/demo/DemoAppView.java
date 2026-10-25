package com.limelight.demo;

import android.content.Context;
import android.content.Intent;
import android.widget.AbsListView;

import com.limelight.AppView;
import com.limelight.utils.ServerHelper;

public final class DemoAppView extends AppView {
    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override public void receiveAbsListView(AbsListView list) {
        super.receiveAbsListView(list);
        // The real item click starts the game from the poster with the same window animation.
        list.setOnItemClickListener((parent, view, position, id) -> startActivity(
                new Intent(this, DemoGameActivity.class).putExtra("state", "compact"),
                ServerHelper.launchOptions(this, view)));
    }
}
