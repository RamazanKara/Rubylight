package com.limelight.ui;

import android.content.Context;
import android.view.Gravity;
import android.widget.LinearLayout;
import android.widget.ImageButton;
import android.widget.TextView;

import androidx.activity.OnBackPressedCallback;
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
    private final LinearLayout body;
    private final LinearLayout content;
    private final TextView heading;
    private final ImageButton back;
    private final NestedScrollView scroll;
    private final int spacing;
    private MaterialButton firstAction;
    private Runnable backHandler;
    // Enabled only while a page is shown, so Back returns to the page before instead of closing.
    private final OnBackPressedCallback pageBack = new OnBackPressedCallback(false) {
        @Override public void handleOnBackPressed() {
            if (backHandler != null) backHandler.run();
        }
    };

    public ActionSheet(Context context, CharSequence title) {
        super(context);
        if (context instanceof android.app.Activity) setOwnerActivity((android.app.Activity) context);
        context = getContext();
        spacing = Math.round(8 * context.getResources().getDisplayMetrics().density);
        body = new LinearLayout(context);
        body.setOrientation(LinearLayout.VERTICAL);
        ViewCompat.setAccessibilityPaneTitle(body, title);
        LinearLayout header = new LinearLayout(context);
        header.setGravity(Gravity.CENTER_VERTICAL);
        header.setPadding(spacing * 3, spacing, spacing, spacing);
        content = new LinearLayout(context);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(spacing * 2, 0, spacing * 2, spacing * 2);
        heading = new TextView(context);
        heading.setTextAppearance(context, R.style.TextAppearance_Nightjar_ScreenTitle);
        heading.setText(title);
        heading.setMaxLines(2);
        heading.setEllipsize(android.text.TextUtils.TruncateAt.END);
        heading.setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurface, 0));
        heading.setPadding(0, spacing, spacing, spacing);
        ViewCompat.setAccessibilityHeading(heading, true);
        // Shown on a page: touch gets the same way back to the first page as Back, Escape and B.
        back = new androidx.appcompat.widget.AppCompatImageButton(context, null, 0);
        back.setId(R.id.action_sheet_back);
        back.setImageResource(R.drawable.ic_arrow_back);
        back.setImageTintList(android.content.res.ColorStateList.valueOf(
                MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurfaceVariant, 0)));
        back.setPadding(spacing * 3 / 2, spacing * 3 / 2, spacing * 3 / 2, spacing * 3 / 2);
        back.setBackgroundResource(R.drawable.bg_icon_button);
        back.setContentDescription(context.getString(R.string.sheet_back));
        back.setOnClickListener(view -> {
            if (backHandler != null) backHandler.run();
        });
        back.setVisibility(android.view.View.GONE);
        LinearLayout.LayoutParams backParams = new LinearLayout.LayoutParams(spacing * 6, spacing * 6);
        backParams.setMarginEnd(spacing);
        header.addView(back, backParams);
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
        scroll = new NestedScrollView(context);
        scroll.setClipToPadding(false);
        scroll.addView(content);
        body.addView(scroll, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        setContentView(body);
        setOnKeyListener((dialog, key, event) -> {
            if (!event.hasNoModifiers() || (key != android.view.KeyEvent.KEYCODE_ESCAPE &&
                    key != android.view.KeyEvent.KEYCODE_BUTTON_B)) return false;
            if (event.getAction() == android.view.KeyEvent.ACTION_UP && !event.isCanceled()) {
                if (pageBack.isEnabled()) pageBack.handleOnBackPressed();
                else dismiss();
            }
            return true;
        });
        getOnBackPressedDispatcher().addCallback(pageBack);
    }

    /**
     * While set, Back, Escape and controller B run {@code handler} instead of closing the sheet, so a
     * page shown in the sheet returns to the one before it. Clear it on the first page.
     */
    public void setBackHandler(Runnable handler) {
        backHandler = handler;
        pageBack.setEnabled(handler != null);
        back.setVisibility(handler != null ? android.view.View.VISIBLE : android.view.View.GONE);
    }

    /** The sheet's title, for a page shown in place of the first one. */
    public void setHeading(CharSequence title) {
        heading.setText(title);
        ViewCompat.setAccessibilityPaneTitle(body, title);
    }

    /** Removes every row so another page can be shown in the same sheet, scrolled back to its top. */
    public void clearActions() {
        content.removeAllViews();
        firstAction = null;
        scroll.scrollTo(0, 0);
    }

    /** Moves D-pad and keyboard focus to the first row, as when the sheet opens. */
    public void focusFirstAction() {
        if (!content.isInTouchMode() && firstAction != null) firstAction.requestFocus();
    }

    /** A row of equal toggle buttons above the actions, for settings changed without leaving the sheet. */
    public LinearLayout addToggleRow() {
        LinearLayout row = new ToggleRow(getContext(), spacing);
        row.setPadding(spacing, spacing, spacing, spacing);
        content.addView(row, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        return row;
    }

    /** A toggle in {@code row}, filled while on. A click flips {@code isChecked()} before its listener runs. */
    public MaterialButton addToggle(LinearLayout row, int icon, CharSequence label, boolean checked) {
        Context context = getContext();
        MaterialButton button = new MaterialButton(context, null,
                com.google.android.material.R.attr.materialButtonOutlinedStyle);
        button.setCheckable(true);
        button.setChecked(checked);
        button.setText(label);
        button.setAllCaps(false);
        button.setTextAppearance(context, R.style.TextAppearance_Nightjar_Label);
        button.setSingleLine(false);
        button.setMaxLines(2);
        button.setIconResource(icon);
        button.setIconGravity(MaterialButton.ICON_GRAVITY_TOP);
        button.setIconSize(spacing * 3);
        button.setIconPadding(spacing / 2);
        button.setMinHeight(spacing * 9);
        // The icon sits above the label, so the label can use nearly the whole width of its share.
        button.setPaddingRelative(spacing, button.getPaddingTop(), spacing, button.getPaddingBottom());
        int[][] states = {new int[] {android.R.attr.state_checked}, new int[] {}};
        android.content.res.ColorStateList foreground = new android.content.res.ColorStateList(states, new int[] {
                MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnPrimaryContainer, 0),
                MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurface, 0)});
        button.setTextColor(foreground);
        button.setIconTint(foreground);
        button.setBackgroundTintList(new android.content.res.ColorStateList(states, new int[] {
                MaterialColors.getColor(context, com.google.android.material.R.attr.colorPrimaryContainer, 0),
                android.graphics.Color.TRANSPARENT}));
        row.addView(button, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.MATCH_PARENT, 1));
        claimFirstAction(button);
        return button;
    }

    /**
     * Equal toggles side by side while every word of their labels fits its share of the width. With a
     * large font or a narrow sheet they stack instead, one full-width row each with the icon at the
     * start, so no label breaks mid-word.
     */
    static final class ToggleRow extends LinearLayout {
        private final int gap;

        ToggleRow(Context context, int gap) {
            super(context);
            this.gap = gap;
            // Equal buttons in a straight line, whatever the size of their icons or text.
            setBaselineAligned(false);
        }

        @Override
        protected void onMeasure(int widthSpec, int heightSpec) {
            int count = getChildCount();
            int width = MeasureSpec.getSize(widthSpec) - getPaddingLeft() - getPaddingRight();
            boolean stacked = count > 0 && MeasureSpec.getMode(widthSpec) != MeasureSpec.UNSPECIFIED &&
                    !wordsFit((width - gap * (count - 1)) / count);
            setOrientation(stacked ? VERTICAL : HORIZONTAL);
            for (int i = 0; i < count; i++) {
                MaterialButton button = (MaterialButton) getChildAt(i);
                LayoutParams params = (LayoutParams) button.getLayoutParams();
                params.width = stacked ? LayoutParams.MATCH_PARENT : 0;
                params.height = stacked ? LayoutParams.WRAP_CONTENT : LayoutParams.MATCH_PARENT;
                params.weight = stacked ? 0 : 1;
                params.setMarginStart(stacked || i == 0 ? 0 : gap);
                params.topMargin = stacked && i > 0 ? gap : 0;
                int iconGravity = stacked ? MaterialButton.ICON_GRAVITY_TEXT_START : MaterialButton.ICON_GRAVITY_TOP;
                if (button.getIconGravity() != iconGravity) button.setIconGravity(iconGravity);
                // A stacked toggle has its icon beside the label, so it needs only a list row's height.
                int minHeight = gap * (stacked ? 6 : 9);
                if (button.getMinHeight() != minHeight) button.setMinHeight(minHeight);
            }
            super.onMeasure(widthSpec, heightSpec);
        }

        private boolean wordsFit(int buttonWidth) {
            for (int i = 0; i < getChildCount(); i++) {
                MaterialButton button = (MaterialButton) getChildAt(i);
                int room = buttonWidth - button.getPaddingLeft() - button.getPaddingRight();
                for (String word : button.getText().toString().split("\\s+")) {
                    if (button.getPaint().measureText(word) > room) return false;
                }
            }
            return true;
        }
    }

    /** An action that opens another page of this sheet, marked with a chevron at its end. */
    public MaterialButton addPageAction(int icon, CharSequence label) {
        MaterialButton button = addAction(icon, label);
        content.removeView(button);
        android.widget.FrameLayout frame = new android.widget.FrameLayout(getContext());
        frame.addView(button, new android.widget.FrameLayout.LayoutParams(
                android.widget.FrameLayout.LayoutParams.MATCH_PARENT, android.widget.FrameLayout.LayoutParams.WRAP_CONTENT));
        android.widget.ImageView chevron = new android.widget.ImageView(getContext());
        chevron.setImageResource(R.drawable.settings_chevron);
        chevron.setImportantForAccessibility(android.view.View.IMPORTANT_FOR_ACCESSIBILITY_NO);
        android.widget.FrameLayout.LayoutParams chevronParams = new android.widget.FrameLayout.LayoutParams(
                spacing * 3, spacing * 3, Gravity.END | Gravity.CENTER_VERTICAL);
        chevronParams.setMarginEnd(spacing);
        frame.addView(chevron, chevronParams);
        button.setPaddingRelative(button.getPaddingStart(), button.getPaddingTop(), spacing * 5, button.getPaddingBottom());
        content.addView(frame, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        return button;
    }

    private void claimFirstAction(MaterialButton button) {
        if (firstAction == null) {
            firstAction = button;
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) button.setFocusedByDefault(true);
        }
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

    /** An action with a second, quieter line that says what will happen. */
    public MaterialButton addAction(int icon, CharSequence label, CharSequence note) {
        return addAction(icon, withNote(label, note));
    }

    /** Rewrites an action added with a note, for a row that changes a setting in place. */
    public void setActionText(MaterialButton action, CharSequence label, CharSequence note) {
        action.setText(withNote(label, note));
    }

    private CharSequence withNote(CharSequence label, CharSequence note) {
        android.text.SpannableStringBuilder text = new android.text.SpannableStringBuilder(label).append('\n');
        int start = text.length();
        text.append(note);
        text.setSpan(new android.text.style.TextAppearanceSpan(getContext(), R.style.TextAppearance_Nightjar_Supporting),
                start, text.length(), 0);
        text.setSpan(new android.text.style.ForegroundColorSpan(MaterialColors.getColor(getContext(),
                com.google.android.material.R.attr.colorOnSurfaceVariant, 0)), start, text.length(), 0);
        return text;
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
        claimFirstAction(button);
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
        if (window == null || !hidesSystemBars(ViewCompat.getRootWindowInsets(owner.getWindow().getDecorView()))) return;
        WindowInsetsControllerCompat controller = WindowCompat.getInsetsController(window, window.getDecorView());
        controller.setSystemBarsBehavior(WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
        controller.hide(WindowInsetsCompat.Type.systemBars());
    }

    /** True only when both bars are hidden, as over an immersive stream. */
    static boolean hidesSystemBars(WindowInsetsCompat insets) {
        // systemBars() also covers the caption bar, which phones never show, so test the two bars separately.
        return insets != null && !insets.isVisible(WindowInsetsCompat.Type.statusBars())
                && !insets.isVisible(WindowInsetsCompat.Type.navigationBars());
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
