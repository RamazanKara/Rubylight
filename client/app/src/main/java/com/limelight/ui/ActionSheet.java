package com.limelight.ui;

import android.content.Context;
import android.widget.ImageButton;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.activity.OnBackPressedCallback;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;
import androidx.core.widget.NestedScrollView;

import com.google.android.material.bottomsheet.BottomSheetBehavior;
import com.google.android.material.bottomsheet.BottomSheetDialog;
import com.google.android.material.button.MaterialButton;
import com.limelight.R;

/**
 * Moonlit Glass bottom sheet and floating panel dialog shared by the home screens and the in-stream
 * menu, handling TV D-pad focus, backdrop blur, anchored placement, and controller/keyboard dismissal.
 */
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
    // A remote has no face buttons: the ways out show their controller caps only while a controller could press them.
    private boolean capsShown = true;
    // A floating panel over the stream instead of a sheet across the bottom; its footer stays out of the scroll.
    private final boolean panel;
    private final FooterBar footer;
    // A panel's cover beside its title, its surface, and the blur under it while it is open.
    private android.widget.ImageView cover;
    private GlassPanelDrawable menuGlass;
    private MenuBlurWindow blur;
    // The page blurred behind a sheet, or behind a panel that stands on a page; never over a stream.
    private PageBlur pageBlur;
    private boolean pageBlurAsked;
    private MaterialButton panelClose;
    // The control a panel hangs under, as Home's PC sheet hangs under its capsule; null for a panel docked by the window.
    private android.view.View anchor;
    // A panel under an anchor starts below the top row: its toggles and headings are a size tighter, so Home's View sheet
    // fits a 1080p TV whole instead of cutting its last part off at the panel's foot.
    private boolean compact;
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
        // The screen behind learns what chose here: a pad's Disconnect in the stream menu leaves a PC list a pad can act on.
        if (event.getAction() == android.view.KeyEvent.ACTION_DOWN) rememberInput(InputSource.of(event));
        return dpadEchoes.drop(event) || super.dispatchKeyEvent(event);
    }

    @Override
    public boolean dispatchTouchEvent(android.view.MotionEvent event) {
        if (event.getActionMasked() == android.view.MotionEvent.ACTION_DOWN) rememberInput(InputSource.of(event));
        return super.dispatchTouchEvent(event);
    }

    private static void rememberInput(InputSource source) {
        if (source != null) InputSource.remember(source);
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
        sheet.compact = true;
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
        content = new LinearLayout(context);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(edge - spacing, 0, edge - spacing, panel ? spacing / 2 : spacing * 2);
        if (television) content.setClipToPadding(false);
        ActionSheetRows.HeaderViews header = ActionSheetRows.buildHeader(context, title, panel, spacing, edge,
                () -> { if (backHandler != null) backHandler.run(); }, this::dismiss);
        heading = header.heading;
        subtitle = header.subtitle;
        back = header.back;
        cover = header.cover;
        panelClose = header.panelClose;
        body.addView(header.header);
        scroll = new StillScroll(context);
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
            if (!television) ActionSheetRows.addGrabber(body, context, spacing);
            setContentView(body);
        }
        setOnKeyListener((dialog, key, event) -> handleDialogKey(key, event));
        getOnBackPressedDispatcher().addCallback(pageBack);
    }

    private boolean handleDialogKey(int key, android.view.KeyEvent event) {
        if (key != android.view.KeyEvent.KEYCODE_ESCAPE && key != android.view.KeyEvent.KEYCODE_BUTTON_B) {
            if (keyHandler != null && event.hasNoModifiers()) {
                // The release of a key the handler took goes with it, so nothing below sees half a press.
                if (event.getAction() == android.view.KeyEvent.ACTION_DOWN) {
                    if (event.getRepeatCount() > 0 ? handledKeys.contains(key) : keyHandler.test(key)) {
                        handledKeys.add(key);
                        return true;
                    }
                } else if (event.getAction() == android.view.KeyEvent.ACTION_UP && handledKeys.remove(key)) {
                    return true;
                }
            }
            // A Y the sheet has no tile for would come back as Space and press the focused action (Nightjar #81).
            return UiNavigation.swallowsPadY(key, getCurrentFocus());
        }
        if (!event.hasNoModifiers()) return false;
        if (event.getAction() == android.view.KeyEvent.ACTION_UP && !event.isCanceled()) {
            if (pageBack.isEnabled()) pageBack.handleOnBackPressed();
            else dismiss();
        }
        return true;
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
        // A stream that is finishing refuses a new window: the panel and its blur must not be created over it.
        android.app.Activity owner = getOwnerActivity();
        if (owner != null && (owner.isFinishing() || owner.isDestroyed())) {
            return;
        }
        // Shown first, so it stands under the panel: it blurs the stream, not the menu.
        // A panel over a page samples nothing live: its surface stays the near-opaque one, over the page's dim.
        if (panel && anchor == null && blur == null && !isShowing() && owner != null) {
            int radius = Math.round(GlassSpec.menuWindow(getContext()).blur);
            blur = MenuBlurWindow.openIfAllowed(owner, radius, on -> menuGlass.setOverBlur(on));
            menuGlass.setOverBlur(blur != null);
            if (blur != null) {
                body.addOnLayoutChangeListener((view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> placeBlur());
            }
        }
        try {
            super.show();
        } catch (RuntimeException e) {
            // A window this stream no longer takes must not leave its blur behind on its own.
            closeBlur();
            throw e;
        }
    }

    private void blurPage() {
        if (pageBlurAsked) {
            // Shown again: battery saver or the system's switch may have changed since.
            if (pageBlur != null) pageBlur.refresh();
            return;
        }
        pageBlurAsked = true;
        pageBlur = PageBlur.behind(this);
    }

    /** The blur of the page behind this sheet, where one may ever stand; null over a stream. */
    PageBlur pageBlur() {
        return pageBlur;
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

    /**
     * Opened by a controller after a touch: the window leaves touch mode and the first action shows focus at once, as
     * after a D-pad press, instead of the first press only revealing it.
     */
    public void focusFirstActionFromController() {
        content.post(() -> {
            if (firstAction != null && isShowing()) firstAction.requestFocusFromTouch();
        });
    }

    /** Moves D-pad and keyboard focus to the first row, as when the sheet opens. */
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
        return addToggleRow(null);
    }

    /** {@link #addToggleRow()} in {@code group}, a column of {@link #addColumns}, or the sheet's own content for null. */
    public LinearLayout addToggleRow(android.view.ViewGroup group) {
        LinearLayout row = new ToggleRow(getContext(), spacing, compactTile());
        row.setPadding(spacing, spacing, spacing, spacing);
        // A focused toggle grows past the row's padding on TV; clipping there cut off its rounded ends.
        if (television) {
            row.setClipToPadding(false);
            row.setClipChildren(false);
        }
        (group == null ? content : group).addView(row, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        return row;
    }

    /**
     * {@code count} columns side by side, each an equal share of the sheet's width, for a page whose parts go across
     * instead of down: a TV's View sheet, whose panel hanging under its button has no room below.
     */
    public LinearLayout[] addColumns(int count) {
        return ActionSheetRows.addColumns(content, getContext(), count);
    }

    /** A toggle in {@code row}, filled while on. A click flips {@code isChecked()} before its listener runs. */
    public MaterialButton addToggle(LinearLayout row, int icon, CharSequence label, boolean checked) {
        MaterialButton button = ActionSheetRows.addToggle(getContext(), row, icon, label, checked,
                spacing, compact, compactTile(), television);
        claimFirstAction(button);
        return button;
    }

    /** A toggle tile's least height: a size down in a panel under an anchor. */
    private int compactTile() {
        return compact ? spacing * 15 / 2 : spacing * 9;
    }

    /** A grid of compact tiles, as many to a row as fit, for {@link TileGrid#addTile}. */
    public TileGrid addTileGrid() {
        return ActionSheetRows.addTileGrid(content, getContext(), spacing, television);
    }

    /**
     * A way out. In a panel it is a capsule in the footer; {@code destructive} there is the one red control. In a sheet it is a
     * row like {@link #addAction} or {@link #addDestructiveAction}.
     */
    public MaterialButton addFooterAction(int icon, CharSequence label, boolean destructive) {
        return addFooterAction(icon, label, destructive, null);
    }

    /** Draws the controller caps on the ways out and status rows added from here on, and says them; their buttons work either way. */
    public void setCapsShown(boolean shown) {
        capsShown = shown;
    }

    /** The same with the controller button that runs it ({@code "Y"}) at its end, or null for none. */
    public MaterialButton addFooterAction(int icon, CharSequence label, boolean destructive, String hotkey) {
        if (footer == null) {
            MaterialButton row = destructive ? addDestructiveAction(icon, label) : addAction(icon, label);
            if (hotkey != null) row.setText(label + "  " + hotkey);
            return row;
        }
        return ActionSheetRows.addFooterAction(getContext(), footer, icon, label, destructive, hotkey, spacing, capsShown, television);
    }

    /** Rows standing together on one flat tint, hairlines between them. */
    public LinearLayout addRowGroup() {
        return ActionSheetRows.addRowGroup(content, getContext(), spacing);
    }

    /** The flat tint and hairlines of {@link #addRowGroup}, also for a group a layout holds. */
    static void styleRowGroup(LinearLayout group) {
        ActionSheetRows.styleRowGroup(group);
    }

    /** A status row in {@link #addRowGroup}: its name, a value at its end, a chevron. */
    public MenuRow addStatusRow(LinearLayout group, CharSequence label, CharSequence value) {
        return addStatusRow(group, label, value, null);
    }

    /** The same with the controller button that opens it ({@code "Y"}) before its chevron, or null for none. */
    public MenuRow addStatusRow(LinearLayout group, CharSequence label, CharSequence value, String hotkey) {
        MenuRow row = new MenuRow(getContext(), label, value);
        row.setHotkey(hotkey, capsShown);
        group.addView(row, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT));
        return row;
    }

    /**
     * A slider in {@link #addRowGroup}, its name and value over it. The whole row takes the ring while the slider has the focus;
     * D-pad left and right move it. Answers the line showing the value.
     */
    public TextView addSliderRow(LinearLayout group, CharSequence label, CharSequence value, StepSlider slider) {
        return ActionSheetRows.addSliderRow(getContext(), group, label, value, slider, spacing);
    }

    /** Keys other than Back, Escape and B: {@code handler} gets each key's code once, as it goes down, and takes it by answering true. */
    public void setKeyHandler(java.util.function.IntPredicate handler) {
        keyHandler = handler;
    }

    /** An action that opens another page of this sheet, marked with a chevron at its end. */
    public MaterialButton addPageAction(int icon, CharSequence label) {
        return ActionSheetRows.wrapPageAction(content, getContext(), addAction(icon, label), spacing, television);
    }

    private void claimFirstAction(MaterialButton button) {
        if (firstAction == null) {
            firstAction = button;
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) button.setFocusedByDefault(true);
        }
    }

    /** A heading; returned so a sheet can hide it with what it heads. */
    public TextView addSection(int title) {
        TextView heading = ActionSheetRows.sectionHeading(getContext(), getContext().getText(title), spacing, compact);
        content.addView(heading);
        return heading;
    }

    /** A heading from the host's words, such as a session's game. */
    public void addSection(CharSequence title) {
        content.addView(ActionSheetRows.sectionHeading(getContext(), title, spacing, compact));
    }

    public int spacing() {
        return spacing;
    }

    /**
     * A vertical box in the sheet that a page can fill and refill by itself, for rows that change while
     * the sheet is open. Its rows come from the {@code ViewGroup} variants below.
     */
    public LinearLayout addGroup() {
        return ActionSheetRows.addGroup(content, getContext(), television);
    }

    public TextView addSection(android.view.ViewGroup parent, int title) {
        TextView heading = ActionSheetRows.sectionHeading(getContext(), getContext().getText(title), spacing, compact);
        parent.addView(heading);
        return heading;
    }

    public MaterialButton addAction(android.view.ViewGroup parent, int icon, CharSequence label) {
        return addAction(parent, icon, label, com.google.android.material.R.attr.colorOnSurface,
                com.google.android.material.R.attr.colorOnSurfaceVariant);
    }

    /** Text under a row, or on its own: quiet, or in the error colour. */
    public TextView addNote(android.view.ViewGroup parent, CharSequence text, boolean error) {
        return ActionSheetRows.addNote(getContext(), parent, text, error, spacing);
    }

    public void addInfo(CharSequence label, CharSequence value) {
        addInfo(content, label, value);
    }

    /** A fact with its label at the start and its value at the end. */
    public void addInfo(android.view.ViewGroup parent, CharSequence label, CharSequence value) {
        ActionSheetRows.addInfo(getContext(), parent, label, value, spacing);
    }

    /**
     * The focus look of a full-width row, also for rows a page adds itself: the one glass ring on every form, with no zoom, so
     * its edges stay in line with the rows around it.
     */
    public void styleRow(MaterialButton button) {
        ActionSheetRows.styleRow(getContext(), television, button);
    }

    public MaterialButton addAction(int icon, CharSequence label) {
        return addAction(icon, label, com.google.android.material.R.attr.colorOnSurface,
                com.google.android.material.R.attr.colorOnSurfaceVariant);
    }

    /** An action with a second, quieter line that says what will happen. */
    public MaterialButton addAction(int icon, CharSequence label, CharSequence note) {
        return addAction(icon, ActionSheetRows.withNote(getContext(), label, note));
    }

    /** Rewrites an action added with a note, for a row that changes a setting in place. */
    public void setActionText(MaterialButton action, CharSequence label, CharSequence note) {
        action.setText(ActionSheetRows.withNote(getContext(), label, note));
    }

    /** An action that removes something, in the error colour and kept last in the sheet. */
    public MaterialButton addDestructiveAction(int icon, CharSequence label) {
        return addAction(icon, label, androidx.appcompat.R.attr.colorError, androidx.appcompat.R.attr.colorError);
    }

    private MaterialButton addAction(int icon, CharSequence label, int textColor, int iconColor) {
        return addAction(content, icon, label, textColor, iconColor);
    }

    private MaterialButton addAction(android.view.ViewGroup parent, int icon, CharSequence label, int textColor, int iconColor) {
        MaterialButton button = ActionSheetRows.addAction(getContext(), parent, icon, label, textColor, iconColor, spacing, television);
        if (parent == content) claimFirstAction(button);
        return button;
    }

    public void addDivider() {
        ActionSheetRows.addDivider(content, getContext(), spacing);
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
        // The stream menu's panel keeps the picture sharp around it; everything on a page blurs the page where it may.
        if (!(panel && anchor == null)) blurPage();
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

    /** Removes the blur under the panel, if this sheet opened one, and returns the glass to its opaque surface. */
    private void closeBlur() {
        if (blur == null) return;
        MenuBlurWindow open = blur;
        blur = null;
        open.close();
        if (menuGlass != null) menuGlass.setOverBlur(false);
    }

    @Override
    protected void onStop() {
        super.onStop();
        // Removed, not hidden: a stream with the menu closed carries nothing of it.
        closeBlur();
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

    static final class ToggleRow extends ActionSheetLayoutViews.ToggleRowBase {
        ToggleRow(Context context, int gap) {
            super(context, gap);
        }

        ToggleRow(Context context, int gap, int tileHeight) {
            super(context, gap, tileHeight);
        }
    }

    static final class PanelFrame extends ActionSheetLayoutViews.PanelFrameBase {
        PanelFrame(Context context, android.view.View panel, int margin) {
            super(context, panel, margin);
        }
    }

    static final class FooterBar extends ActionSheetLayoutViews.FooterBarBase {
        FooterBar(Context context, int gap) {
            super(context, gap);
        }
    }

    // Not final: a test records the sounds the scroll itself would play.
    static class StillScroll extends ActionSheetLayoutViews.StillScrollBase {
        StillScroll(Context context) {
            super(context);
        }
    }
}
