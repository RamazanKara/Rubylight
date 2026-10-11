package com.limelight.preferences;

import android.content.Context;
import android.preference.SwitchPreference;
import android.util.AttributeSet;
import android.view.View;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.google.android.material.materialswitch.MaterialSwitch;
import com.limelight.R;
import com.limelight.utils.UiHelper;

public class MaterialSwitchPreference extends SwitchPreference {
    private CharSequence explanation;

    public MaterialSwitchPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
        explanation = getSummary();
        setSummaryOn(R.string.stream_enabled);
        setSummaryOff(R.string.stream_disabled);
        setWidgetLayoutResource(R.layout.settings_switch);
    }

    public CharSequence getExplanation() {
        return explanation;
    }

    public void setExplanation(CharSequence explanation) {
        this.explanation = explanation;
        notifyChanged();
    }

    @Override
    protected void onBindView(View view) {
        setSummaryOn(getContext().getString(R.string.stream_enabled) +
                (explanation == null ? "" : "\n" + explanation));
        setSummaryOff(getContext().getString(R.string.stream_disabled) +
                (explanation == null ? "" : "\n" + explanation));
        super.onBindView(view);
        MaterialSwitch toggle = view.findViewById(R.id.preference_switch);
        toggle.setChecked(isChecked());
        androidx.core.view.ViewCompat.setAccessibilityDelegate(view, new androidx.core.view.AccessibilityDelegateCompat() {
            @Override
            public void onInitializeAccessibilityNodeInfo(View host,
                    androidx.core.view.accessibility.AccessibilityNodeInfoCompat info) {
                super.onInitializeAccessibilityNodeInfo(host, info);
                info.setClassName(android.widget.Switch.class.getName());
                info.setCheckable(true);
                info.setChecked(isChecked());
            }
        });
        View help = view.findViewById(R.id.preference_help);
        help.setVisibility(explanation == null ? View.GONE : View.VISIBLE);
        help.setContentDescription(getContext().getString(R.string.setting_help, getTitle()));
        help.setOnClickListener(v -> UiHelper.showDialog(getContext(), new MaterialAlertDialogBuilder(getContext())
                .setTitle(getTitle()).setMessage(explanation).setPositiveButton(android.R.string.ok, null).create()));
    }
}
