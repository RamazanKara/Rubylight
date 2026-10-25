package com.limelight.demo;

import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.widget.Toast;

import com.limelight.AppView;
import com.limelight.PcView;
import com.limelight.R;
import com.limelight.console.ConsoleHomeActivity;
import com.limelight.console.GameDetailActivity;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.preferences.StreamSettings;

/** Home on the demo hosts: canned library, and every other screen it opens is the demo's own. */
public final class DemoConsoleHome extends ConsoleHomeActivity {
    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override protected ConsoleRepository createRepository(String uniqueId) {
        return DemoConsole.repository(this);
    }

    private boolean optionsOpened;

    @Override protected void onResume() {
        super.onResume();
        // Once: coming back from the PC settings the sheet is not wanted again.
        if (!optionsOpened && "options".equals(getIntent().getStringExtra("state"))) {
            optionsOpened = true;
            openOptions(0);
        }
    }

    /** Presses the three dots on the Living-room PC's card once Home has painted it. */
    private void openOptions(int attempt) {
        androidx.recyclerview.widget.RecyclerView list = findViewById(R.id.console_list);
        android.view.View more = moreOf(list);
        if (more != null) {
            more.performClick();
            return;
        }
        // A short screen leaves the PCs row below the fold, where it is not laid out: scroll to it.
        if (attempt == 3 && list.getAdapter() != null && list.getAdapter().getItemCount() > 0) {
            list.scrollToPosition(list.getAdapter().getItemCount() - 1);
        }
        if (attempt < 30) list.postDelayed(() -> openOptions(attempt + 1), 300);
    }

    /** The three dots of the Living-room PC's card, in a shelf or in the grid; null while it is not laid out. */
    private static android.view.View moreOf(android.view.View view) {
        if (view.getId() == R.id.console_more) {
            // The card's title may sit beside the dots or higher up in the card.
            for (android.view.ViewParent up = view.getParent(); up instanceof android.view.View; up = up.getParent()) {
                android.widget.TextView title = ((android.view.View) up).findViewById(R.id.console_title);
                if (title != null) {
                    return "Living-room PC".contentEquals(title.getText()) ? view : null;
                }
            }
        }
        if (view instanceof android.view.ViewGroup) {
            android.view.ViewGroup group = (android.view.ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                android.view.View found = moreOf(group.getChildAt(i));
                if (found != null) return found;
            }
        }
        return null;
    }

    @Override protected Class<? extends GameDetailActivity> detailActivity() {
        return DemoGameDetail.class;
    }

    @Override public void startActivity(Intent intent, Bundle options) {
        String target = intent.getComponent() != null ? intent.getComponent().getClassName() : null;
        if (AppView.class.getName().equals(target)) {
            super.startActivity(new Intent(intent).setClass(this, DemoAppView.class), options);
        } else if (PcView.class.getName().equals(target)) {
            super.startActivity(new Intent(intent).setClass(this, DemoPcView.class), options);
        } else if (StreamSettings.class.getName().equals(target)) {
            super.startActivity(new Intent(this, DemoStreamSettings.class), options);
        } else if (target != null && target.startsWith("com.limelight.demo.")) {
            super.startActivity(intent, options);
        } else {
            Toast.makeText(this, R.string.demo_offline, Toast.LENGTH_SHORT).show();
        }
    }
}
