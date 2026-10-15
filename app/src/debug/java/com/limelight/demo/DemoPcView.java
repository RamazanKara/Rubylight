package com.limelight.demo;

import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.widget.AbsListView;
import android.widget.Toast;

import com.limelight.AppView;
import com.limelight.PcView;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.HostStreamSettings;
import com.limelight.preferences.StreamSettings;

public final class DemoPcView extends PcView {
    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override public void startActivityForResult(Intent intent, int requestCode, Bundle options) {
        String target = intent.getComponent() != null ? intent.getComponent().getClassName() : null;
        if (StreamSettings.class.getName().equals(target)) {
            super.startActivityForResult(new Intent(this, DemoStreamSettings.class), requestCode, options);
        } else if (HostStreamSettings.class.getName().equals(target)) {
            // PC settings… in a PC's menu opens the real per-PC screen on the fixtures.
            super.startActivityForResult(new Intent(intent).setClass(this, DemoHostStreamSettings.class), requestCode, options);
        } else {
            Toast.makeText(this, R.string.demo_offline, Toast.LENGTH_SHORT).show();
        }
    }

    @Override public void receiveAbsListView(AbsListView list) {
        super.receiveAbsListView(list);
        // Long presses and the options button open the real PC menu; taps stay offline.
        list.setOnItemClickListener((parent, view, position, id) -> open((ComputerObject) parent.getItemAtPosition(position)));
    }

    @Override protected void doWakeOnLan(ComputerDetails computer) {
        Toast.makeText(this, R.string.demo_offline, Toast.LENGTH_SHORT).show();
    }

    private void open(ComputerObject computer) {
        if (computer.details.uuid.equals(DemoFixtures.PC_UUID)) {
            super.startActivityForResult(new Intent(this, DemoAppView.class)
                    .putExtra(AppView.UUID_EXTRA, computer.details.uuid)
                    .putExtra(AppView.NAME_EXTRA, computer.details.name), -1, null);
            return;
        }
        Toast.makeText(this, R.string.demo_offline, Toast.LENGTH_SHORT).show();
    }
}
