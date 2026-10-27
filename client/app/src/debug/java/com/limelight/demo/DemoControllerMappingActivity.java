package com.limelight.demo;

import android.content.Context;
import android.view.KeyEvent;

import com.limelight.binding.input.ControllerButtonMap;
import com.limelight.preferences.ControllerMappingActivity;

public final class DemoControllerMappingActivity extends ControllerMappingActivity {
    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override protected void selectSingleConnectedController() {
        ControllerButtonMap map = new ControllerButtonMap();
        map.put(KeyEvent.KEYCODE_BUTTON_A, KeyEvent.KEYCODE_BUTTON_B);
        map.put(KeyEvent.KEYCODE_BUTTON_B, KeyEvent.KEYCODE_BUTTON_A);
        map.put(KeyEvent.KEYCODE_BUTTON_X, KeyEvent.KEYCODE_BUTTON_Y);
        map.put(KeyEvent.KEYCODE_BUTTON_Y, KeyEvent.KEYCODE_BUTTON_X);
        map.save(this, ControllerButtonMap.deviceKey(0, 0, "Demo wireless controller"));
        select(0, 0, "Demo wireless controller");
    }
}
