package com.limelight.ui;

import android.content.Context;
import android.view.Gravity;
import android.widget.LinearLayout;
import android.widget.ImageButton;
import android.widget.TextView;

import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;
import com.google.android.material.color.MaterialColors;
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
        heading.setTextAppearance(context, R.style.TextAppearance_Nightjar_ScreenTitle);
        heading.setText(title);
        heading.setMaxLines(2);
        heading.setEllipsize(android.text.TextUtils.TruncateAt.END);
        heading.setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurface, 0));
        heading.setPadding(0, spacing, spacing, spacing);
        ViewCompat.setAccessibilityHeading(heading, true);
        header.addView(heading, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        ImageButton close = new androidx.appcompat.widget.AppCompatImageButton(context, null, 0);
        close.setId(R.id.action_sheet_close);
        close.setImageResource(R.drawable.ic_close);
        close.setImageTintList(android.content.res.ColorStateList.valueOf(
                MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurfaceVariant, 0)));
        close.setPadding(spacing * 3 / 2, spacing * 3 / 2, spacing * 3 / 2, spacing * 3 / 2);
        // The same round focus ring as the other icon buttons instead of a grey square.
        close.setBackgroundResource(R.drawable.bg_icon_button);
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
        heading.setTextAppearance(getContext(), R.style.TextAppearance_Nightjar_Label);
        heading.setTextColor(MaterialColors.getColor(getContext(), com.google.android.material.R.attr.colorOnSurfaceVariant, 0));
        heading.setPadding(spacing, spacing * 2, spacing, spacing);
        ViewCompat.setAccessibilityHeading(heading, true);
        content.addView(heading);
    }

    public MaterialButton addAction(int icon, CharSequence label) {
        return addAction(icon, label, com.google.android.material.R.attr.colorOnSurface,
                com.google.android.material.R.attr.colorOnSurfaceVariant);
    }

    /** An action that removes something, in the error colour and kept last in the sheet. */
    public MaterialButton addDestructiveAction(int icon, CharSequence label) {
        return addAction(icon, label, androidx.appcompat.R.attr.colorError, androidx.appcompat.R.attr.colorError);
    }

    private MaterialButton addAction(int icon, CharSequence label, int textColor, int iconColor) {
        MaterialButton button = new MaterialButton(getContext(), null, androidx.appcompat.R.attr.borderlessButtonStyle);
        button.setText(label);
        button.setTextAppearance(getContext(), R.style.TextAppearance_Nightjar_Body);
        button.setTextColor(MaterialColors.getColor(getContext(), textColor, 0));
        button.setAllCaps(false);
        button.setGravity(Gravity.START | Gravity.CENTER_VERTICAL);
        button.setMinHeight(spacing * 7);
        // Let translated labels and accessibility font sizes grow instead of truncating actions.
        button.setSingleLine(false);
        button.setPaddingRelative(spacing, spacing, spacing, spacing);
        button.setIconResource(icon);
        button.setIconTint(android.content.res.ColorStateList.valueOf(MaterialColors.getColor(getContext(), iconColor, 0)));
        button.setIconPadding(spacing * 2);
        button.setIconSize(spacing * 3);
        content.addView(button, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        if (firstAction == null) {
            firstAction = button;
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) button.setFocusedByDefault(true);
        }
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

    /**
     * A dialog window that takes focus shows the system bars again unless it hides them too. Over an
     * immersive stream, hide them on the dialog as well so a menu does not bring the bars back.
     */
    public static void keepSystemBarsHidden(android.app.Dialog dialog, android.app.Activity owner) {
        android.view.Window window = dialog.getWindow();
        WindowInsetsCompat ownerInsets = ViewCompat.getRootWindowInsets(owner.getWindow().getDecorView());
        if (window == null || ownerInsets == null || ownerInsets.isVisible(WindowInsetsCompat.Type.systemBars())) return;
        WindowInsetsControllerCompat controller = WindowCompat.getInsetsController(window, window.getDecorView());
        controller.setSystemBarsBehavior(WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
        controller.hide(WindowInsetsCompat.Type.systemBars());
    }

    @Override
    protected void onStart() {
        super.onStart();
        getBehavior().setState(BottomSheetBehavior.STATE_EXPANDED);
        getBehavior().setSkipCollapsed(true);
        // Material's sheet behavior owns system-bar insets. Consuming them on the dialog's
        // outer content view prevents its bottom padding and hides the last action under navigation.
        if (getOwnerActivity() != null) {
            AdaptiveLayout.attachBottomSheet(getOwnerActivity(), findViewById(android.R.id.content));
            keepSystemBarsHidden(this, getOwnerActivity());
        }
        getBehavior().setMaxWidth(spacing * 75);
        if (!content.isInTouchMode() && firstAction != null) firstAction.requestFocus();
    }

    @Override
    public void onAttachedToWindow() {
        super.onAttachedToWindow();
        // The window hands focus to its first focusable, the close button, once the sheet is laid out.
        // D-pad users should start on the first action instead.
        content.post(() -> {
            if (!content.isInTouchMode() && firstAction != null && isShowing()) firstAction.requestFocus();
        });
    }
}
