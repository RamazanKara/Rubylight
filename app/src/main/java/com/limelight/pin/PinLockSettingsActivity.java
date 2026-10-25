package com.limelight.pin;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;
import android.preference.Preference;
import android.preference.PreferenceCategory;
import android.preference.PreferenceFragment;
import android.preference.PreferenceScreen;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import android.widget.ListView;
import android.widget.Toast;

import com.limelight.R;
import com.limelight.computers.ComputerDatabaseManager;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.UiHelper;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * Settings, PIN lock: set, change or remove the PIN and pick what it protects. With a PIN set the page itself asks
 * for it first, and changing or removing asks again.
 */
public class PinLockSettingsActivity extends NightjarActivity {
    private static final int REQUEST_OPEN = 1, REQUEST_CHANGE = 2, REQUEST_SET = 3, REQUEST_REMOVE = 4;
    private static final String STATE_VERIFIED = "verified";
    private static final String STATE_AWAITING = "awaiting";

    private PinStore store;
    /** The PIN was given and this screen has stayed in front since; never kept across leaving it. */
    private boolean verified;
    /** A PIN screen was started for a result that has not come back yet; kept so a recreation does not start another. */
    private boolean awaiting;
    private FrameLayout content;

    public static void open(Activity from) {
        from.startActivity(new Intent(from, PinLockSettingsActivity.class));
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        UiHelper.setLocale(this);
        store = new PinStore(this);
        // Only a rotation or a similar restart of this very screen keeps the verification.
        verified = state != null && state.getBoolean(STATE_VERIFIED);
        awaiting = state != null && state.getBoolean(STATE_AWAITING);
        content = TopAppBar.setUpPage(this, R.string.pin_lock_title, this::finish);
        if (state == null) {
            getFragmentManager().beginTransaction().replace(content.getId(), new LockFragment()).commit();
        }
        if (store.hasPin() && !verified) content.setVisibility(View.INVISIBLE);
        UiHelper.notifyNewRootView(this);
    }

    @Override
    protected void onStart() {
        super.onStart();
        if (store.hasPin() && !verified && !awaiting) {
            content.setVisibility(View.INVISIBLE);
            startActivityForResult(PinLockActivity.forVerify(this), REQUEST_OPEN);
        }
    }

    @Override
    protected void onStop() {
        super.onStop();
        // Home, recents, another app: the next visit asks again. A PIN screen on top of this one is not leaving.
        if (!isChangingConfigurations() && !awaiting) {
            verified = false;
            if (store.hasPin()) content.setVisibility(View.INVISIBLE);
        }
    }

    @Override
    protected void onSaveInstanceState(Bundle state) {
        super.onSaveInstanceState(state);
        state.putBoolean(STATE_VERIFIED, verified && isChangingConfigurations());
        state.putBoolean(STATE_AWAITING, awaiting);
    }

    @Override
    public void startActivityForResult(Intent intent, int request, Bundle options) {
        if (request >= 0) awaiting = true;
        super.startActivityForResult(intent, request, options);
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        awaiting = false;
        boolean ok = result == RESULT_OK;
        switch (request) {
            case REQUEST_OPEN:
                if (ok) {
                    verified = true;
                    content.setVisibility(View.VISIBLE);
                } else {
                    finish();
                }
                break;
            case REQUEST_CHANGE:
                if (ok) {
                    verified = true;
                    startActivityForResult(PinLockActivity.forSet(this), REQUEST_SET);
                }
                break;
            case REQUEST_SET:
                verified = ok || verified;
                if (ok && store.mode() == PinStore.Mode.OFF) {
                    // A PIN that locks nothing would surprise: start with the app itself.
                    store.setMode(PinStore.Mode.APP);
                    PinGate.unlockApp();
                }
                refresh();
                break;
            case REQUEST_REMOVE:
                if (ok) {
                    store.removePin();
                    Toast.makeText(this, R.string.pin_removed, Toast.LENGTH_SHORT).show();
                    verified = false;
                    refresh();
                }
                break;
            default:
                break;
        }
    }

    private void refresh() {
        Object fragment = getFragmentManager().findFragmentById(content.getId());
        if (fragment instanceof LockFragment) ((LockFragment) fragment).refresh();
    }

    void changePin() {
        startActivityForResult(store.hasPin() ? PinLockActivity.forVerify(this) : PinLockActivity.forSet(this),
                store.hasPin() ? REQUEST_CHANGE : REQUEST_SET);
    }

    void removePin() {
        UiHelper.showDialog(this, new NavigationDialogBuilder(this).setTitle(R.string.pin_remove_title)
                .setMessage(R.string.pin_remove_message)
                .setPositiveButton(R.string.pin_remove, (dialog, which) ->
                        startActivityForResult(PinLockActivity.forVerify(this), REQUEST_REMOVE))
                .setNegativeButton(android.R.string.cancel, null).create());
    }

    private static final PinStore.Mode[] MODES = {PinStore.Mode.OFF, PinStore.Mode.APP, PinStore.Mode.STREAMS,
            PinStore.Mode.SELECTED};

    static int modeLabel(PinStore.Mode mode) {
        switch (mode) {
            case APP: return R.string.pin_mode_app;
            case STREAMS: return R.string.pin_mode_streams;
            case SELECTED: return R.string.pin_mode_selected;
            default: return R.string.pin_mode_off;
        }
    }

    public static class LockFragment extends PreferenceFragment {
        private PinLockSettingsActivity activity;
        private PreferenceCategory pin, lock;

        @Override
        public void onActivityCreated(Bundle state) {
            super.onActivityCreated(state);
            activity = (PinLockSettingsActivity) getActivity();
            PreferenceScreen screen = getPreferenceManager().createPreferenceScreen(activity);
            setPreferenceScreen(screen);
            pin = category(screen, R.string.pin_lock_title);
            lock = category(screen, R.string.pin_mode_title);
            Preference footer = new Preference(activity);
            footer.setLayoutResource(R.layout.settings_preference);
            footer.setKey("pin_footer");
            footer.setSummary(R.string.pin_footer);
            footer.setSelectable(false);
            screen.addPreference(footer);
            refresh();
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
            preference.setOnPreferenceClickListener(pref -> {
                onClick.run();
                return true;
            });
            category.addPreference(preference);
            return preference;
        }

        void refresh() {
            if (activity == null) return;
            PinStore store = activity.store;
            boolean has = store.hasPin();
            pin.removeAll();
            row(pin, "pin_change", getString(has ? R.string.pin_change : R.string.pin_set),
                    getString(has ? R.string.pin_is_set_summary : R.string.pin_set_summary), activity::changePin);
            if (has) {
                row(pin, "pin_remove", getString(R.string.pin_remove), "", activity::removePin);
            }

            lock.removeAll();
            PinStore.Mode mode = store.mode();
            row(lock, "pin_mode", getString(R.string.pin_mode_title), getString(modeLabel(mode)), this::pickMode)
                    .setEnabled(has);
            if (mode == PinStore.Mode.SELECTED) {
                row(lock, "pin_pcs", getString(R.string.pin_pcs_title), pcsSummary(store), this::pickPcs);
            }
            if (mode == PinStore.Mode.APP) {
                row(lock, "pin_grace", getString(R.string.pin_grace_title), graceLabel(store.graceMinutes()),
                        this::pickGrace);
            }
        }

        @Override
        public void onResume() {
            super.onResume();
            refresh();
        }

        private String graceLabel(int minutes) {
            if (minutes == 0) return getString(R.string.pin_grace_always);
            if (minutes == 60) return getString(R.string.pin_grace_hour);
            return getString(R.string.pin_grace_minutes, minutes);
        }

        private String pcsSummary(PinStore store) {
            int count = store.pcs().size();
            return count == 0 ? getString(R.string.pin_pcs_none)
                    : getResources().getQuantityString(R.plurals.pin_pcs_selected, count, count);
        }

        private void pickMode() {
            PinStore store = activity.store;
            CharSequence[] labels = new CharSequence[MODES.length];
            int checked = 0;
            for (int i = 0; i < MODES.length; i++) {
                labels[i] = getString(modeLabel(MODES[i]));
                if (MODES[i] == store.mode()) checked = i;
            }
            UiHelper.showDialog(activity, new NavigationDialogBuilder(activity).setTitle(R.string.pin_mode_title)
                    .setSingleChoiceItems(labels, checked, (dialog, which) -> {
                        store.setMode(MODES[which]);
                        // Turning the app lock on while using the app must not lock it on the spot.
                        PinGate.unlockApp();
                        dialog.dismiss();
                        refresh();
                        if (MODES[which] == PinStore.Mode.SELECTED && store.pcs().isEmpty()) pickPcs();
                    }).setNegativeButton(android.R.string.cancel, null).create());
        }

        private void pickGrace() {
            PinStore store = activity.store;
            int[] choices = PinStore.GRACE_CHOICES;
            CharSequence[] labels = new CharSequence[choices.length];
            int checked = 0;
            for (int i = 0; i < choices.length; i++) {
                labels[i] = graceLabel(choices[i]);
                if (choices[i] == store.graceMinutes()) checked = i;
            }
            UiHelper.showDialog(activity, new NavigationDialogBuilder(activity).setTitle(R.string.pin_grace_title)
                    .setSingleChoiceItems(labels, checked, (dialog, which) -> {
                        store.setGraceMinutes(choices[which]);
                        dialog.dismiss();
                        refresh();
                    }).setNegativeButton(android.R.string.cancel, null).create());
        }

        private void pickPcs() {
            PinStore store = activity.store;
            List<String> uuids = new ArrayList<>();
            List<CharSequence> names = new ArrayList<>();
            ComputerDatabaseManager database = new ComputerDatabaseManager(activity);
            try {
                for (ComputerDetails computer : database.getAllComputers()) {
                    uuids.add(computer.uuid);
                    names.add(computer.displayName());
                }
            } finally {
                database.close();
            }
            if (uuids.isEmpty()) {
                Toast.makeText(activity, R.string.pin_pcs_empty, Toast.LENGTH_LONG).show();
                return;
            }
            Set<String> chosen = new HashSet<>();
            for (String stored : store.pcs()) chosen.add(stored.toLowerCase(java.util.Locale.ROOT));
            boolean[] checked = new boolean[uuids.size()];
            for (int i = 0; i < checked.length; i++) {
                checked[i] = chosen.contains(uuids.get(i).toLowerCase(java.util.Locale.ROOT));
            }
            UiHelper.showDialog(activity, new NavigationDialogBuilder(activity).setTitle(R.string.pin_pcs_title)
                    .setMultiChoiceItems(names.toArray(new CharSequence[0]), checked,
                            (dialog, which, isChecked) -> checked[which] = isChecked)
                    .setPositiveButton(android.R.string.ok, (dialog, which) -> {
                        Set<String> picked = new HashSet<>();
                        for (int i = 0; i < checked.length; i++) if (checked[i]) picked.add(uuids.get(i));
                        store.setPcs(picked);
                        refresh();
                    }).setNegativeButton(android.R.string.cancel, null).create());
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
