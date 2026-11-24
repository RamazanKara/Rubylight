package com.limelight.ui;

import android.content.Context;
import android.view.Gravity;
import android.widget.ImageButton;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.core.view.ViewCompat;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.color.MaterialColors;
import com.limelight.R;

/**
 * Header chrome and row/control builders for {@link ActionSheet}, keeping Moonlit Glass styling,
 * TV focus rings, and accessibility properties consistent across bottom sheets and floating panels.
 */
final class ActionSheetRows {
    private ActionSheetRows() { }

    static final class HeaderViews {
        final LinearLayout header;
        final TextView heading;
        final TextView subtitle;
        final ImageButton back;
        final android.widget.ImageView cover;
        final MaterialButton panelClose;

        HeaderViews(LinearLayout header, TextView heading, TextView subtitle, ImageButton back,
                    android.widget.ImageView cover, MaterialButton panelClose) {
            this.header = header;
            this.heading = heading;
            this.subtitle = subtitle;
            this.back = back;
            this.cover = cover;
            this.panelClose = panelClose;
        }
    }

    static HeaderViews buildHeader(Context context, CharSequence title, boolean panel, int spacing, int edge,
                                   Runnable onBack, Runnable onDismiss) {
        LinearLayout header = new LinearLayout(context);
        header.setGravity(Gravity.CENTER_VERTICAL);
        header.setPadding(edge, panel ? edge : spacing, panel ? edge : spacing, panel ? spacing : spacing);
        TextView heading = new TextView(context);
        // A panel's title is the game's name over a compact grid: a size down from a sheet's, on one line.
        heading.setTextAppearance(context, panel ? R.style.TextAppearance_Nightjar_StreamMenuTitle : R.style.TextAppearance_Nightjar_ScreenTitle);
        heading.setText(title);
        heading.setMaxLines(panel ? 1 : 2);
        heading.setEllipsize(android.text.TextUtils.TruncateAt.END);
        heading.setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurface, 0));
        heading.setPadding(0, panel ? 0 : spacing, spacing, panel ? 0 : spacing);
        ViewCompat.setAccessibilityHeading(heading, true);
        TextView subtitle = new TextView(context);
        subtitle.setTextAppearance(context, R.style.TextAppearance_Nightjar_Supporting);
        subtitle.setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurfaceVariant, 0));
        subtitle.setSingleLine(true);
        subtitle.setEllipsize(android.text.TextUtils.TruncateAt.END);
        subtitle.setPadding(0, panel ? spacing / 4 : 0, spacing, panel ? 0 : spacing);
        subtitle.setVisibility(android.view.View.GONE);
        // Shown on a page: touch gets the same way back to the first page as Back, Escape and B.
        ImageButton back = new androidx.appcompat.widget.AppCompatImageButton(context, null, 0);
        back.setId(R.id.action_sheet_back);
        back.setImageResource(R.drawable.ic_arrow_back);
        back.setImageTintList(android.content.res.ColorStateList.valueOf(
                MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurfaceVariant, 0)));
        back.setPadding(spacing * 3 / 2, spacing * 3 / 2, spacing * 3 / 2, spacing * 3 / 2);
        back.setBackgroundResource(R.drawable.bg_icon_button);
        back.setContentDescription(context.getString(R.string.sheet_back));
        back.setOnClickListener(view -> onBack.run());
        back.setVisibility(android.view.View.GONE);
        LinearLayout.LayoutParams backParams = new LinearLayout.LayoutParams(spacing * 6, spacing * 6);
        backParams.setMarginEnd(spacing);
        header.addView(back, backParams);
        android.widget.ImageView cover = null;
        if (panel) {
            android.content.res.Resources res = context.getResources();
            int coverRadius = spacing;
            cover = new android.widget.ImageView(context);
            cover.setId(R.id.stream_menu_cover);
            cover.setScaleType(android.widget.ImageView.ScaleType.CENTER_CROP);
            cover.setImportantForAccessibility(android.view.View.IMPORTANT_FOR_ACCESSIBILITY_NO);
            android.graphics.drawable.GradientDrawable coverShape = new android.graphics.drawable.GradientDrawable();
            coverShape.setCornerRadius(coverRadius);
            coverShape.setColor(MaterialColors.getColor(context, R.attr.glassPlaceholder, 0));
            cover.setBackground(coverShape);
            cover.setClipToOutline(true);
            cover.setVisibility(android.view.View.GONE);
            LinearLayout.LayoutParams coverParams = new LinearLayout.LayoutParams(res.getDimensionPixelSize(R.dimen.stream_menu_cover_width),
                    res.getDimensionPixelSize(R.dimen.stream_menu_cover_height));
            coverParams.setMarginEnd(spacing * 3 / 2);
            header.addView(cover, coverParams);
            int disc = res.getDimensionPixelSize(R.dimen.stream_menu_close);
            back.setBackground(new GlassControlDrawable(context, GlassControlDrawable.Kind.CONTROL, disc));
            backParams.width = backParams.height = disc;
        }
        LinearLayout titles = new LinearLayout(context);
        titles.setOrientation(LinearLayout.VERTICAL);
        titles.addView(heading);
        titles.addView(subtitle);
        header.addView(titles, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        ImageButton close = new androidx.appcompat.widget.AppCompatImageButton(context, null, 0);
        close.setId(R.id.action_sheet_close);
        close.setImageResource(R.drawable.ic_close);
        close.setImageTintList(android.content.res.ColorStateList.valueOf(
                MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurfaceVariant, 0)));
        close.setPadding(spacing * 3 / 2, spacing * 3 / 2, spacing * 3 / 2, spacing * 3 / 2);
        // The same round focus ring as the other icon buttons instead of a grey square.
        close.setBackgroundResource(R.drawable.bg_icon_button);
        close.setContentDescription(context.getString(R.string.sheet_close));
        close.setOnClickListener(view -> onDismiss.run());
        header.addView(close, new LinearLayout.LayoutParams(spacing * 6, spacing * 6));
        // A panel's way back to the stream is its round X, in the header on every page.
        MaterialButton panelClose = null;
        if (panel) {
            close.setVisibility(android.view.View.GONE);
            int disc = context.getResources().getDimensionPixelSize(R.dimen.stream_menu_close);
            panelClose = new MaterialButton(context, null, androidx.appcompat.R.attr.borderlessButtonStyle);
            panelClose.setId(R.id.stream_menu_close);
            panelClose.setIconResource(R.drawable.ic_close);
            panelClose.setIconGravity(MaterialButton.ICON_GRAVITY_TEXT_START);
            panelClose.setIconPadding(0);
            panelClose.setIconSize(spacing * 5 / 2);
            panelClose.setIconTint(android.content.res.ColorStateList.valueOf(
                    MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurface, 0)));
            panelClose.setInsetTop(0);
            panelClose.setInsetBottom(0);
            panelClose.setMinWidth(0);
            panelClose.setMinimumWidth(0);
            panelClose.setMinHeight(0);
            panelClose.setMinimumHeight(0);
            panelClose.setPadding(0, 0, 0, 0);
            panelClose.setBackground(new GlassControlDrawable(context, GlassControlDrawable.Kind.CONTROL, disc));
            panelClose.setContentDescription(context.getString(R.string.sheet_close));
            panelClose.setOnClickListener(view -> onDismiss.run());
            LinearLayout.LayoutParams closeParams = new LinearLayout.LayoutParams(disc, disc);
            closeParams.gravity = Gravity.TOP;
            header.addView(panelClose, closeParams);
        }
        return new HeaderViews(header, heading, subtitle, back, cover, panelClose);
    }

    static void addGrabber(LinearLayout body, Context context, int spacing) {
        android.view.View grabber = new android.view.View(context);
        grabber.setBackgroundResource(R.drawable.glass_grabber);
        LinearLayout.LayoutParams grabberParams = new LinearLayout.LayoutParams(spacing * 9 / 2, spacing / 2);
        grabberParams.gravity = Gravity.CENTER_HORIZONTAL;
        grabberParams.topMargin = spacing;
        body.addView(grabber, 0, grabberParams);
    }

    static LinearLayout[] addColumns(LinearLayout content, Context context, int count) {
        LinearLayout columns = new LinearLayout(context);
        columns.setOrientation(LinearLayout.HORIZONTAL);
        LinearLayout[] parts = new LinearLayout[count];
        for (int i = 0; i < count; i++) {
            parts[i] = new LinearLayout(context);
            parts[i].setOrientation(LinearLayout.VERTICAL);
            columns.addView(parts[i], new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        }
        content.addView(columns, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        return parts;
    }

    static MaterialButton addToggle(Context context, LinearLayout row, int icon, CharSequence label, boolean checked,
                                    int spacing, boolean compact, int minTileHeight, boolean television) {
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
        button.setIconSize(compact ? spacing * 5 / 2 : spacing * 3);
        button.setIconPadding(spacing / 2);
        button.setMinHeight(minTileHeight);
        // The icon sits above the label, so the label can use nearly the whole width of its share.
        // A full step above the icon: half a step, or the button's own few pixels, left it against the tile's top edge.
        if (compact) button.setPaddingRelative(spacing, spacing, spacing, spacing / 2);
        else button.setPaddingRelative(spacing, Math.max(spacing, button.getPaddingTop()), spacing, button.getPaddingBottom());
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
        styleToggle(context, television, button);
        return button;
    }

    static TileGrid addTileGrid(LinearLayout content, Context context, int spacing, boolean television) {
        TileGrid grid = new TileGrid(context);
        // Room around the tiles for a TV's focus ring, which grows a tile; elsewhere the ring sits inside it.
        grid.setPadding(spacing, spacing / 2, spacing, spacing / 2);
        if (television) {
            grid.setClipToPadding(false);
            grid.setClipChildren(false);
        }
        content.addView(grid, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        return grid;
    }

    static MaterialButton addFooterAction(Context context, android.view.ViewGroup footer, int icon, CharSequence label,
                                          boolean destructive, String hotkey, int spacing, boolean capsShown, boolean television) {
        android.content.res.Resources res = context.getResources();
        TileGrid.Tile button = new TileGrid.Tile(context, androidx.appcompat.R.attr.borderlessButtonStyle);
        int ink = destructive ? androidx.core.content.ContextCompat.getColor(context, R.color.nj_color_on_danger_fill)
                : MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurface, 0);
        button.setText(label);
        button.setAllCaps(false);
        button.setTextAppearance(context, R.style.TextAppearance_Nightjar_Label);
        button.setTextColor(ink);
        button.setSingleLine(true);
        button.setEllipsize(android.text.TextUtils.TruncateAt.END);
        button.setIconResource(icon);
        button.setIconTint(android.content.res.ColorStateList.valueOf(ink));
        button.setIconSize(res.getDimensionPixelSize(R.dimen.stream_menu_footer_icon));
        button.setIconPadding(spacing);
        button.setIconGravity(MaterialButton.ICON_GRAVITY_TEXT_START);
        int height = res.getDimensionPixelSize(R.dimen.stream_menu_exit_height);
        button.setMinHeight(height);
        button.setMinimumHeight(height);
        button.setMinWidth(0);
        button.setMinimumWidth(0);
        button.setInsetTop(0);
        button.setInsetBottom(0);
        // The cap sits at the end, so the end padding makes room for it.
        boolean cap = hotkey != null && capsShown;
        button.setPaddingRelative(spacing * 3 / 2, 0, cap ? spacing * 5 : spacing * 3 / 2, 0);
        button.setHotkey(hotkey);
        button.setHotkeyAtEnd(true);
        button.setCapShown(capsShown);
        if (cap) button.setContentDescription(label + ", " + hotkey);
        button.setBackground(new GlassControlDrawable(context,
                destructive ? GlassControlDrawable.Kind.DANGER : GlassControlDrawable.Kind.CONTROL, height));
        if (television) button.setStateListAnimator(Motion.rowFocus(context));
        footer.addView(button);
        return button;
    }

    static LinearLayout addRowGroup(LinearLayout content, Context context, int spacing) {
        android.content.res.Resources res = context.getResources();
        LinearLayout group = new LinearLayout(context);
        styleRowGroup(group);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        params.setMargins(spacing, res.getDimensionPixelSize(R.dimen.stream_menu_group_gap) - spacing / 2, spacing, spacing / 2);
        content.addView(group, params);
        return group;
    }

    /** The flat tint and hairlines of {@link ActionSheet#addRowGroup}, also for a group a layout holds. */
    static void styleRowGroup(LinearLayout group) {
        Context context = group.getContext();
        android.content.res.Resources res = context.getResources();
        float density = res.getDisplayMetrics().density;
        group.setOrientation(LinearLayout.VERTICAL);
        group.setBackground(new GlassControlDrawable(context, GlassControlDrawable.Kind.GROUP, res.getDimension(R.dimen.nj_radius_control)));
        android.graphics.drawable.GradientDrawable line = new android.graphics.drawable.GradientDrawable();
        line.setColor(MaterialColors.getColor(context, R.attr.glassSeparator, 0));
        line.setSize(0, Math.max(1, Math.round(density)));
        group.setDividerDrawable(line);
        group.setShowDividers(LinearLayout.SHOW_DIVIDER_MIDDLE);
        group.setDividerPadding(Math.round(16 * density));
    }

    static TextView addSliderRow(Context context, LinearLayout group, CharSequence label, CharSequence value,
                                 StepSlider slider, int spacing) {
        android.content.res.Resources res = context.getResources();
        LinearLayout row = new LinearLayout(context);
        row.setOrientation(LinearLayout.VERTICAL);
        row.setAddStatesFromChildren(true);
        row.setBackground(new GlassControlDrawable(context, GlassControlDrawable.Kind.ROW, res.getDimension(R.dimen.nj_radius_control)));
        row.setPadding(spacing * 2, spacing * 3 / 2, spacing * 2, spacing / 2);
        LinearLayout line = new LinearLayout(context);
        line.setGravity(Gravity.CENTER_VERTICAL);
        TextView name = new TextView(context);
        name.setText(label);
        name.setTextAppearance(context, R.style.TextAppearance_Nightjar_Label);
        name.setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurface, 0));
        TextView shown = new TextView(context);
        shown.setText(value);
        shown.setTextSize(android.util.TypedValue.COMPLEX_UNIT_PX, res.getDimension(R.dimen.stream_menu_row_value));
        shown.setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurfaceVariant, 0));
        shown.setGravity(Gravity.END);
        line.addView(name, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.WRAP_CONTENT, LinearLayout.LayoutParams.WRAP_CONTENT));
        line.addView(shown, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        row.addView(line);
        int thumb = Math.round(res.getDisplayMetrics().density * 10);
        slider.setThumbRadius(thumb);
        slider.setHaloRadius(0);
        slider.setTrackStopIndicatorSize(0);
        slider.setThumbTrackGapSize(0);
        slider.setTrackHeight(Math.round(res.getDisplayMetrics().density * 6));
        slider.setThumbTintList(android.content.res.ColorStateList.valueOf(
                androidx.core.content.ContextCompat.getColor(context, R.color.nj_color_label)));
        slider.setThumbElevation(0);
        slider.setTrackActiveTintList(android.content.res.ColorStateList.valueOf(MaterialColors.getColor(context, androidx.appcompat.R.attr.colorPrimary, 0)));
        slider.setTrackInactiveTintList(android.content.res.ColorStateList.valueOf(
                androidx.core.content.ContextCompat.getColor(context, R.color.nj_tint_track_fill)));
        slider.setHaloTintList(android.content.res.ColorStateList.valueOf(android.graphics.Color.TRANSPARENT));
        slider.setContentDescription(label);
        // The track starts under the name: the slider keeps a margin for its thumb that the row need not pay twice.
        LinearLayout.LayoutParams sliderParams = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        sliderParams.setMarginStart(-slider.getTrackSidePadding());
        sliderParams.setMarginEnd(-slider.getTrackSidePadding());
        row.addView(slider, sliderParams);
        group.addView(row, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT));
        return shown;
    }

    static MaterialButton wrapPageAction(LinearLayout content, Context context, MaterialButton button,
                                         int spacing, boolean television) {
        content.removeView(button);
        android.widget.FrameLayout frame = new android.widget.FrameLayout(context);
        if (television) frame.setClipChildren(false);
        frame.addView(button, new android.widget.FrameLayout.LayoutParams(
                android.widget.FrameLayout.LayoutParams.MATCH_PARENT, android.widget.FrameLayout.LayoutParams.WRAP_CONTENT));
        android.widget.ImageView chevron = new android.widget.ImageView(context);
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

    static TextView sectionHeading(Context context, CharSequence title, int spacing, boolean compact) {
        TextView heading = new TextView(context);
        heading.setText(title);
        heading.setTextAppearance(context, R.style.TextAppearance_Nightjar_Label);
        heading.setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurfaceVariant, 0));
        heading.setPadding(spacing, compact ? spacing : spacing * 2, spacing, spacing);
        ViewCompat.setAccessibilityHeading(heading, true);
        return heading;
    }

    static LinearLayout addGroup(LinearLayout content, Context context, boolean television) {
        LinearLayout group = new LinearLayout(context);
        group.setOrientation(LinearLayout.VERTICAL);
        if (television) group.setClipChildren(false);
        content.addView(group, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        return group;
    }

    static TextView addNote(Context context, android.view.ViewGroup parent, CharSequence text, boolean error, int spacing) {
        TextView note = new TextView(context);
        note.setText(text);
        note.setTextAppearance(context, R.style.TextAppearance_Nightjar_Supporting);
        note.setTextColor(MaterialColors.getColor(context, error ?
                androidx.appcompat.R.attr.colorError : com.google.android.material.R.attr.colorOnSurfaceVariant, 0));
        note.setPadding(spacing, 0, spacing, spacing);
        parent.addView(note, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        return note;
    }

    static void addInfo(Context context, android.view.ViewGroup parent, CharSequence label, CharSequence value, int spacing) {
        LinearLayout row = new LinearLayout(context);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(spacing, spacing / 2, spacing, spacing / 2);
        TextView name = new TextView(context);
        name.setText(label);
        name.setTextAppearance(context, R.style.TextAppearance_Nightjar_Supporting);
        name.setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurfaceVariant, 0));
        TextView text = new TextView(context);
        text.setText(value);
        text.setTextAppearance(context, R.style.TextAppearance_Nightjar_Body);
        text.setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurface, 0));
        text.setGravity(Gravity.END);
        row.addView(name, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        LinearLayout.LayoutParams valueParams = new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1);
        valueParams.setMarginStart(spacing * 2);
        row.addView(text, valueParams);
        row.setContentDescription(label + ": " + value);
        parent.addView(row, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
    }

    static void styleRow(Context context, boolean television, MaterialButton button) {
        button.setBackground(new GlassControlDrawable(context, GlassControlDrawable.Kind.ROW,
                context.getResources().getDimension(R.dimen.nj_radius_control)));
        if (television) button.setStateListAnimator(Motion.rowFocus(context));
    }

    /** A toggle on a TV: side by side with room between them, so it may grow a little under the ring. */
    static void styleToggle(Context context, boolean television, MaterialButton button) {
        if (television) {
            button.setStrokeColor(androidx.appcompat.content.res.AppCompatResources.getColorStateList(context, R.color.card_stroke));
            button.setStateListAnimator(Motion.controlFocus(context));
        }
    }

    static CharSequence withNote(Context context, CharSequence label, CharSequence note) {
        android.text.SpannableStringBuilder text = new android.text.SpannableStringBuilder(label).append('\n');
        int start = text.length();
        text.append(note);
        text.setSpan(new android.text.style.TextAppearanceSpan(context, R.style.TextAppearance_Nightjar_Supporting),
                start, text.length(), 0);
        text.setSpan(new android.text.style.ForegroundColorSpan(MaterialColors.getColor(context,
                com.google.android.material.R.attr.colorOnSurfaceVariant, 0)), start, text.length(), 0);
        return text;
    }

    static MaterialButton addAction(Context context, android.view.ViewGroup parent, int icon, CharSequence label,
                                    int textColor, int iconColor, int spacing, boolean television) {
        MaterialButton button = new MaterialButton(context, null, androidx.appcompat.R.attr.borderlessButtonStyle);
        button.setText(label);
        button.setTextAppearance(context, R.style.TextAppearance_Nightjar_Body);
        button.setTextColor(MaterialColors.getColor(context, textColor, 0));
        button.setAllCaps(false);
        button.setGravity(Gravity.START | Gravity.CENTER_VERTICAL);
        button.setMinHeight(spacing * 7);
        // Let translated labels and accessibility font sizes grow instead of truncating actions.
        button.setSingleLine(false);
        button.setPaddingRelative(spacing, spacing, spacing, spacing);
        button.setIconResource(icon);
        button.setIconTint(android.content.res.ColorStateList.valueOf(MaterialColors.getColor(context, iconColor, 0)));
        button.setIconPadding(spacing * 2);
        button.setIconSize(spacing * 3);
        parent.addView(button, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        styleRow(context, television, button);
        return button;
    }

    static void addDivider(LinearLayout content, Context context, int spacing) {
        com.google.android.material.divider.MaterialDivider divider =
                new com.google.android.material.divider.MaterialDivider(context);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        params.setMargins(spacing, spacing, spacing, spacing);
        content.addView(divider, params);
    }
}
