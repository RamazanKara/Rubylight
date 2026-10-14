package com.limelight.ui;

import android.content.Context;
import android.view.Gravity;
import android.widget.LinearLayout;
import android.widget.ImageButton;
import android.widget.TextView;

import androidx.core.view.ViewCompat;
import androidx.core.widget.NestedScrollView;
import com.google.android.material.bottomsheet.BottomSheetBehavior;
import com.google.android.material.bottomsheet.BottomSheetDialog;
import com.google.android.material.button.MaterialButton;
import com.limelight.R;

public final class ActionSheet extends BottomSheetDialog {
    private final LinearLayout content;
    private final int spacing;
    private MaterialButton firstAction;

    public ActionSheet(Context context, CharSequence title) {
        super(context);
        if (context instanceof android.app.Activity) setOwnerActivity((android.app.Activity) context);
        context = getContext();
        spacing = Math.round(8 * context.getResources().getDisplayMetrics().density);
        LinearLayout body = new LinearLayout(context);
        body.setOrientation(LinearLayout.VERTICAL);
        ViewCompat.setAccessibilityPaneTitle(body, title);
        LinearLayout header = new LinearLayout(context);
        header.setGravity(Gravity.CENTER_VERTICAL);
        header.setPadding(spacing * 3, spacing, spacing, spacing);
        content = new LinearLayout(context);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(spacing * 2, 0, spacing * 2, spacing * 2);
        TextView heading = new TextView(context);
        heading.setTextAppearance(context, com.google.android.material.R.style.TextAppearance_Material3_TitleLarge);
        heading.setText(title);
        heading.setMaxLines(2);
        heading.setEllipsize(android.text.TextUtils.TruncateAt.END);
        heading.setTextColor(androidx.core.content.ContextCompat.getColor(context, R.color.on_surface));
        heading.setPadding(0, spacing, spacing, spacing);
        ViewCompat.setAccessibilityHeading(heading, true);
        header.addView(heading, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        ImageButton close = new androidx.appcompat.widget.AppCompatImageButton(context, null, androidx.appcompat.R.attr.borderlessButtonStyle);
        close.setId(R.id.action_sheet_close);
        close.setImageResource(R.drawable.ic_close);
        close.setImageTintList(android.content.res.ColorStateList.valueOf(
                androidx.core.content.ContextCompat.getColor(context, R.color.on_surface)));
        close.setPadding(spacing * 3 / 2, spacing * 3 / 2, spacing * 3 / 2, spacing * 3 / 2);
        close.setContentDescription(context.getString(R.string.sheet_close));
        close.setOnClickListener(view -> dismiss());
        header.addView(close, new LinearLayout.LayoutParams(spacing * 6, spacing * 6));
        body.addView(header);
        NestedScrollView scroll = new NestedScrollView(context);
        scroll.setClipToPadding(false);
        scroll.addView(content);
        body.addView(scroll, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        setContentView(body);
        setOnKeyListener((dialog, key, event) -> {
            if (!event.hasNoModifiers() || (key != android.view.KeyEvent.KEYCODE_ESCAPE &&
                    key != android.view.KeyEvent.KEYCODE_BUTTON_B)) return false;
            if (event.getAction() == android.view.KeyEvent.ACTION_UP && !event.isCanceled()) dismiss();
            return true;
        });
    }

    public void addSection(int title) {
        TextView heading = new TextView(getContext());
        heading.setText(title);
        heading.setTextAppearance(getContext(), com.google.android.material.R.style.TextAppearance_Material3_LabelLarge);
        heading.setTextColor(androidx.core.content.ContextCompat.getColor(getContext(), R.color.on_surface_variant));
        heading.setPadding(spacing, spacing * 2, spacing, spacing);
        ViewCompat.setAccessibilityHeading(heading, true);
        content.addView(heading);
    }

    public MaterialButton addAction(int icon, CharSequence label) {
        MaterialButton button = new MaterialButton(getContext(), null, androidx.appcompat.R.attr.borderlessButtonStyle);
        button.setText(label);
        button.setTextColor(androidx.core.content.ContextCompat.getColor(getContext(), R.color.on_surface));
        button.setTextSize(16);
        button.setAllCaps(false);
        button.setGravity(Gravity.START | Gravity.CENTER_VERTICAL);
        button.setMinHeight(spacing * 7);
        // Let translated labels and accessibility font sizes grow instead of truncating actions.
        button.setSingleLine(false);
        button.setPaddingRelative(spacing, spacing, spacing, spacing);
        button.setIconResource(icon);
        button.setIconTintResource(R.color.on_surface_variant);
        button.setIconPadding(spacing * 2);
        button.setIconSize(spacing * 3);
        content.addView(button, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        if (firstAction == null) firstAction = button;
        return button;
    }

    public void addDivider() {
        com.google.android.material.divider.MaterialDivider divider =
                new com.google.android.material.divider.MaterialDivider(getContext());
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        params.setMargins(spacing, spacing, spacing, spacing);
        content.addView(divider, params);
    }

    @Override
    protected void onStart() {
        super.onStart();
        getBehavior().setState(BottomSheetBehavior.STATE_EXPANDED);
        getBehavior().setSkipCollapsed(true);
        if (getOwnerActivity() != null) AdaptiveLayout.attach(getOwnerActivity(), findViewById(android.R.id.content));
        getBehavior().setMaxWidth(spacing * 75);
        if (!content.isInTouchMode() && firstAction != null) firstAction.requestFocus();
    }
}
