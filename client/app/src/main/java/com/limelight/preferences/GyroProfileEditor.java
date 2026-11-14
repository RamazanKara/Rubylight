package com.limelight.preferences;

import android.app.Activity;
import android.content.Context;
import android.content.DialogInterface;
import android.view.Gravity;
import android.view.InputDevice;
import android.view.ViewGroup;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.button.MaterialButtonToggleGroup;
import com.google.android.material.checkbox.MaterialCheckBox;
import com.google.android.material.color.MaterialColors;
import com.google.android.material.slider.Slider;
import com.google.android.material.textfield.TextInputEditText;
import com.google.android.material.textfield.TextInputLayout;
import com.limelight.R;
import com.limelight.binding.input.GyroAxisTransformer;
import com.limelight.binding.input.GyroMappingPreferences;
import com.limelight.binding.input.GyroMappingPreferences.Profile;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.ui.StepSlider;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * Settings › Controls › Gyroscope mapping: picks each connected controller's mapping and edits
 * the custom profiles.
 *
 * Ported from jvanderv42's Nightjar fork (jvanderv42/nightjar, GPL-3.0, commit d0e14cf25,
 * app/src/main/java/com/limelight/preferences/GyroProfileEditor.java), rebuilt on the app's
 * dialog theme and translated text.
 */
public final class GyroProfileEditor {
    private static final String[] AXIS_NAMES = {"X", "Y", "Z"};
    private static final int[] OUTPUT_LABELS = {R.string.gyro_profile_axis_x, R.string.gyro_profile_axis_y,
            R.string.gyro_profile_axis_z};

    private GyroProfileEditor() {}

    static List<InputDevice> controllers() {
        List<InputDevice> devices = new ArrayList<>();
        for (int id : InputDevice.getDeviceIds()) {
            InputDevice device = InputDevice.getDevice(id);
            if (device == null || device.isVirtual() || device.getDescriptor() == null) continue;
            int sources = device.getSources();
            if ((sources & InputDevice.SOURCE_GAMEPAD) != InputDevice.SOURCE_GAMEPAD
                    && (sources & InputDevice.SOURCE_JOYSTICK) != InputDevice.SOURCE_JOYSTICK) continue;
            devices.add(device);
        }
        return devices;
    }

    private static String choiceName(Context context, String choice) {
        if (GyroMappingPreferences.FLYDIGI.equals(choice)) return context.getString(R.string.gyro_choice_flydigi);
        if (choice.startsWith(GyroMappingPreferences.CUSTOM_PREFIX)) {
            for (Profile p : GyroMappingPreferences.profiles(context)) {
                if (choice.equals(GyroMappingPreferences.CUSTOM_PREFIX + p.id)) return p.name;
            }
        }
        return context.getString(R.string.gyro_choice_standard);
    }

    private static String deviceName(Context context, InputDevice device, List<InputDevice> all) {
        String name = device.getName() == null || device.getName().trim().isEmpty()
                ? context.getString(R.string.controller_mapping_unnamed) : device.getName().trim();
        int sameName = 0;
        for (InputDevice other : all) {
            if (name.equals(other.getName() == null ? "" : other.getName().trim())) sameName++;
        }
        // Flydigi pads in Switch mode both report "Pro Controller": tell such pads apart.
        if (sameName > 1) {
            String descriptor = device.getDescriptor();
            name += " · " + descriptor.substring(0, Math.min(6, descriptor.length()));
        }
        return name;
    }

    public static void open(Activity activity) {
        List<InputDevice> devices = controllers();
        List<String> labels = new ArrayList<>();
        for (InputDevice device : devices) {
            labels.add(activity.getString(R.string.gyro_mapping_device_row, deviceName(activity, device, devices),
                    choiceName(activity, GyroMappingPreferences.getChoice(activity, device.getDescriptor()))));
        }
        labels.add(activity.getString(R.string.gyro_profiles_title));
        new NavigationDialogBuilder(activity)
                .setTitle(R.string.title_gyro_mapping)
                .setItems(labels.toArray(new String[0]), (dialog, which) -> {
                    if (which == devices.size()) manageProfiles(activity);
                    else chooseMapping(activity, devices.get(which), deviceName(activity, devices.get(which), devices));
                })
                .setNegativeButton(android.R.string.cancel, null)
                .show();
    }

    private static void chooseMapping(Activity activity, InputDevice device, String name) {
        List<Profile> profiles = GyroMappingPreferences.profiles(activity);
        List<String> options = new ArrayList<>();
        options.add(activity.getString(R.string.gyro_choice_standard));
        options.add(activity.getString(R.string.gyro_choice_flydigi));
        for (Profile p : profiles) options.add(activity.getString(R.string.gyro_choice_custom, p.name));
        options.add(activity.getString(R.string.gyro_choice_new));

        String selected = GyroMappingPreferences.getChoice(activity, device.getDescriptor());
        int checked = GyroMappingPreferences.FLYDIGI.equals(selected) ? 1 : 0;
        for (int i = 0; i < profiles.size(); i++) {
            if (selected.equals(GyroMappingPreferences.CUSTOM_PREFIX + profiles.get(i).id)) {
                checked = 2 + i;
                break;
            }
        }
        new NavigationDialogBuilder(activity)
                .setTitle(activity.getString(R.string.gyro_mapping_choice_title, name))
                .setSingleChoiceItems(options.toArray(new String[0]), checked, (dialog, index) -> {
                    dialog.dismiss();
                    if (index == options.size() - 1) {
                        editProfile(activity, null, device);
                        return;
                    }
                    String choice = index == 0 ? GyroMappingPreferences.STANDARD
                            : index == 1 ? GyroMappingPreferences.FLYDIGI
                            : GyroMappingPreferences.CUSTOM_PREFIX + profiles.get(index - 2).id;
                    GyroMappingPreferences.setChoice(activity, device, choice);
                    Toast.makeText(activity, R.string.gyro_mapping_saved, Toast.LENGTH_LONG).show();
                })
                .setNegativeButton(android.R.string.cancel, null)
                .show();
    }

    private static void manageProfiles(Activity activity) {
        List<Profile> profiles = GyroMappingPreferences.profiles(activity);
        List<String> items = new ArrayList<>();
        items.add(activity.getString(R.string.gyro_profile_new));
        for (Profile profile : profiles) items.add(profile.name);
        new NavigationDialogBuilder(activity)
                .setTitle(R.string.gyro_profiles_title)
                .setItems(items.toArray(new String[0]), (dialog, which) -> {
                    if (which == 0) editProfile(activity, null, null);
                    else profileActions(activity, profiles.get(which - 1));
                })
                .setNegativeButton(android.R.string.cancel, null)
                .show();
    }

    private static void profileActions(Activity activity, Profile profile) {
        String[] actions = {activity.getString(R.string.gyro_profile_edit),
                activity.getString(R.string.gyro_profile_duplicate), activity.getString(R.string.gyro_profile_delete)};
        new NavigationDialogBuilder(activity)
                .setTitle(profile.name)
                .setItems(actions, (dialog, which) -> {
                    if (which == 0) editProfile(activity, profile, null);
                    else if (which == 1) editProfile(activity, new Profile(null,
                            activity.getString(R.string.gyro_profile_copy_name, profile.name),
                            profile.axes, profile.signs,
                            profile.horizontalSensitivity, profile.verticalSensitivity), null);
                    else confirmDelete(activity, profile);
                })
                .setNegativeButton(android.R.string.cancel, null)
                .show();
    }

    private static void confirmDelete(Activity activity, Profile profile) {
        new NavigationDialogBuilder(activity)
                .setTitle(R.string.gyro_profile_delete_title)
                .setMessage(activity.getString(R.string.gyro_profile_delete_message, profile.name))
                .setPositiveButton(R.string.gyro_profile_delete, (dialog, which) -> {
                    GyroMappingPreferences.deleteProfile(activity, profile.id);
                    Toast.makeText(activity, R.string.gyro_profile_deleted, Toast.LENGTH_SHORT).show();
                })
                .setNegativeButton(android.R.string.cancel, null)
                .show();
    }

    private static TextView text(Context context, CharSequence value, int appearance, int colorAttr) {
        TextView view = new TextView(context);
        view.setText(value);
        // A bare TextView takes only the app's face, black at the smallest size on the night glass.
        view.setTextAppearance(context, appearance);
        view.setTextColor(MaterialColors.getColor(view, colorAttr));
        return view;
    }

    static String sensitivityText(float value) {
        return String.format(Locale.US, "%.2f×", value);
    }

    /**
     * Edits {@code source}, or a new profile when it is null; a new profile made from a controller's
     * own picker becomes that controller's choice.
     */
    static void editProfile(Activity activity, Profile source, InputDevice assignTo) {
        NavigationDialogBuilder builder = new NavigationDialogBuilder(activity);
        Context context = builder.getContext();
        float density = context.getResources().getDisplayMetrics().density;
        int pad = Math.round(24 * density);
        LinearLayout form = new LinearLayout(context);
        form.setOrientation(LinearLayout.VERTICAL);
        form.setPadding(pad, pad / 3, pad, 0);

        form.addView(text(context, context.getString(R.string.gyro_profile_help),
                R.style.TextAppearance_Nightjar_Body, com.google.android.material.R.attr.colorOnSurfaceVariant));

        TextInputLayout nameBox = new TextInputLayout(context);
        nameBox.setHint(R.string.gyro_profile_name);
        TextInputEditText name = new TextInputEditText(nameBox.getContext());
        name.setSingleLine(true);
        name.setText(source == null ? "" : source.name);
        nameBox.addView(name);
        LinearLayout.LayoutParams nameParams = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        nameParams.topMargin = pad / 2;
        form.addView(nameBox, nameParams);

        MaterialButtonToggleGroup[] axes = new MaterialButtonToggleGroup[3];
        MaterialCheckBox[] inverted = new MaterialCheckBox[3];
        Map<Integer, Integer> axisOfButton = new HashMap<>();
        for (int i = 0; i < 3; i++) {
            TextView label = text(context, context.getString(OUTPUT_LABELS[i]),
                    R.style.TextAppearance_Nightjar_Body, com.google.android.material.R.attr.colorOnSurface);
            LinearLayout.LayoutParams labelParams = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            labelParams.topMargin = pad / 2;
            form.addView(label, labelParams);

            LinearLayout row = new LinearLayout(context);
            row.setOrientation(LinearLayout.HORIZONTAL);
            row.setGravity(Gravity.CENTER_VERTICAL);
            MaterialButtonToggleGroup group = new MaterialButtonToggleGroup(context);
            group.setSingleSelection(true);
            group.setSelectionRequired(true);
            int selected = source == null ? i : source.axes[i];
            for (int axis = 0; axis < 3; axis++) {
                MaterialButton button = new MaterialButton(context, null,
                        com.google.android.material.R.attr.materialButtonOutlinedStyle);
                button.setId(android.view.View.generateViewId());
                button.setText(AXIS_NAMES[axis]);
                button.setContentDescription(context.getString(R.string.gyro_profile_source_axis,
                        context.getString(OUTPUT_LABELS[i]), AXIS_NAMES[axis]));
                axisOfButton.put(button.getId(), axis);
                group.addView(button);
                if (axis == selected) group.check(button.getId());
            }
            row.addView(group);
            axes[i] = group;

            MaterialCheckBox invert = new MaterialCheckBox(context);
            invert.setText(R.string.gyro_profile_invert);
            invert.setChecked(source != null && source.signs[i] == -1);
            LinearLayout.LayoutParams invertParams = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            invertParams.setMarginStart(pad / 2);
            row.addView(invert, invertParams);
            inverted[i] = invert;
            form.addView(row);
        }

        // 0.10× to 3.00× in steps of 0.05×, held as whole percents so the slider's steps stay exact.
        float[] sensitivity = {source == null ? 1f : source.horizontalSensitivity,
                source == null ? 1f : source.verticalSensitivity};
        int[] sensitivityLabels = {R.string.gyro_profile_horizontal, R.string.gyro_profile_vertical};
        for (int i = 0; i < 2; i++) {
            final int index = i;
            LinearLayout heading = new LinearLayout(context);
            heading.setOrientation(LinearLayout.HORIZONTAL);
            TextView title = text(context, context.getString(sensitivityLabels[i]),
                    R.style.TextAppearance_Nightjar_Body, com.google.android.material.R.attr.colorOnSurface);
            heading.addView(title, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1));
            TextView value = text(context, sensitivityText(sensitivity[i]),
                    R.style.TextAppearance_Nightjar_Body, com.google.android.material.R.attr.colorOnSurface);
            heading.addView(value);
            LinearLayout.LayoutParams headingParams = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            headingParams.topMargin = pad / 2;
            form.addView(heading, headingParams);

            // The app's slider: no step dots, and a halo a TV remote can see while it has focus.
            Slider slider = new StepSlider(context);
            slider.setValueFrom(Math.round(GyroAxisTransformer.MIN_SENSITIVITY * 100));
            slider.setValueTo(Math.round(GyroAxisTransformer.MAX_SENSITIVITY * 100));
            slider.setStepSize(5);
            slider.setValue(Math.round(sensitivity[i] * 20) * 5);
            slider.setContentDescription(context.getString(sensitivityLabels[i]));
            StepSlider.showValue(slider, percent -> sensitivityText(percent / 100f));
            slider.addOnChangeListener((view, percent, fromUser) -> {
                sensitivity[index] = Math.round(percent) / 100f;
                value.setText(sensitivityText(sensitivity[index]));
            });
            form.addView(slider, new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        }

        ScrollView scroll = new ScrollView(context);
        scroll.setFillViewport(true);
        scroll.setClipToPadding(false);
        scroll.addView(form);
        // A duplicate is new too: its id is not saved yet.
        boolean isNew = source == null || GyroMappingPreferences.profiles(activity).stream()
                .noneMatch(p -> p.id.equals(source.id));
        AlertDialog dialog = builder
                .setTitle(isNew ? R.string.gyro_profile_create_title : R.string.gyro_profile_edit_title)
                .setView(scroll)
                .setPositiveButton(R.string.keyboard_shortcuts_save, null)
                .setNegativeButton(android.R.string.cancel, null)
                .create();
        dialog.setOnShowListener(ignored -> dialog.getButton(DialogInterface.BUTTON_POSITIVE).setOnClickListener(v -> {
            int[] selectedAxes = new int[3];
            int[] signs = new int[3];
            for (int i = 0; i < 3; i++) {
                Integer axis = axisOfButton.get(axes[i].getCheckedButtonId());
                selectedAxes[i] = axis == null ? i : axis;
                signs[i] = inverted[i].isChecked() ? -1 : 1;
            }
            String profileName = name.getText() == null ? "" : name.getText().toString().trim();
            if (profileName.isEmpty()) {
                nameBox.setError(context.getString(R.string.gyro_profile_name_required));
                return;
            }
            nameBox.setError(null);
            if (!GyroMappingPreferences.validAxes(selectedAxes, signs)) {
                Toast.makeText(activity, R.string.gyro_profile_axes_unique, Toast.LENGTH_LONG).show();
                return;
            }
            Profile profile = new Profile(source == null ? null : source.id, profileName, selectedAxes, signs,
                    sensitivity[0], sensitivity[1]);
            GyroMappingPreferences.saveProfile(activity, profile);
            if (assignTo != null) {
                GyroMappingPreferences.setChoice(activity, assignTo, GyroMappingPreferences.CUSTOM_PREFIX + profile.id);
            }
            Toast.makeText(activity, R.string.gyro_profile_saved, Toast.LENGTH_LONG).show();
            dialog.dismiss();
        }));
        dialog.show();
    }
}
