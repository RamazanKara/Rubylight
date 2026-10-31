package com.limelight.preferences;

import com.limelight.ui.NightjarActivity;
import androidx.core.view.ViewCompat;
import com.google.android.material.button.MaterialButton;
import androidx.appcompat.app.AlertDialog;
import com.limelight.ui.NavigationDialogBuilder;
import android.content.res.TypedArray;
import android.os.Bundle;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.View;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;

import com.limelight.R;
import com.limelight.binding.input.ControllerButtonMap;
import com.limelight.binding.input.ControllerAxisMap;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.UiHelper;

import java.util.Map;

/** Lets the user choose what each physical controller button sends to the host. */
public class ControllerMappingActivity extends NightjarActivity {
    private TextView deviceView;
    private LinearLayout mappingList;
    private Button resetButton;
    private String deviceKey;
    private String deviceName;
    private ControllerButtonMap map;
    private ControllerAxisMap axisMap;
    private MaterialButton axisButton;
    private boolean capturing;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        UiHelper.setLocale(this);
        android.widget.FrameLayout page = TopAppBar.setUpPage(this, R.string.controller_mapping_title, this::finish);

        float density = getResources().getDisplayMetrics().density;
        int padding = Math.round(16 * density);
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);

        TextView intro = new TextView(this);
        intro.setText(R.string.controller_mapping_intro);
        intro.setTextAppearance(this, R.style.TextAppearance_Nightjar_Body);
        root.addView(intro);

        deviceView = new TextView(this);
        ViewCompat.setAccessibilityLiveRegion(deviceView, ViewCompat.ACCESSIBILITY_LIVE_REGION_POLITE);
        deviceView.setTextAppearance(this, R.style.TextAppearance_Nightjar_Label);
        deviceView.setTextColor(accentColor());
        deviceView.setPadding(0, padding * 3 / 2, 0, padding / 2);
        root.addView(deviceView);

        MaterialButton capture = new MaterialButton(this);
        capture.setId(R.id.controller_mapping_capture);
        capture.setText(R.string.controller_mapping_capture);
        capture.setOnClickListener(v -> {
            capturing = true;
            deviceView.setText(R.string.controller_mapping_press);
        });
        root.addView(capture);

        axisButton = new MaterialButton(this, null, R.attr.nightjarButtonTonalStyle);
        axisButton.setText(R.string.controller_axes_title);
        axisButton.setOnClickListener(v -> chooseAxis());
        root.addView(axisButton);

        MaterialButton testButton = new MaterialButton(this, null, R.attr.nightjarButtonTonalStyle);
        testButton.setText(R.string.controller_test_open);
        testButton.setOnClickListener(v -> startActivity(new android.content.Intent(this, ControllerTestActivity.class)));
        root.addView(testButton);

        mappingList = new LinearLayout(this);
        mappingList.setOrientation(LinearLayout.VERTICAL);
        root.addView(mappingList);

        resetButton = new MaterialButton(this, null, R.attr.nightjarButtonDestructiveStyle);
        resetButton.setText(R.string.controller_mapping_reset);
        resetButton.setOnClickListener(v -> {
            map = new ControllerButtonMap();
            map.save(this, deviceKey);
            axisMap = new ControllerAxisMap();
            axisMap.save(this, deviceKey);
            refresh();
        });
        root.addView(resetButton);

        TopAppBar.addScrollingBody(page, root);
        UiHelper.notifyNewRootView(this);

        selectSingleConnectedController();
        refresh();
    }

    protected void selectSingleConnectedController() {
        InputDevice found = null;
        for (int id : InputDevice.getDeviceIds()) {
            InputDevice device = InputDevice.getDevice(id);
            if (device == null || device.isVirtual() ||
                    (device.getSources() & InputDevice.SOURCE_GAMEPAD) != InputDevice.SOURCE_GAMEPAD) {
                continue;
            }
            if (found != null) {
                return; // Several controllers: wait for a button press to pick one
            }
            found = device;
        }
        if (found != null) {
            select(found.getVendorId(), found.getProductId(), found.getName());
        }
    }

    protected void select(int vendorId, int productId, String name) {
        deviceName = name == null || name.trim().isEmpty() ? getString(R.string.controller_mapping_unnamed) : name.trim();
        deviceKey = ControllerButtonMap.deviceKey(vendorId, productId, name);
        map = ControllerButtonMap.load(this, deviceKey);
        axisMap = ControllerAxisMap.load(this, deviceKey);
    }

    private void refresh() {
        rebuild();
        com.limelight.ui.SubPageFocus.apply(this, findViewById(R.id.page_content));
    }

    private void rebuild() {
        mappingList.removeAllViews();
        axisButton.setEnabled(deviceKey != null);
        if (deviceKey == null) {
            deviceView.setText(R.string.controller_mapping_waiting);
            resetButton.setVisibility(View.GONE);
            return;
        }
        deviceView.setText(deviceName);
        resetButton.setVisibility(map.isEmpty() && axisMap.isEmpty() ? View.GONE : View.VISIBLE);
        mappingList.addView(row(getString(R.string.controller_connect_as),
                connectAsName(map.connectAs()), v -> chooseConnectAs()));
        if (!axisMap.isEmpty()) {
            mappingList.addView(row(getString(R.string.controller_axes_title),
                    getString(R.string.controller_axes_configured), v -> chooseAxis()));
        }
        if (map.entries().isEmpty()) {
            mappingList.addView(row(getString(R.string.controller_mapping_none), null, null));
            return;
        }
        for (Map.Entry<Integer, Integer> entry : map.entries().entrySet()) {
            int source = entry.getKey();
            mappingList.addView(row(buttonName(source),
                    getString(R.string.controller_mapping_row, targetName(entry.getValue())),
                    v -> chooseTarget(source)));
        }
    }

    // Two-line rows with dividers, matching the settings list
    private View row(String title, String summary, View.OnClickListener onClick) {
        float density = getResources().getDisplayMetrics().density;
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.VERTICAL);
        int padding = Math.round(14 * density);
        row.setPadding(0, padding, 0, padding);
        TextView titleView = new TextView(this);
        titleView.setTextAppearance(this, R.style.TextAppearance_Nightjar_Body);
        titleView.setTextColor(primaryTextColor());
        titleView.setText(title);
        row.addView(titleView);
        if (summary != null) {
            TextView summaryView = new TextView(this);
            summaryView.setTextAppearance(this, R.style.TextAppearance_Nightjar_Supporting);
            summaryView.setText(summary);
            row.addView(summaryView);
            TypedArray attrs = obtainStyledAttributes(new int[] {android.R.attr.selectableItemBackground});
            row.setBackground(attrs.getDrawable(0));
            attrs.recycle();
            row.setOnClickListener(onClick);
            row.setFocusable(true);
            row.setMinimumHeight(Math.round(48 * density));
            ViewCompat.setScreenReaderFocusable(row, true);
        }
        LinearLayout wrapper = new LinearLayout(this);
        wrapper.setOrientation(LinearLayout.VERTICAL);
        wrapper.addView(row);
        View divider = new View(this);
        divider.setBackgroundColor(com.google.android.material.color.MaterialColors.getColor(this,
                com.google.android.material.R.attr.colorOutlineVariant, 0));
        wrapper.addView(divider, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                Math.max(1, Math.round(density))));
        return wrapper;
    }

    private int primaryTextColor() {
        TypedArray attrs = obtainStyledAttributes(new int[] {android.R.attr.textColorPrimary});
        int color = attrs.getColor(0, 0xFFFFFFFF);
        attrs.recycle();
        return color;
    }

    private void chooseConnectAs() {
        if (deviceKey == null) return;
        byte[] choices = ControllerButtonMap.CONNECT_AS_CHOICES;
        CharSequence[] labels = new CharSequence[choices.length];
        int selected = 0;
        for (int i = 0; i < choices.length; i++) {
            labels[i] = connectAsName(choices[i]);
            if (choices[i] == map.connectAs()) selected = i;
        }
        UiHelper.showDialog(this, new NavigationDialogBuilder(this).setTitle(R.string.controller_connect_as)
                .setSingleChoiceItems(labels, selected, (dialog, which) -> {
                    map.setConnectAs(choices[which]);
                    map.save(this, deviceKey); refresh(); dialog.dismiss();
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    private String connectAsName(byte type) {
        switch (type) {
            case ControllerButtonMap.CONNECT_AS_XBOX: return getString(R.string.controller_connect_as_xbox);
            case ControllerButtonMap.CONNECT_AS_PLAYSTATION: return getString(R.string.controller_connect_as_playstation);
            case ControllerButtonMap.CONNECT_AS_NINTENDO: return getString(R.string.controller_connect_as_nintendo);
            default: return getString(R.string.controller_connect_as_automatic);
        }
    }

    private void chooseAxis() {
        if (deviceKey == null) return;
        String[] names = getResources().getStringArray(R.array.controller_axes);
        UiHelper.showDialog(this, new NavigationDialogBuilder(this).setTitle(R.string.controller_axes_title)
                .setItems(names, (dialog, which) -> {
                    if (which < 6) chooseAxisSource(which);
                    else chooseDpadTarget(which - 6);
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    private void chooseAxisSource(int destination) {
        String[] names = getResources().getStringArray(R.array.controller_axes);
        boolean stick = destination < 4;
        int first = stick ? 0 : 4, count = stick ? 4 : 2;
        String[] choices = new String[1 + count * (stick ? 2 : 1)];
        choices[0] = getString(R.string.controller_mapping_disabled);
        int selected = 0;
        for (int i = 0; i < count; i++) {
            choices[1 + i] = names[first + i];
            if (stick) choices[1 + count + i] = getString(R.string.controller_axes_inverted, names[first + i]);
            if (axisMap.source(destination) == first + i) selected = 1 + i + (axisMap.inverted(destination) ? count : 0);
        }
        UiHelper.showDialog(this, new NavigationDialogBuilder(this)
                .setTitle(getString(R.string.controller_axes_source, names[destination]))
                .setSingleChoiceItems(choices, selected, (dialog, which) -> {
                    axisMap.setAxis(destination, which == 0 ? -1 : first + (which - 1) % count, which > count);
                    axisMap.save(this, deviceKey); refresh(); dialog.dismiss();
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    private void chooseDpadTarget(int direction) {
        String[] labels = getResources().getStringArray(R.array.controller_dpad_targets);
        int selected = 0;
        for (int i = 0; i < ControllerAxisMap.DPAD_TARGETS.length; i++)
            if (axisMap.dpadTarget(direction) == ControllerAxisMap.DPAD_TARGETS[i]) selected = i;
        UiHelper.showDialog(this, new NavigationDialogBuilder(this)
                .setTitle(getResources().getStringArray(R.array.controller_axes)[direction + 6])
                .setSingleChoiceItems(labels, selected, (dialog, which) -> {
                    axisMap.setDpad(direction, ControllerAxisMap.DPAD_TARGETS[which]);
                    axisMap.save(this, deviceKey); refresh(); dialog.dismiss();
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    private int accentColor() {
        TypedArray attrs = obtainStyledAttributes(new int[] {android.R.attr.colorAccent});
        int color = attrs.getColor(0, 0xFF80CBC4);
        attrs.recycle();
        return color;
    }

    private static boolean isControllerEvent(KeyEvent event) {
        return (event.getSource() & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD ||
                (event.getSource() & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK;
    }

    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        if (capturing && (event.getKeyCode() == KeyEvent.KEYCODE_BACK || event.getKeyCode() == KeyEvent.KEYCODE_ESCAPE)) {
            if (event.getAction() == KeyEvent.ACTION_UP) {
                capturing = false;
                refresh();
            }
            return true;
        }
        if (!capturing || !isControllerEvent(event) || !ControllerButtonMap.isRemappableSource(event.getKeyCode())) {
            return super.dispatchKeyEvent(event);
        }
        // Act on release so the press cannot leak into the dialog that opens next
        if (event.getAction() == KeyEvent.ACTION_UP) {
            capturing = false;
            InputDevice device = event.getDevice();
            if (device != null) {
                select(device.getVendorId(), device.getProductId(), device.getName());
            }
            else {
                select(0, 0, null);
            }
            refresh();
            chooseTarget(event.getKeyCode());
        }
        return true;
    }

    private void chooseTarget(int source) {
        // The button itself is offered once, as the default entry
        int[] targets = new int[ControllerButtonMap.TARGETS.length];
        int count = 0;
        for (int target : ControllerButtonMap.TARGETS) {
            if (target != source) {
                targets[count++] = target;
            }
        }
        CharSequence[] labels = new CharSequence[count + 1];
        labels[0] = getString(R.string.controller_mapping_default, buttonName(source));
        for (int i = 0; i < count; i++) {
            labels[i + 1] = targetName(targets[i]);
        }
        UiHelper.showDialog(this, new NavigationDialogBuilder(this)
                .setTitle(getString(R.string.controller_mapping_choose, buttonName(source)))
                .setItems(labels, (dialog, which) -> {
                    if (which == 0) {
                        map.remove(source);
                    }
                    else {
                        map.put(source, targets[which - 1]);
                    }
                    map.save(this, deviceKey);
                    refresh();
                })
                .setNegativeButton(android.R.string.cancel, null)
                .create());
    }

    private String targetName(int keyCode) {
        return keyCode == ControllerButtonMap.DISABLED ?
                getString(R.string.controller_mapping_disabled) : buttonName(keyCode);
    }

    private String buttonName(int keyCode) {
        switch (keyCode) {
            case KeyEvent.KEYCODE_BUTTON_A: return "A";
            case KeyEvent.KEYCODE_BUTTON_B: return "B";
            case KeyEvent.KEYCODE_BUTTON_C: return "C";
            case KeyEvent.KEYCODE_BUTTON_X: return "X";
            case KeyEvent.KEYCODE_BUTTON_Y: return "Y";
            case KeyEvent.KEYCODE_BUTTON_Z: return "Z";
            case KeyEvent.KEYCODE_BUTTON_L1: return "LB";
            case KeyEvent.KEYCODE_BUTTON_R1: return "RB";
            case KeyEvent.KEYCODE_BUTTON_THUMBL: return getString(R.string.controller_button_left_stick);
            case KeyEvent.KEYCODE_BUTTON_THUMBR: return getString(R.string.controller_button_right_stick);
            case KeyEvent.KEYCODE_BUTTON_START: return getString(R.string.controller_button_start);
            case KeyEvent.KEYCODE_BUTTON_SELECT: return getString(R.string.controller_button_select);
            case KeyEvent.KEYCODE_BUTTON_MODE: return getString(R.string.controller_button_guide);
            case ControllerButtonMap.TARGET_TOUCHPAD: return getString(R.string.controller_button_touchpad);
            case KeyEvent.KEYCODE_MEDIA_RECORD:
            case ControllerButtonMap.TARGET_MISC: return getString(R.string.controller_button_misc);
            case ControllerButtonMap.TARGET_PADDLE1: return getString(R.string.controller_button_paddle, 1);
            case ControllerButtonMap.TARGET_PADDLE2: return getString(R.string.controller_button_paddle, 2);
            case ControllerButtonMap.TARGET_PADDLE3: return getString(R.string.controller_button_paddle, 3);
            case ControllerButtonMap.TARGET_PADDLE4: return getString(R.string.controller_button_paddle, 4);
            default:
                if (keyCode >= KeyEvent.KEYCODE_BUTTON_1 && keyCode <= KeyEvent.KEYCODE_BUTTON_16) {
                    return getString(R.string.controller_button_number, keyCode - KeyEvent.KEYCODE_BUTTON_1 + 1);
                }
                return KeyEvent.keyCodeToString(keyCode);
        }
    }
}
