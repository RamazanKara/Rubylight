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
    private final TextView subtitle;
    private final ImageButton back;
    private final NestedScrollView scroll;
    private final int spacing;
    // Where a panel's title, tiles and footer start, from its edge.
    private final int edge;
    private final boolean television;
    // A floating panel over the stream instead of a sheet across the bottom; its footer stays out of the scroll.
    private final boolean panel;
    private final FooterBar footer;
    // A panel's cover beside its title, its surface, and the blur under it while it is open.
    private android.widget.ImageView cover;
    private GlassPanelDrawable menuGlass;
    private MenuBlurWindow blur;
    private MaterialButton panelClose;
    // The control a panel hangs under, as Home's PC sheet hangs under its capsule; null for a panel docked by the window.
    private android.view.View anchor;
    private PanelFrame frame;
    private MaterialButton firstAction;
    private Runnable backHandler;
    private Runnable onClosed;
    private java.util.function.IntPredicate keyHandler;
    private final java.util.Set<Integer> handledKeys = new java.util.HashSet<>();
    // Enabled only while a page is shown, so Back returns to the page before instead of closing.
    private final OnBackPressedCallback pageBack = new OnBackPressedCallback(false) {
        @Override public void handleOnBackPressed() {
            if (backHandler != null) backHandler.run();
        }
    };

    private final DpadEchoFilter dpadEchoes = new DpadEchoFilter();

    @Override
    public boolean dispatchKeyEvent(android.view.KeyEvent event) {
        return dpadEchoes.drop(event) || super.dispatchKeyEvent(event);
    }

    private boolean focusPlaced;

    /** Releases that arrive while another window has focus never reach this one. */
    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        if (!hasFocus) dpadEchoes.reset();
        super.onWindowFocusChanged(hasFocus);
        // A remote's first press must move the ring, not reveal it. A new window starts in touch mode even when a key opened
        // it, so ask the window behind: that is where the key landed.
        if (hasFocus && !focusPlaced) {
            focusPlaced = true;
            android.app.Activity owner = getOwnerActivity();
            boolean byKeys = owner != null && !owner.getWindow().getDecorView().isInTouchMode();
            if (byKeys && content.findFocus() == null && firstAction != null) firstAction.requestFocusFromTouch();
        }
    }

    public ActionSheet(Context context, CharSequence title) {
        this(context, title, false);
    }

    /**
     * A panel hanging under {@code anchor}, its end on the anchor's end, over a dimmed page: a TV's sheet opened from the top
     * row, as the PC sheet hangs under Home's capsule. A remote has no bottom edge to reach for.
     */
    public static ActionSheet under(Context context, CharSequence title, android.view.View anchor) {
        ActionSheet sheet = new ActionSheet(context, title, true);
        sheet.anchor = anchor;
        sheet.frame.setAnchor(anchor);
        return sheet;
    }

    /**
     * With {@code panel}, a bounded panel floating over the stream instead of a sheet across the bottom: docked to the
     * end edge in landscape and centred in portrait, at most the desktop menu's width, the stream visible around it and
     * a tap beside it closing it. Its header shows {@link #setSubtitle}, a cover and a round close, and
     * {@link #addFooterAction} sits in a footer outside the part that scrolls, so the ways out are always in view.
     */
    public ActionSheet(Context context, CharSequence title, boolean panel) {
        super(context);
        this.panel = panel;
        if (context instanceof android.app.Activity) setOwnerActivity((android.app.Activity) context);
        context = getContext();
        television = (context.getResources().getConfiguration().uiMode &
                android.content.res.Configuration.UI_MODE_TYPE_MASK) == android.content.res.Configuration.UI_MODE_TYPE_TELEVISION;
        // The sheet's slide from the bottom edge does not fit a panel docked at the side: it fades in as on a TV.
        if ((television || panel) && !Motion.reduced(context)) getWindow().setWindowAnimations(R.style.TvSheetAnimation);
        spacing = Math.round(8 * context.getResources().getDisplayMetrics().density);
        // A panel lines its title, tiles and footer up on one edge, as desktop Nightjar's menu does.
        edge = panel ? context.getResources().getDimensionPixelSize(R.dimen.stream_menu_edge) : spacing * 3;
        body = new LinearLayout(context);
        body.setOrientation(LinearLayout.VERTICAL);
        ViewCompat.setAccessibilityPaneTitle(body, title);
        LinearLayout header = new LinearLayout(context);
        header.setGravity(Gravity.CENTER_VERTICAL);
        header.setPadding(edge, panel ? edge : spacing, panel ? edge : spacing, panel ? spacing : spacing);
        content = new LinearLayout(context);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(edge - spacing, 0, edge - spacing, panel ? spacing / 2 : spacing * 2);
        if (television) content.setClipToPadding(false);
        heading = new TextView(context);
        // A panel's title is the game's name over a compact grid: a size down from a sheet's, on one line.
        heading.setTextAppearance(context, panel ? R.style.TextAppearance_Nightjar_StreamMenuTitle : R.style.TextAppearance_Nightjar_ScreenTitle);
        heading.setText(title);
        heading.setMaxLines(panel ? 1 : 2);
        heading.setEllipsize(android.text.TextUtils.TruncateAt.END);
        heading.setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurface, 0));
        heading.setPadding(0, panel ? 0 : spacing, spacing, panel ? 0 : spacing);
        ViewCompat.setAccessibilityHeading(heading, true);
        subtitle = new TextView(context);
        subtitle.setTextAppearance(context, R.style.TextAppearance_Nightjar_Supporting);
        subtitle.setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurfaceVariant, 0));
        subtitle.setSingleLine(true);
        subtitle.setEllipsize(android.text.TextUtils.TruncateAt.END);
        subtitle.setPadding(0, panel ? spacing / 4 : 0, spacing, panel ? 0 : spacing);
        subtitle.setVisibility(android.view.View.GONE);
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
        close.setOnClickListener(view -> dismiss());
        header.addView(close, new LinearLayout.LayoutParams(spacing * 6, spacing * 6));
        // A panel's way back to the stream is its round X, in the header on every page.
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
            panelClose.setOnClickListener(view -> dismiss());
            LinearLayout.LayoutParams closeParams = new LinearLayout.LayoutParams(disc, disc);
            closeParams.gravity = Gravity.TOP;
            header.addView(panelClose, closeParams);
        }
        body.addView(header);
        scroll = new NestedScrollView(context);
        scroll.setClipToPadding(false);
        // A panel's rows go on under its footer: they fade there instead of ending on a cut edge.
        if (panel) {
            scroll.setVerticalFadingEdgeEnabled(true);
            scroll.setFadingEdgeLength(spacing * 3);
        }
        scroll.addView(content);
        body.addView(scroll, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        if (panel) {
            footer = new FooterBar(context, context.getResources().getDimensionPixelSize(R.dimen.stream_tile_gap));
            footer.setPadding(edge, spacing * 3 / 2, edge, edge);
            if (television) footer.setClipChildren(false);
            body.addView(footer, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT));
            // Near-opaque until show() knows a blur stands under it.
            menuGlass = GlassPanelDrawable.menu(context, false);
            body.setBackground(menuGlass);
            body.setClipToOutline(true);
            body.setElevation(spacing);
            frame = new PanelFrame(context, body, context.getResources().getDimensionPixelSize(R.dimen.stream_menu_margin));
            frame.setOnOutsideTap(this::cancel);
            setContentView(frame);
        } else {
            footer = null;
            // A sheet along the bottom says so; a TV's fades in and has no edge to drag it from.
            if (!television) {
                android.view.View grabber = new android.view.View(context);
                grabber.setBackgroundResource(R.drawable.glass_grabber);
                LinearLayout.LayoutParams grabberParams = new LinearLayout.LayoutParams(spacing * 9 / 2, spacing / 2);
                grabberParams.gravity = Gravity.CENTER_HORIZONTAL;
                grabberParams.topMargin = spacing;
                body.addView(grabber, 0, grabberParams);
            }
            setContentView(body);
        }
        setOnKeyListener((dialog, key, event) -> {
            if (!event.hasNoModifiers()) return false;
            if (key != android.view.KeyEvent.KEYCODE_ESCAPE && key != android.view.KeyEvent.KEYCODE_BUTTON_B) {
                if (keyHandler == null) return false;
                // The release of a key the handler took goes with it, so nothing below sees half a press.
                if (event.getAction() == android.view.KeyEvent.ACTION_DOWN) {
                    if (event.getRepeatCount() > 0) return handledKeys.contains(key);
                    if (keyHandler.test(key)) {
                        handledKeys.add(key);
                        return true;
                    }
                    return false;
                }
                return event.getAction() == android.view.KeyEvent.ACTION_UP && handledKeys.remove(key);
            }
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

    /** The game's cover beside a panel's title; null for none. */
    public void setCover(android.graphics.Bitmap art) {
        if (cover == null) return;
        cover.setImageBitmap(art);
        cover.setVisibility(art == null ? android.view.View.GONE : android.view.View.VISIBLE);
    }

    /** The view showing the cover, on a panel. */
    android.widget.ImageView cover() {
        return cover;
    }

    /** A panel's round X, the way back to the stream; null on a sheet. */
    public MaterialButton panelClose() {
        return panelClose;
    }

    /** The system blur under the panel while it is open; null otherwise. */
    MenuBlurWindow blurWindow() {
        return blur;
    }

    /** The panel's surface; null on a sheet. */
    GlassPanelDrawable menuGlass() {
        return menuGlass;
    }

    @Override
    public void show() {
        // Shown first, so it stands under the panel: it blurs the stream, not the menu.
        // A panel over a page samples nothing live: its surface stays the near-opaque one, over the page's dim.
        if (panel && anchor == null && blur == null && !isShowing() && getOwnerActivity() != null) {
            int radius = Math.round(GlassSpec.menuWindow(getContext()).blur);
            blur = MenuBlurWindow.openIfAllowed(getOwnerActivity(), radius, on -> menuGlass.setOverBlur(on));
            menuGlass.setOverBlur(blur != null);
            if (blur != null) {
                body.addOnLayoutChangeListener((view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> placeBlur());
            }
        }
        super.show();
    }

    private void placeBlur() {
        if (blur == null) return;
        int[] at = new int[2];
        body.getLocationOnScreen(at);
        blur.cover(at[0], at[1], body.getWidth(), body.getHeight());
    }

    /** The sheet's title, for a page shown in place of the first one. */
    public void setHeading(CharSequence title) {
        heading.setText(title);
        ViewCompat.setAccessibilityPaneTitle(body, title);
    }

    /** A quieter line under the title, such as the stream's mode; null or empty for none. */
    public void setSubtitle(CharSequence text) {
        boolean shown = text != null && text.length() > 0;
        subtitle.setText(text);
        subtitle.setVisibility(shown ? android.view.View.VISIBLE : android.view.View.GONE);
        if (!panel) heading.setPadding(0, spacing, spacing, shown ? 0 : spacing);
    }

    CharSequence heading() {
        return heading.getText();
    }

    /** The line under the title; gone while there is none. */
    public TextView subtitle() {
        return subtitle;
    }

    /** True for a floating panel ({@link #ActionSheet(Context, CharSequence, boolean)}). */
    public boolean isPanel() {
        return panel;
    }

    /** Removes every row so another page can be shown in the same sheet, scrolled back to its top. */
    public void clearActions() {
        content.removeAllViews();
        if (footer != null) footer.removeAllViews();
        firstAction = null;
        scroll.scrollTo(0, 0);
        if (television && isShowing()) {
            content.animate().cancel();
            content.setAlpha(0);
            content.setTranslationY(spacing);
            content.animate().alpha(1).translationY(0).setDuration(180)
                    .setInterpolator(new android.view.animation.DecelerateInterpolator()).start();
        }
    }

    /** Moves D-pad and keyboard focus to the first row, as when the sheet opens. */
    /**
     * Opened by a controller after a touch: the window leaves touch mode and the first action shows focus at once, as
     * after a D-pad press, instead of the first press only revealing it.
     */
    public void focusFirstActionFromController() {
        content.post(() -> {
            if (firstAction != null && isShowing()) firstAction.requestFocusFromTouch();
        });
    }

    public void focusFirstAction() {
        if (!content.isInTouchMode() && firstAction != null) firstAction.requestFocus();
    }

    /** Makes another row the one D-pad focus starts on, such as one that doesn't end the stream. */
    public void setFirstAction(MaterialButton button) {
        if (button == null || button == firstAction) return;
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
            if (firstAction != null) firstAction.setFocusedByDefault(false);
            button.setFocusedByDefault(true);
        }
        firstAction = button;
    }

    /** A row of equal toggle buttons above the actions, for settings changed without leaving the sheet. */
    public LinearLayout addToggleRow() {
        LinearLayout row = new ToggleRow(getContext(), spacing);
        row.setPadding(spacing, spacing, spacing, spacing);
        // A focused toggle grows past the row's padding on TV; clipping there cut off its rounded ends.
        if (television) {
            row.setClipToPadding(false);
            row.setClipChildren(false);
        }
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
        styleToggle(button);
        claimFirstAction(button);
        return button;
    }

    /**
     * Equal toggles side by side while every word of their labels fits its share of the width. When they
     * do not, as four toggles on a phone held upright, they form two columns of tiles; with a large font
     * or a very narrow sheet they stack, one full-width row each with the icon at the start, so no label
     * breaks mid-word.
     */
    static final class ToggleRow extends LinearLayout {
        private final int gap;
        private int columns;

        ToggleRow(Context context, int gap) {
            super(context);
            this.gap = gap;
            // Equal buttons in a straight line, whatever the size of their icons or text.
            setBaselineAligned(false);
        }

        /** How many toggles share a line: all of them, two, or one. */
        int columns() {
            return columns;
        }

        @Override
        protected void onMeasure(int widthSpec, int heightSpec) {
            int count = getChildCount();
            int width = MeasureSpec.getSize(widthSpec) - getPaddingLeft() - getPaddingRight();
            boolean bounded = MeasureSpec.getMode(widthSpec) != MeasureSpec.UNSPECIFIED;
            if (count == 0 || !bounded || wordsFit((width - gap * (count - 1)) / count)) columns = Math.max(count, 1);
            else if (count > 2 && wordsFit((width - gap) / 2)) columns = 2;
            else columns = 1;
            boolean stacked = columns == 1 && count > 1;
            setOrientation(stacked ? VERTICAL : HORIZONTAL);
            for (int i = 0; i < count; i++) {
                MaterialButton button = (MaterialButton) getChildAt(i);
                LayoutParams params = (LayoutParams) button.getLayoutParams();
                params.width = stacked ? LayoutParams.MATCH_PARENT : 0;
                params.height = stacked ? LayoutParams.WRAP_CONTENT : LayoutParams.MATCH_PARENT;
                params.weight = stacked ? 0 : 1;
                params.setMarginStart(stacked || i % columns == 0 ? 0 : gap);
                params.resolveLayoutDirection(getLayoutDirection());
                params.topMargin = i >= columns ? gap : 0;
                int iconGravity = stacked ? MaterialButton.ICON_GRAVITY_TEXT_START : MaterialButton.ICON_GRAVITY_TOP;
                if (button.getIconGravity() != iconGravity) button.setIconGravity(iconGravity);
                // A stacked toggle has its icon beside the label, so it needs only a list row's height.
                int minHeight = gap * (stacked ? 6 : 9);
                if (button.getMinHeight() != minHeight) button.setMinHeight(minHeight);
            }
            if (columns == count || stacked) {
                super.onMeasure(widthSpec, heightSpec);
                return;
            }
            // Tiles: equal columns, and each line as tall as its tallest tile so the grid stays even.
            int cell = (width - gap * (columns - 1)) / columns;
            int height = getPaddingTop() + getPaddingBottom();
            for (int line = 0; line * columns < count; line++) {
                int tallest = 0;
                for (int i = line * columns; i < Math.min(count, (line + 1) * columns); i++) {
                    android.view.View child = getChildAt(i);
                    child.measure(MeasureSpec.makeMeasureSpec(cell, MeasureSpec.EXACTLY),
                            MeasureSpec.makeMeasureSpec(0, MeasureSpec.UNSPECIFIED));
                    tallest = Math.max(tallest, child.getMeasuredHeight());
                }
                for (int i = line * columns; i < Math.min(count, (line + 1) * columns); i++) {
                    getChildAt(i).measure(MeasureSpec.makeMeasureSpec(cell, MeasureSpec.EXACTLY),
                            MeasureSpec.makeMeasureSpec(tallest, MeasureSpec.EXACTLY));
                }
                height += tallest + (line > 0 ? gap : 0);
            }
            setMeasuredDimension(MeasureSpec.getSize(widthSpec), resolveSize(height, heightSpec));
        }

        @Override
        protected void onLayout(boolean changed, int left, int top, int right, int bottom) {
            int count = getChildCount();
            if (columns == count || columns == 1) {
                super.onLayout(changed, left, top, right, bottom);
                return;
            }
            boolean rtl = getLayoutDirection() == LAYOUT_DIRECTION_RTL;
            int y = getPaddingTop();
            for (int line = 0; line * columns < count; line++) {
                int tallest = 0;
                for (int i = line * columns; i < Math.min(count, (line + 1) * columns); i++) {
                    android.view.View child = getChildAt(i);
                    int column = i % columns;
                    int x = column * (child.getMeasuredWidth() + gap);
                    int childLeft = rtl ? right - left - getPaddingRight() - x - child.getMeasuredWidth()
                            : getPaddingLeft() + x;
                    child.layout(childLeft, y, childLeft + child.getMeasuredWidth(), y + child.getMeasuredHeight());
                    tallest = Math.max(tallest, child.getMeasuredHeight());
                }
                y += tallest + gap;
            }
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

    /** A grid of compact tiles, as many to a row as fit, for {@link TileGrid#addTile}. */
    public TileGrid addTileGrid() {
        TileGrid grid = new TileGrid(getContext());
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

    /**
     * A way out. In a panel it is a capsule in the footer; {@code destructive} there is the one red control. In a sheet it is a
     * row like {@link #addAction} or {@link #addDestructiveAction}.
     */
    public MaterialButton addFooterAction(int icon, CharSequence label, boolean destructive) {
        if (footer == null) return destructive ? addDestructiveAction(icon, label) : addAction(icon, label);
        Context context = getContext();
        android.content.res.Resources res = context.getResources();
        MaterialButton button = new MaterialButton(context, null, androidx.appcompat.R.attr.borderlessButtonStyle);
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
        button.setPaddingRelative(spacing * 3 / 2, 0, spacing * 3 / 2, 0);
        button.setBackground(new GlassControlDrawable(context,
                destructive ? GlassControlDrawable.Kind.DANGER : GlassControlDrawable.Kind.CONTROL, height));
        if (television) button.setStateListAnimator(Motion.rowFocus(context));
        footer.addView(button);
        return button;
    }

    /** Rows standing together on one flat tint, hairlines between them. */
    public LinearLayout addRowGroup() {
        Context context = getContext();
        android.content.res.Resources res = context.getResources();
        LinearLayout group = new LinearLayout(context);
        styleRowGroup(group);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        params.setMargins(spacing, res.getDimensionPixelSize(R.dimen.stream_menu_group_gap) - spacing / 2, spacing, spacing / 2);
        content.addView(group, params);
        return group;
    }

    /** The flat tint and hairlines of {@link #addRowGroup}, also for a group a layout holds. */
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

    /** A status row in {@link #addRowGroup}: its name, a value at its end, a chevron. */
    public MenuRow addStatusRow(LinearLayout group, CharSequence label, CharSequence value) {
        MenuRow row = new MenuRow(getContext(), label, value);
        group.addView(row, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT));
        return row;
    }

    /**
     * A slider in {@link #addRowGroup}, its name and value over it. The whole row takes the ring while the slider has the focus;
     * D-pad left and right move it. Answers the line showing the value.
     */
    public TextView addSliderRow(LinearLayout group, CharSequence label, CharSequence value, StepSlider slider) {
        Context context = getContext();
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

    /**
     * A panel's window: the panel docked to the end edge in landscape and centred in portrait, the stream around it,
     * and a tap beside the panel closing it. The panel is the design's 452 dp where there is room; in landscape it takes at
     * most two thirds of the width but never less than 360 dp.
     */
    static final class PanelFrame extends android.widget.FrameLayout {
        static final int WIDTH_DP = 452, MIN_WIDTH_DP = 360;
        static final float LANDSCAPE_SHARE = 0.66f;
        private final android.view.View panel;
        private final int margin;
        private Runnable onOutsideTap;
        private boolean pressedOutside;

        PanelFrame(Context context, android.view.View panel, int margin) {
            super(context);
            this.panel = panel;
            this.margin = margin;
            addView(panel, new LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT));
            // Over an immersive stream only a display cutout is left to keep clear of; elsewhere the bars too.
            ViewCompat.setOnApplyWindowInsetsListener(this, (view, insets) -> {
                androidx.core.graphics.Insets bars = insets.getInsets(
                        WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout());
                view.setPadding(bars.left, bars.top, bars.right, bars.bottom);
                return insets;
            });
        }

        void setOnOutsideTap(Runnable onOutsideTap) {
            this.onOutsideTap = onOutsideTap;
        }

        private android.view.View anchor;

        void setAnchor(android.view.View anchor) {
            this.anchor = anchor;
        }

        /**
         * Under an anchor: {@link #MIN_WIDTH_DP} wide, 8 dp below it, its end on the anchor's end (start in RTL), kept
         * {@code edge} inside the window. Positions are on screen; the result is the panel's left and top in this frame.
         */
        static int[] underAnchor(int[] anchorAt, int anchorWidth, int anchorHeight, int[] frameAt, int frameWidth,
                                 int panelWidth, int edge, float density, boolean rtl) {
            int start = rtl ? anchorAt[0] - frameAt[0] : anchorAt[0] - frameAt[0] + anchorWidth - panelWidth;
            int left = Math.max(edge, Math.min(start, frameWidth - panelWidth - edge));
            return new int[] {left, anchorAt[1] - frameAt[1] + anchorHeight + Math.round(8 * density)};
        }

        /** The panel's width in pixels in a window {@code width} × {@code height} pixels big. */
        static int panelWidth(int width, int height, int margin, float density) {
            int wide = Math.round(WIDTH_DP * density);
            int target = width > height
                    ? Math.max(Math.round(MIN_WIDTH_DP * density), Math.min(wide, Math.round(width * LANDSCAPE_SHARE)))
                    : wide;
            return Math.max(0, Math.min(target, width - 2 * margin));
        }

        @Override
        protected void onMeasure(int widthSpec, int heightSpec) {
            int width = MeasureSpec.getSize(widthSpec) - getPaddingLeft() - getPaddingRight();
            int height = MeasureSpec.getSize(heightSpec) - getPaddingTop() - getPaddingBottom();
            LayoutParams params = (LayoutParams) panel.getLayoutParams();
            float density = getResources().getDisplayMetrics().density;
            if (anchor != null && anchor.isAttachedToWindow()) {
                params.width = Math.max(0, Math.min(Math.round(MIN_WIDTH_DP * density), width - 2 * margin));
                params.gravity = Gravity.TOP | Gravity.LEFT;
                int[] anchorAt = new int[2];
                int[] frameAt = new int[2];
                anchor.getLocationOnScreen(anchorAt);
                getLocationOnScreen(frameAt);
                frameAt[0] += getPaddingLeft();
                frameAt[1] += getPaddingTop();
                int[] at = underAnchor(anchorAt, anchor.getWidth(), anchor.getHeight(), frameAt, width, params.width, margin,
                        density, getLayoutDirection() == LAYOUT_DIRECTION_RTL);
                params.setMargins(at[0], at[1], 0, margin);
            } else {
                params.width = panelWidth(width, height, margin, density);
                params.gravity = width > height ? Gravity.END | Gravity.CENTER_VERTICAL : Gravity.CENTER;
                params.setMargins(margin, margin, margin, margin);
            }
            super.onMeasure(widthSpec, heightSpec);
        }

        private boolean outside(android.view.MotionEvent event) {
            return event.getX() < panel.getLeft() || event.getX() >= panel.getRight()
                    || event.getY() < panel.getTop() || event.getY() >= panel.getBottom();
        }

        // Touches the panel's rows leave fall through to here: beside the panel they close it, on it they do nothing.
        @Override
        public boolean onTouchEvent(android.view.MotionEvent event) {
            switch (event.getActionMasked()) {
                case android.view.MotionEvent.ACTION_DOWN:
                    pressedOutside = outside(event);
                    break;
                case android.view.MotionEvent.ACTION_UP:
                    if (pressedOutside && outside(event)) performClick();
                    pressedOutside = false;
                    break;
                case android.view.MotionEvent.ACTION_CANCEL:
                    pressedOutside = false;
                    break;
                default:
                    break;
            }
            return true;
        }

        @Override
        public boolean performClick() {
            super.performClick();
            if (onOutsideTap != null) onOutsideTap.run();
            return true;
        }
    }

    /**
     * A panel's footer: the ways out side by side in capsules of equal width. When a label would not fit its share, the last one,
     * the red one, takes a line of its own.
     */
    static final class FooterBar extends android.view.ViewGroup {
        private final int gap;
        private boolean split;

        FooterBar(Context context, int gap) {
            super(context);
            this.gap = gap;
        }

        /** True while the last capsule has its own line. */
        boolean split() {
            return split;
        }

        @Override
        protected void onMeasure(int widthSpec, int heightSpec) {
            int width = MeasureSpec.getSize(widthSpec);
            int room = width - getPaddingLeft() - getPaddingRight();
            int count = getChildCount();
            if (count == 0) {
                split = false;
                setMeasuredDimension(width, 0);
                return;
            }
            int cell = (room - gap * (count - 1)) / count;
            int widest = 0;
            for (int i = 0; i < count; i++) {
                android.view.View child = getChildAt(i);
                child.measure(MeasureSpec.makeMeasureSpec(0, MeasureSpec.UNSPECIFIED), MeasureSpec.makeMeasureSpec(0, MeasureSpec.UNSPECIFIED));
                widest = Math.max(widest, child.getMeasuredWidth());
            }
            split = count > 1 && widest > cell;
            int height = getPaddingTop() + getPaddingBottom();
            if (split) {
                height += measureLine(0, count - 1, (room - gap * (count - 2)) / (count - 1)) + gap;
                height += measureLine(count - 1, count, room);
            } else {
                height += measureLine(0, count, cell);
            }
            setMeasuredDimension(width, height);
        }

        // Children [from, to) on one line, each as tall as the tallest; answers the line's height.
        private int measureLine(int from, int to, int cell) {
            int tallest = 0;
            for (int i = from; i < to; i++) {
                android.view.View child = getChildAt(i);
                child.measure(MeasureSpec.makeMeasureSpec(cell, MeasureSpec.EXACTLY),
                        MeasureSpec.makeMeasureSpec(0, MeasureSpec.UNSPECIFIED));
                tallest = Math.max(tallest, child.getMeasuredHeight());
            }
            for (int i = from; i < to; i++) {
                android.view.View child = getChildAt(i);
                child.measure(MeasureSpec.makeMeasureSpec(child.getMeasuredWidth(), MeasureSpec.EXACTLY),
                        MeasureSpec.makeMeasureSpec(tallest, MeasureSpec.EXACTLY));
            }
            return tallest;
        }

        @Override
        protected void onLayout(boolean changed, int left, int top, int right, int bottom) {
            boolean rtl = getLayoutDirection() == LAYOUT_DIRECTION_RTL;
            int width = right - left;
            int y = getPaddingTop();
            int x = 0;
            int count = getChildCount();
            for (int i = 0; i < count; i++) {
                android.view.View child = getChildAt(i);
                if (split && i == count - 1) {
                    y += getChildAt(0).getMeasuredHeight() + gap;
                    x = 0;
                }
                int childLeft = rtl ? width - getPaddingRight() - x - child.getMeasuredWidth() : getPaddingLeft() + x;
                child.layout(childLeft, y, childLeft + child.getMeasuredWidth(), y + child.getMeasuredHeight());
                x += child.getMeasuredWidth() + gap;
            }
        }
    }

    /** Keys other than Back, Escape and B: {@code handler} gets each key's code once, as it goes down, and takes it by answering true. */
    public void setKeyHandler(java.util.function.IntPredicate handler) {
        keyHandler = handler;
    }

    /** An action that opens another page of this sheet, marked with a chevron at its end. */
    public MaterialButton addPageAction(int icon, CharSequence label) {
        MaterialButton button = addAction(icon, label);
        content.removeView(button);
        android.widget.FrameLayout frame = new android.widget.FrameLayout(getContext());
        if (television) frame.setClipChildren(false);
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

    /** A heading; returned so a sheet can hide it with what it heads. */
    public TextView addSection(int title) {
        TextView heading = sectionHeading(getContext().getText(title));
        content.addView(heading);
        return heading;
    }

    /** A heading from the host's words, such as a session's game. */
    public void addSection(CharSequence title) {
        content.addView(sectionHeading(title));
    }

    private TextView sectionHeading(CharSequence title) {
        TextView heading = new TextView(getContext());
        heading.setText(title);
        heading.setTextAppearance(getContext(), R.style.TextAppearance_Nightjar_Label);
        heading.setTextColor(MaterialColors.getColor(getContext(), com.google.android.material.R.attr.colorOnSurfaceVariant, 0));
        heading.setPadding(spacing, spacing * 2, spacing, spacing);
        ViewCompat.setAccessibilityHeading(heading, true);
        return heading;
    }

    public int spacing() {
        return spacing;
    }

    /**
     * A vertical box in the sheet that a page can fill and refill by itself, for rows that change while
     * the sheet is open. Its rows come from the {@code ViewGroup} variants below.
     */
    public LinearLayout addGroup() {
        LinearLayout group = new LinearLayout(getContext());
        group.setOrientation(LinearLayout.VERTICAL);
        if (television) group.setClipChildren(false);
        content.addView(group, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        return group;
    }

    public void addSection(android.view.ViewGroup parent, int title) {
        parent.addView(sectionHeading(getContext().getText(title)));
    }

    public MaterialButton addAction(android.view.ViewGroup parent, int icon, CharSequence label) {
        return addAction(parent, icon, label, com.google.android.material.R.attr.colorOnSurface,
                com.google.android.material.R.attr.colorOnSurfaceVariant);
    }

    /** Text under a row, or on its own: quiet, or in the error colour. */
    public TextView addNote(android.view.ViewGroup parent, CharSequence text, boolean error) {
        TextView note = new TextView(getContext());
        note.setText(text);
        note.setTextAppearance(getContext(), R.style.TextAppearance_Nightjar_Supporting);
        note.setTextColor(MaterialColors.getColor(getContext(), error ?
                androidx.appcompat.R.attr.colorError : com.google.android.material.R.attr.colorOnSurfaceVariant, 0));
        note.setPadding(spacing, 0, spacing, spacing);
        parent.addView(note, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        return note;
    }

    public void addInfo(CharSequence label, CharSequence value) {
        addInfo(content, label, value);
    }

    /** A fact with its label at the start and its value at the end. */
    public void addInfo(android.view.ViewGroup parent, CharSequence label, CharSequence value) {
        LinearLayout row = new LinearLayout(getContext());
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(spacing, spacing / 2, spacing, spacing / 2);
        TextView name = new TextView(getContext());
        name.setText(label);
        name.setTextAppearance(getContext(), R.style.TextAppearance_Nightjar_Supporting);
        name.setTextColor(MaterialColors.getColor(getContext(), com.google.android.material.R.attr.colorOnSurfaceVariant, 0));
        TextView text = new TextView(getContext());
        text.setText(value);
        text.setTextAppearance(getContext(), R.style.TextAppearance_Nightjar_Body);
        text.setTextColor(MaterialColors.getColor(getContext(), com.google.android.material.R.attr.colorOnSurface, 0));
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

    /**
     * The focus look of a full-width row, also for rows a page adds itself: the one glass ring on every form, with no zoom, so
     * its edges stay in line with the rows around it.
     */
    public void styleRow(MaterialButton button) {
        button.setBackground(new GlassControlDrawable(getContext(), GlassControlDrawable.Kind.ROW,
                getContext().getResources().getDimension(R.dimen.nj_radius_control)));
        if (television) button.setStateListAnimator(Motion.rowFocus(getContext()));
    }

    /** A toggle on a TV: side by side with room between them, so it may grow a little under the ring. */
    private void styleToggle(MaterialButton button) {
        if (television) {
            button.setStrokeColor(androidx.appcompat.content.res.AppCompatResources.getColorStateList(getContext(), R.color.card_stroke));
            button.setStateListAnimator(Motion.controlFocus(getContext()));
        }
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
        return addAction(content, icon, label, textColor, iconColor);
    }

    private MaterialButton addAction(android.view.ViewGroup parent, int icon, CharSequence label, int textColor, int iconColor) {
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
        parent.addView(button, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        styleRow(button);
        if (parent == content) claimFirstAction(button);
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
        if (panel) {
            startPanel();
            return;
        }
        getBehavior().setState(BottomSheetBehavior.STATE_EXPANDED);
        getBehavior().setSkipCollapsed(true);
        // Material's sheet behavior owns system-bar insets. Consuming them on the dialog's
        // outer content view prevents its bottom padding and hides the last action under navigation.
        if (getOwnerActivity() != null) {
            AdaptiveLayout.attachBottomSheet(getOwnerActivity(), body);
            keepSystemBarsHidden(this, getOwnerActivity());
        }
        getBehavior().setMaxWidth(spacing * 75);
        android.view.View sheet = findViewById(com.google.android.material.R.id.design_bottom_sheet);
        if (sheet != null && television) {
            floatOnTelevision(sheet);
        } else if (sheet != null) {
            GlassPanelDrawable glass = GlassPanelDrawable.sheet(getContext());
            sheet.setBackground(glass);
            // Material installs its surface during the first layout, replacing any other.
            sheet.addOnLayoutChangeListener((view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> {
                if (view.getBackground() != glass) view.setBackground(glass);
            });
        }
        if (getWindow() != null) getWindow().setDimAmount(GlassPanelDrawable.backdropDim(getContext()));
        if (!content.isInTouchMode() && firstAction != null) firstAction.requestFocus();
    }

    /** The gap a TV's sheet keeps from the screen's top and bottom edges. */
    int televisionGap() {
        return spacing * 4;
    }

    // A TV's sheet is a rounded panel lifted off the bottom edge, never taller than the screen less a gap above and below:
    // a long one (Home's Hold OK options) scrolls inside it and fades at its edge instead of running off the screen.
    private void floatOnTelevision(android.view.View sheet) {
        int gap = televisionGap();
        getBehavior().setDraggable(false);
        getBehavior().setMaxHeight(getContext().getResources().getDisplayMetrics().heightPixels - gap);
        sheet.setBackground(null);
        // Material installs its surface during the first layout, replacing the empty one.
        sheet.addOnLayoutChangeListener((view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> {
            if (view.getBackground() != null) view.setBackground(null);
        });
        if (body.getLayoutParams() instanceof android.view.ViewGroup.MarginLayoutParams) {
            ((android.view.ViewGroup.MarginLayoutParams) body.getLayoutParams()).bottomMargin = gap;
        }
        body.setBackground(GlassPanelDrawable.dialog(getContext()));
        body.setClipToOutline(true);
        scroll.setVerticalFadingEdgeEnabled(true);
        scroll.setFadingEdgeLength(spacing * 3);
        sheet.requestLayout();
    }

    // The bottom sheet becomes a still, see-through layer over the whole window: the panel frame in it places the panel
    // and closes it on a tap beside it. Back, Escape and B close it as they close a sheet.
    private void startPanel() {
        if (getOwnerActivity() != null) keepSystemBarsHidden(this, getOwnerActivity());
        BottomSheetBehavior<?> behavior = getBehavior();
        behavior.setDraggable(false);
        behavior.setSkipCollapsed(true);
        behavior.setMaxWidth(Integer.MAX_VALUE);
        behavior.setState(BottomSheetBehavior.STATE_EXPANDED);
        // The stream stays as it is around the panel: no dim over the whole picture. A page under a panel dims as under a sheet.
        if (getWindow() != null) {
            if (anchor == null) getWindow().clearFlags(android.view.WindowManager.LayoutParams.FLAG_DIM_BEHIND);
            else getWindow().setDimAmount(GlassPanelDrawable.backdropDim(getContext()));
        }
        android.view.View sheet = findViewById(com.google.android.material.R.id.design_bottom_sheet);
        if (sheet != null) {
            sheet.getLayoutParams().height = android.view.ViewGroup.LayoutParams.MATCH_PARENT;
            sheet.setBackground(null);
            // Material installs its surface during the first layout, replacing the empty one.
            sheet.addOnLayoutChangeListener((view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> {
                if (view.getBackground() != null) view.setBackground(null);
            });
            sheet.requestLayout();
        }
        if (!content.isInTouchMode() && firstAction != null) firstAction.requestFocus();
    }

    /** Runs when the sheet closes, however it closes; a page that polls the host stops there. */
    public void setOnClosed(Runnable onClosed) {
        this.onClosed = onClosed;
    }

    @Override
    protected void onStop() {
        super.onStop();
        // Removed, not hidden: a stream with the menu closed carries nothing of it.
        if (blur != null) {
            blur.close();
            blur = null;
        }
        if (onClosed != null) onClosed.run();
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
