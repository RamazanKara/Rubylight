package com.limelight.preferences;

import android.content.Context;
import android.preference.SwitchPreference;
import android.util.AttributeSet;
import android.view.View;
import android.widget.ImageButton;

import com.google.android.material.materialswitch.MaterialSwitch;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.R;
import com.limelight.utils.UiHelper;

/**
 * A settings switch: the title, the switch showing its state, and android:summary only when a
 * short line is needed. The longer android:dialogMessage explanation sits behind an info button.
 */
public class MaterialSwitchPreference extends SwitchPreference {
    private static final String ANDROID_SCHEMA_URL = "http://schemas.android.com/apk/res/android";
    // Material's disabled content opacity, as in color/text_on_surface.xml
    private static final float DISABLED_ALPHA = 0.38f;
    private CharSequence explanation;

    public MaterialSwitchPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
        int message = attrs == null ? 0 : attrs.getAttributeResourceValue(ANDROID_SCHEMA_URL, "dialogMessage", 0);
        explanation = message == 0 ? null : context.getText(message);
        setWidgetLayoutResource(R.layout.settings_switch);
    }

    /**
     * android:summaryOn and android:summaryOff count as the summary, not only in the bound row: search results and
     * the row's value line both read getSummary(), which the framework leaves at android:summary.
     */
    @Override
    public CharSequence getSummary() {
        CharSequence state = isChecked() ? getSummaryOn() : getSummaryOff();
        return android.text.TextUtils.isEmpty(state) ? super.getSummary() : state;
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
        super.onBindView(view);
        MaterialSwitch toggle = view.findViewById(R.id.preference_switch);
        toggle.setChecked(isChecked());
        Runnable showExplanation = explanation == null ? null : () -> UiHelper.showDialog(getContext(),
                new NavigationDialogBuilder(getContext()).setTitle(getTitle()).setMessage(explanation)
                        .setPositiveButton(android.R.string.ok, null).create());
        ImageButton info = view.findViewById(R.id.preference_info);
        info.setVisibility(showExplanation == null ? View.GONE : View.VISIBLE);
        info.setContentDescription(getContext().getString(R.string.setting_help, getTitle()));
        info.setOnClickListener(showExplanation == null ? null : v -> showExplanation.run());
        // The explanation also says why a switch is unavailable, so it still opens on a disabled row,
        // where the button dims to Material's disabled emphasis like the rest of the row.
        info.setEnabled(showExplanation != null);
        info.setAlpha(isEnabled() ? 1f : DISABLED_ALPHA);
        view.setTag(R.id.preference_help, showExplanation);
        androidx.core.view.ViewCompat.setAccessibilityDelegate(view, new androidx.core.view.AccessibilityDelegateCompat() {
            @Override
            public void onInitializeAccessibilityNodeInfo(View host,
                    androidx.core.view.accessibility.AccessibilityNodeInfoCompat info) {
                super.onInitializeAccessibilityNodeInfo(host, info);
                info.setClassName(android.widget.Switch.class.getName());
                info.setCheckable(true);
                info.setChecked(isChecked());
                if (showExplanation != null) {
                    info.addAction(new androidx.core.view.accessibility.AccessibilityNodeInfoCompat.AccessibilityActionCompat(
                            R.id.preference_help, getContext().getString(R.string.setting_help, getTitle())));
                }
            }

            @Override public boolean performAccessibilityAction(View host, int action, android.os.Bundle args) {
                if (action == R.id.preference_help && showExplanation != null) {
                    showExplanation.run();
                    return true;
                }
                return super.performAccessibilityAction(host, action, args);
            }
        });
    }
}
