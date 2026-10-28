package com.limelight.preferences;

import android.Manifest;
import android.os.Build;
import android.os.Bundle;
import android.preference.Preference;
import android.preference.PreferenceCategory;
import android.preference.PreferenceFragment;
import android.preference.PreferenceScreen;
import android.text.InputFilter;
import android.text.InputType;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ListView;
import android.widget.Toast;

import androidx.appcompat.app.AlertDialog;
import com.limelight.ui.NightjarActivity;

import com.limelight.ui.NavigationDialogBuilder;
import com.google.android.material.textfield.TextInputEditText;
import com.google.android.material.textfield.TextInputLayout;
import com.limelight.R;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.UiHelper;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeSet;

/**
 * Named profiles: make, rename, duplicate and delete them, pick the one in use, and optionally let
 * the network pick one when a stream starts. Each profile's values open in {@link HostStreamSettings}.
 */
public class StreamProfilesActivity extends NightjarActivity {
    private static final int LOCATION_REQUEST = 1;
    static final StreamLayers.Network[] RULE_NETWORKS = {
            StreamLayers.Network.MOBILE, StreamLayers.Network.HOME_WIFI, StreamLayers.Network.WIFI};

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        UiHelper.setLocale(this);
        FrameLayout content = TopAppBar.setUpPage(this, R.string.profiles_title, this::finish);
        TopAppBar.configure(this, R.id.page_toolbar, getString(R.string.profiles_title), R.menu.host_profile)
                .setOnMenuItemClickListener(item -> {
                    UiHelper.showDialog(this, new NavigationDialogBuilder(this).setTitle(R.string.profiles_title)
                            .setMessage(getString(R.string.profiles_order) + "\n\n" + getString(R.string.profiles_privacy))
                            .setPositiveButton(android.R.string.ok, null).create());
                    return true;
                });
        if (state == null) {
            getFragmentManager().beginTransaction().replace(content.getId(), new ProfilesFragment()).commit();
        }
        UiHelper.notifyNewRootView(this);
    }

    private ProfilesFragment fragment() {
        return (ProfilesFragment) getFragmentManager().findFragmentById(R.id.page_content);
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(requestCode, permissions, results);
        if (requestCode != LOCATION_REQUEST) return;
        ProfilesFragment fragment = fragment();
        if (StreamNetwork.canReadWifiName(this)) {
            if (fragment != null) fragment.addCurrentWifi();
        } else {
            Toast.makeText(this, R.string.profiles_location_denied, Toast.LENGTH_LONG).show();
            if (fragment != null) fragment.refresh();
        }
    }

    public static class ProfilesFragment extends PreferenceFragment {
        private StreamProfilesActivity activity;
        private PreferenceCategory list, choice, home;

        @Override
        public void onActivityCreated(Bundle state) {
            super.onActivityCreated(state);
            activity = (StreamProfilesActivity) getActivity();
            PreferenceScreen screen = getPreferenceManager().createPreferenceScreen(activity);
            setPreferenceScreen(screen);
            list = category(screen, R.string.profiles_section_list);
            choice = category(screen, R.string.profiles_section_choice);
            home = category(screen, R.string.profiles_home_networks);
            Preference footer = new Preference(activity);
            footer.setLayoutResource(R.layout.settings_preference);
            footer.setKey("order");
            footer.setSummary(getString(R.string.profiles_order) + "\n\n" + getString(R.string.profiles_privacy));
            footer.setSelectable(false);
            screen.addPreference(footer);
            refresh();
        }

        @Override
        public void onResume() {
            super.onResume();
            if (activity != null) refresh();
        }

        private PreferenceCategory category(PreferenceScreen screen, int title) {
            PreferenceCategory category = new PreferenceCategory(activity);
            category.setTitle(title);
            category.setLayoutResource(R.layout.settings_category);
            screen.addPreference(category);
            return category;
        }

        private Preference row(PreferenceCategory category, String key, CharSequence title, CharSequence summary,
                               Runnable onClick) {
            Preference preference = new Preference(activity);
            preference.setKey(key);
            preference.setTitle(title);
            preference.setSummary(summary);
            preference.setLayoutResource(R.layout.settings_preference);
            preference.setPersistent(false);
            if (onClick == null) {
                preference.setSelectable(false);
            } else {
                preference.setOnPreferenceClickListener(pref -> {
                    onClick.run();
                    return true;
                });
            }
            category.addPreference(preference);
            return preference;
        }

        private String profileName(StreamProfiles.Snapshot profiles, String id, int none) {
            return id == null ? getString(none) : profiles.exists(id) ? profiles.name(id) : getString(none);
        }

        void refresh() {
            StreamProfiles.Snapshot profiles = StreamProfiles.load(activity);
            list.removeAll();
            for (Map.Entry<String, String> entry : profiles.names.entrySet()) {
                String id = entry.getKey();
                row(list, "profile:" + id, entry.getValue(), profileSummary(profiles, id), () -> showProfileActions(id));
            }
            row(list, "new", getString(R.string.profiles_new), getString(R.string.profiles_new_summary),
                    () -> askName(R.string.profiles_new, "", null, name -> {
                        String id = StreamProfiles.create(activity, name, new HostStreamProfile());
                        if (id == null) {
                            Toast.makeText(activity, getString(R.string.profiles_too_many, StreamProfiles.MAX_PROFILES),
                                    Toast.LENGTH_LONG).show();
                            return;
                        }
                        HostStreamSettings.showProfile(activity, id);
                    }));

            choice.removeAll();
            row(choice, "in_use", getString(R.string.profiles_in_use),
                    profileName(profiles, profiles.activeId, R.string.profiles_none), this::pickInUse);
            MaterialSwitchPreference auto = new MaterialSwitchPreference(activity, null);
            auto.setKey("auto");
            auto.setLayoutResource(R.layout.settings_preference);
            auto.setPersistent(false);
            auto.setTitle(R.string.profiles_auto);
            auto.setChecked(profiles.auto);
            auto.setSummary(profiles.auto ? R.string.profiles_auto_summary_on : R.string.profiles_auto_summary_off);
            auto.setOnPreferenceChangeListener((pref, value) -> {
                StreamProfiles.setAuto(activity, (Boolean) value);
                refresh();
                return true;
            });
            choice.addPreference(auto);
            for (StreamLayers.Network network : RULE_NETWORKS) {
                // Mobile data only where there is mobile data, unless a rule set earlier still acts.
                if (network == StreamLayers.Network.MOBILE && profiles.rule(network) == null &&
                        !SettingRules.hasMobileData(activity)) {
                    continue;
                }
                String summary = profileName(profiles, profiles.rule(network), R.string.profiles_rule_default);
                if (network == StreamLayers.Network.MOBILE) {
                    summary += "\n" + getString(R.string.profiles_rule_mobile_summary);
                } else if (network == StreamLayers.Network.WIFI) {
                    summary += "\n" + getString(R.string.profiles_rule_wifi_summary);
                }
                row(choice, "rule:" + network.name(), StreamLayersText.network(activity, network), summary,
                        () -> pickRule(network)).setEnabled(profiles.auto);
            }
            row(choice, "now", getString(R.string.profiles_now), nowSummary(profiles), null);

            home.removeAll();
            boolean access = StreamNetwork.canReadWifiName(activity);
            for (String name : new TreeSet<>(profiles.homeWifi)) {
                row(home, "home:" + name, name, getString(R.string.profiles_home_saved), () -> confirmRemoveHome(name))
                        .setEnabled(profiles.auto);
            }
            String addSummary = getString(R.string.profiles_home_add_current_summary);
            if (!profiles.homeWifi.isEmpty() && !access) {
                addSummary += "\n" + getString(R.string.profiles_home_no_access);
            } else if (profiles.homeWifi.isEmpty()) {
                addSummary += "\n" + getString(R.string.profiles_home_none);
            }
            row(home, "home_add", getString(R.string.profiles_home_add_current), addSummary, this::addCurrentWifiAsked)
                    .setEnabled(profiles.auto);
            row(home, "home_type", getString(R.string.profiles_home_type), getString(R.string.profiles_home_type_summary),
                    this::typeHomeWifi).setEnabled(profiles.auto);
        }

        private String profileSummary(StreamProfiles.Snapshot profiles, String id) {
            HostStreamProfile values = profiles.valuesOrEmpty(id);
            int count = values.count();
            StringBuilder summary = new StringBuilder(count == 0 ? getString(R.string.profiles_values_none) :
                    getResources().getQuantityString(R.plurals.profiles_values_count, count, count));
            if (id.equals(profiles.activeId)) summary.append('\n').append(getString(R.string.profiles_marker_in_use));
            List<String> networks = new ArrayList<>();
            if (profiles.auto) {
                for (StreamLayers.Network network : RULE_NETWORKS) {
                    if (id.equals(profiles.rule(network))) networks.add(StreamLayersText.network(activity, network));
                }
            }
            if (!networks.isEmpty()) {
                summary.append('\n').append(getString(R.string.profiles_marker_rules, String.join(", ", networks)));
            }
            return summary.toString();
        }

        /** What a stream started now would use, from the network now. */
        private String nowSummary(StreamProfiles.Snapshot profiles) {
            StreamLayers.Input input = new StreamLayers.Input();
            input.profiles = profiles;
            StreamNetwork.fill(activity, input);
            StreamLayers.Result result = StreamLayers.resolve(input);
            String used = result.profileName != null ? StreamLayersText.profile(activity, result) :
                    getString(R.string.profiles_main_settings);
            StringBuilder summary = new StringBuilder(getString(R.string.profiles_now_value,
                    StreamLayersText.network(activity, result.network), used));
            for (String problem : StreamLayersText.problems(activity, result)) summary.append('\n').append(problem);
            return summary.toString();
        }

        private void showProfileActions(String id) {
            StreamProfiles.Snapshot profiles = StreamProfiles.load(activity);
            if (!profiles.exists(id)) {
                refresh();
                return;
            }
            String name = profiles.name(id);
            CharSequence[] actions = {getString(R.string.profiles_edit), getString(R.string.profiles_rename),
                    getString(R.string.profiles_duplicate), getString(R.string.profiles_delete)};
            UiHelper.showDialog(activity, new NavigationDialogBuilder(activity).setTitle(name)
                    .setItems(actions, (dialog, which) -> {
                        switch (which) {
                            case 0:
                                HostStreamSettings.showProfile(activity, id);
                                break;
                            case 1:
                                askName(R.string.profiles_rename, name, id, newName -> {
                                    StreamProfiles.rename(activity, id, newName);
                                    refresh();
                                });
                                break;
                            case 2:
                                duplicate(id);
                                break;
                            default:
                                confirmDelete(id, name);
                                break;
                        }
                    }).setNegativeButton(android.R.string.cancel, null).create());
        }

        private void duplicate(String id) {
            StreamProfiles.Snapshot profiles = StreamProfiles.load(activity);
            String copy = StreamProfiles.create(activity,
                    profiles.freeName(getString(R.string.profiles_copy_name, profiles.name(id))),
                    profiles.valuesOrEmpty(id));
            if (copy == null) {
                Toast.makeText(activity, getString(R.string.profiles_too_many, StreamProfiles.MAX_PROFILES),
                        Toast.LENGTH_LONG).show();
            }
            refresh();
        }

        private void confirmDelete(String id, String name) {
            UiHelper.showDialog(activity, new NavigationDialogBuilder(activity)
                    .setTitle(getString(R.string.profiles_delete_title, name))
                    .setMessage(R.string.profiles_delete_message)
                    .setPositiveButton(R.string.profiles_delete, (dialog, which) -> {
                        StreamProfiles.delete(activity, id);
                        Toast.makeText(activity, R.string.profiles_deleted, Toast.LENGTH_SHORT).show();
                        refresh();
                    }).setNegativeButton(android.R.string.cancel, null).create());
        }

        interface NameListener {
            void onName(String name);
        }

        interface PickListener {
            void onPick(String id);
        }

        /** Asks for a profile name and checks it before closing. */
        private void askName(int title, String current, String exceptId, NameListener listener) {
            LinearLayout form = new LinearLayout(activity);
            form.setOrientation(LinearLayout.VERTICAL);
            int padding = Math.round(24 * getResources().getDisplayMetrics().density);
            form.setPadding(padding, padding / 3, padding, 0);
            TextInputLayout container = new TextInputLayout(activity);
            container.setHint(R.string.profiles_name);
            EditText field = new TextInputEditText(container.getContext());
            field.setId(R.id.profiles_name_field);
            field.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_CAP_SENTENCES);
            field.setSingleLine(true);
            field.setFilters(new InputFilter[] {new InputFilter.LengthFilter(StreamProfiles.MAX_NAME + 8)});
            field.setText(current);
            field.setSelectAllOnFocus(true);
            container.addView(field);
            form.addView(container);
            AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle(title).setView(form)
                    .setPositiveButton(android.R.string.ok, null).setNegativeButton(android.R.string.cancel, null).create();
            dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
                String name = field.getText().toString();
                StreamProfiles.NameProblem problem = StreamProfiles.load(activity).checkName(name, exceptId);
                if (problem != null) {
                    container.setError(problem == StreamProfiles.NameProblem.EMPTY ? getString(R.string.profiles_name_empty) :
                            problem == StreamProfiles.NameProblem.TOO_LONG ?
                                    getString(R.string.profiles_name_too_long, StreamProfiles.MAX_NAME) :
                                    getString(R.string.profiles_name_taken));
                    return;
                }
                dialog.dismiss();
                listener.onName(name.trim());
            }));
            UiHelper.showDialog(activity, dialog);
            field.requestFocus();
        }

        /** A choice of None (or the profile in use) and every profile. */
        private void pickProfile(int title, int noneLabel, String current, PickListener onPick) {
            StreamProfiles.Snapshot profiles = StreamProfiles.load(activity);
            List<String> ids = new ArrayList<>(profiles.names.keySet());
            CharSequence[] items = new CharSequence[ids.size() + 1];
            items[0] = getString(noneLabel);
            for (int i = 0; i < ids.size(); i++) items[i + 1] = profiles.name(ids.get(i));
            int checked = current == null || !ids.contains(current) ? 0 : ids.indexOf(current) + 1;
            UiHelper.showDialog(activity, new NavigationDialogBuilder(activity).setTitle(title)
                    .setSingleChoiceItems(items, checked, (dialog, which) -> {
                        onPick.onPick(which == 0 ? null : ids.get(which - 1));
                        dialog.dismiss();
                        refresh();
                    }).setNegativeButton(android.R.string.cancel, null).create());
        }

        private void pickInUse() {
            pickProfile(R.string.profiles_in_use, R.string.profiles_none, StreamProfiles.load(activity).activeId,
                    id -> StreamProfiles.setActive(activity, id));
        }

        private void pickRule(StreamLayers.Network network) {
            int title = network == StreamLayers.Network.MOBILE ? R.string.profiles_network_mobile :
                    network == StreamLayers.Network.HOME_WIFI ? R.string.profiles_network_home_wifi :
                            R.string.profiles_network_wifi;
            pickProfile(title, R.string.profiles_rule_default, StreamProfiles.load(activity).rule(network),
                    id -> StreamProfiles.setRule(activity, network, id));
        }

        private void confirmRemoveHome(String name) {
            UiHelper.showDialog(activity, new NavigationDialogBuilder(activity)
                    .setTitle(getString(R.string.profiles_home_remove_title, name))
                    .setPositiveButton(R.string.profiles_home_remove, (dialog, which) -> {
                        Set<String> names = new TreeSet<>(StreamProfiles.load(activity).homeWifi);
                        names.remove(name);
                        StreamProfiles.setHomeWifi(activity, names);
                        refresh();
                    }).setNegativeButton(android.R.string.cancel, null).create());
        }

        /** Explains why Android wants location access before asking for it; reads the name once granted. */
        private void addCurrentWifiAsked() {
            if (StreamNetwork.canReadWifiName(activity)) {
                addCurrentWifi();
                return;
            }
            UiHelper.showDialog(activity, new NavigationDialogBuilder(activity).setTitle(R.string.profiles_location_title)
                    .setMessage(R.string.profiles_location_message)
                    .setPositiveButton(R.string.profiles_location_continue, (dialog, which) -> {
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                            activity.requestPermissions(new String[] {Manifest.permission.ACCESS_FINE_LOCATION,
                                    Manifest.permission.ACCESS_COARSE_LOCATION}, LOCATION_REQUEST);
                        }
                    }).setNegativeButton(android.R.string.cancel, null).create());
        }

        void addCurrentWifi() {
            StreamNetwork network = StreamNetwork.read(activity, true);
            if (network.wifiName == null) {
                Toast.makeText(activity, R.string.profiles_wifi_unreadable, Toast.LENGTH_LONG).show();
                refresh();
                return;
            }
            addHome(network.wifiName);
        }

        private void addHome(String name) {
            Set<String> names = new TreeSet<>(StreamProfiles.load(activity).homeWifi);
            names.add(name);
            StreamProfiles.setHomeWifi(activity, names);
            refresh();
        }

        private void typeHomeWifi() {
            LinearLayout form = new LinearLayout(activity);
            form.setOrientation(LinearLayout.VERTICAL);
            int padding = Math.round(24 * getResources().getDisplayMetrics().density);
            form.setPadding(padding, padding / 3, padding, 0);
            TextInputLayout container = new TextInputLayout(activity);
            container.setHint(R.string.profiles_home_name);
            EditText field = new TextInputEditText(container.getContext());
            field.setId(R.id.profiles_wifi_field);
            field.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
            field.setSingleLine(true);
            field.setFilters(new InputFilter[] {new InputFilter.LengthFilter(32)});
            container.addView(field);
            form.addView(container);
            AlertDialog dialog = new NavigationDialogBuilder(activity).setTitle(R.string.profiles_home_type)
                    .setView(form).setPositiveButton(android.R.string.ok, null)
                    .setNegativeButton(android.R.string.cancel, null).create();
            dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
                String name = field.getText().toString();
                if (name.isEmpty()) {
                    container.setError(getString(R.string.profiles_name_empty));
                    return;
                }
                dialog.dismiss();
                addHome(name);
            }));
            UiHelper.showDialog(activity, dialog);
            field.requestFocus();
        }

        @Override
        public View onCreateView(LayoutInflater inflater, ViewGroup container, Bundle state) {
            View view = super.onCreateView(inflater, container, state);
            ListView listView = view.findViewById(android.R.id.list);
            com.limelight.ui.UiNavigation.bindPreferences(listView);
            listView.setDivider(null);
            listView.setSelector(R.drawable.list_focus_row);
            listView.setPadding(0, 0, 0, getResources().getDimensionPixelSize(R.dimen.space_s));
            com.limelight.ui.SubPageFocus.apply(getActivity(), view);
            return view;
        }
    }
}
