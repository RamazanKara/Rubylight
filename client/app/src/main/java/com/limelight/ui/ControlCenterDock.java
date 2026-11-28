/**
 * The Control Center over a stream: the rail of modules, the selected module's page, and under every page Return to
 * stream and the ways out. A dock at the end edge with the rail down its side on a TV, a handheld and a phone held
 * sideways; a sheet from the bottom with the rail across its top on an upright phone ({@link ControlCenterFrame}).
 *
 * <p>It is one window, there only while open. The triggers step along the rail from anywhere; on the rail the arrows
 * along it step too and OK or the arrow toward the page enters the page. Y, X, LB and RB go to the key handler: they are
 * the user's own buttons. B, Escape and Back leave a page one level down, then close. What it is drawn on, and the one
 * blur window that may stand under it, is {@link ControlCenterSurface}'s.
 */
package com.limelight.ui;

import android.app.Activity;
import android.app.Dialog;
import android.content.Context;
import android.content.res.Resources;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowManager;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.core.content.ContextCompat;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.function.Consumer;
import java.util.function.IntPredicate;

public final class ControlCenterDock extends Dialog implements ControlCenterFrame.Listener {
    private static final int START = 1, SELECT = 2, LB = 4, RB = 8, CHORD = START | SELECT | LB | RB;

    private final Activity owner;
    private final Context context;
    private final boolean television;
    private final int spacing;
    private final LinearLayout panel, column, footer;
    private final ActionSheet.FooterBar exits;
    private final ActionSheetRows.HeaderViews header;
    private final ControlCenterFrame frame;
    private final ControlCenterPage page;
    private final ControlCenterSurface.Glass glass;
    private final ControlCenterSurface surface;
    private final DpadEchoFilter dpadEchoes = new DpadEchoFilter();
    private final Set<Integer> handledKeys = new HashSet<>();
    private final Map<String, MaterialButton> railEntries = new LinkedHashMap<>();
    // The ways out and the mark each has where there is room for it.
    private final Map<MaterialButton, Integer> exitMarks = new LinkedHashMap<>();
    private List<String> modules = new ArrayList<>();
    private String selected;
    private String placement;
    private boolean compact;
    private LinearLayout rail, railList;
    private FrameLayout railScroll;
    private TextView hintBack, hintForward;
    private MaterialButton primary;
    private Runnable backHandler, onClosed, onClosing;
    private IntPredicate keyHandler;
    private Consumer<String> onSelect;
    private Consumer<Boolean> onStep;
    private View startFocus;
    private boolean capsShown, focusPlaced, guideDown, chordArmed;
    private int held, padVendor, windowHeightDp;
    // A trigger as a key and as an axis: one pull is one step, whichever of them says it, and a trigger already held when
    // the Control Center opened steps nothing.
    private final boolean[] triggerKey = new boolean[2];
    private final Boolean[] triggerAxis = new Boolean[2];

    public ControlCenterDock(Activity owner, CharSequence title) {
        super(owner, R.style.ControlCenterWindow);
        this.owner = owner;
        setOwnerActivity(owner);
        context = getContext();
        // The window's theme is the stream's night palette whatever the app's is; high contrast and the font go on top.
        AppTheme.applyOverlays(owner, context.getTheme());
        television = ControlCenterCards.television(context);
        Resources res = context.getResources();
        spacing = Math.round(8 * res.getDisplayMetrics().density);
        int edge = res.getDimensionPixelSize(R.dimen.stream_menu_edge);
        Window window = getWindow();
        window.requestFeature(Window.FEATURE_NO_TITLE);
        window.clearFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND);
        window.setLayout(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT);
        window.setWindowAnimations(Motion.reduced(context) ? 0 : R.style.TvSheetAnimation);
        WindowCompat.setDecorFitsSystemWindows(window, false);
        window.setStatusBarColor(android.graphics.Color.TRANSPARENT);
        window.setNavigationBarColor(android.graphics.Color.TRANSPARENT);
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.P) {
            WindowManager.LayoutParams params = window.getAttributes();
            params.layoutInDisplayCutoutMode = android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R
                    ? WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
                    : WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            window.setAttributes(params);
        }

        glass = new ControlCenterSurface.Glass(context);
        panel = new LinearLayout(context);
        panel.setBackground(glass);
        panel.setClipToOutline(true);
        ViewCompat.setAccessibilityPaneTitle(panel, title);
        column = new LinearLayout(context);
        column.setOrientation(LinearLayout.VERTICAL);
        header = ActionSheetRows.buildHeader(context, title, true, spacing, edge,
                () -> { if (backHandler != null) backHandler.run(); }, this::dismiss);
        // Return to stream stands under every page: the header needs no close of its own.
        header.panelClose.setVisibility(View.GONE);
        int label = ContextCompat.getColor(context, R.color.nj_surface_control_center_label);
        header.heading.setTextColor(label);
        header.subtitle.setTextColor(ContextCompat.getColor(context, R.color.nj_surface_control_center_label_secondary));
        if (!television) {
            header.heading.setTextSize(android.util.TypedValue.COMPLEX_UNIT_SP, 16f);
            header.subtitle.setTextAppearance(context, R.style.TextAppearance_Nightjar_Caption);
            header.subtitle.setTextColor(ContextCompat.getColor(context, R.color.nj_surface_control_center_label_secondary));
        }
        column.addView(header.header);
        page = new ControlCenterPage(context);
        column.addView(page.scroll(), new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1));
        footer = new LinearLayout(context);
        footer.setOrientation(LinearLayout.VERTICAL);
        footer.setPadding(edge, spacing, edge, edge);
        if (television) {
            footer.setClipChildren(false);
            footer.setClipToPadding(false);
        }
        exits = new ActionSheet.FooterBar(context, res.getDimensionPixelSize(R.dimen.control_center_gap));
        if (television) exits.setClipChildren(false);
        column.addView(footer, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        frame = new ControlCenterFrame(context, panel, this);
        setContentView(frame);
        surface = new ControlCenterSurface(owner, panel, glass);
        View decor = owner.getWindow().getDecorView();
        float density = res.getDisplayMetrics().density;
        android.content.res.Configuration config = res.getConfiguration();
        decide(decor.getWidth() > 0 ? decor.getWidth() : Math.round(config.screenWidthDp * density),
                decor.getHeight() > 0 ? decor.getHeight() : Math.round(config.screenHeightDp * density));
        assemble();
    }

    // Where it sits in a window of this size, whether at compact width, and whether cards take the low height.
    private boolean decide(int width, int height) {
        Resources res = context.getResources();
        float density = res.getDisplayMetrics().density;
        float widthDp = width / density;
        float heightDp = height / density;
        String form = ViewportScaler.form(res.getConfiguration(), widthDp);
        String now = ControlCenterModel.placement(form, widthDp, heightDp);
        boolean tight = !television && (widthDp < 600f || heightDp < 480f);
        int nextHeightDp = Math.round(heightDp);
        boolean wasLow = ControlCenterCards.isLowWindow(context, windowHeightDp);
        boolean nowLow = ControlCenterCards.isLowWindow(context, nextHeightDp);
        boolean changed = !now.equals(placement) || tight != compact || nowLow != wasLow;
        placement = now;
        compact = tight;
        windowHeightDp = nextHeightDp;
        if (page != null) page.setWindowHeightDp(nextHeightDp);
        return changed;
    }

    private boolean isSheet() {
        return ControlCenterModel.SHEET.equals(placement);
    }

    // The panel for the placement: the rail down the dock's start side, or across the sheet's top under its grabber.
    private void assemble() {
        Resources res = context.getResources();
        boolean sheet = isSheet();
        panel.removeAllViews();
        panel.setOrientation(sheet ? LinearLayout.VERTICAL : LinearLayout.HORIZONTAL);
        float r = res.getDimension(sheet ? R.dimen.nj_radius_sheet : R.dimen.nj_radius_panel);
        float[] radii = sheet ? new float[] {r, r, r, r, 0, 0, 0, 0} : new float[] {r, r, r, r, r, r, r, r};
        glass.setRadii(radii);
        if (sheet) ActionSheetRows.addGrabber(panel, context, spacing);
        rail = new LinearLayout(context);
        rail.setOrientation(sheet ? LinearLayout.HORIZONTAL : LinearLayout.VERTICAL);
        rail.setGravity(android.view.Gravity.CENTER);
        if (!AppTheme.isHighContrast(context)) {
            int fill = ContextCompat.getColor(context, R.color.nj_surface_control_center_rail_fill);
            // Down the dock's start edge the rail's corners there are the dock's, so its fill stays inside the rim.
            boolean rtl = res.getConfiguration().getLayoutDirection() == View.LAYOUT_DIRECTION_RTL;
            android.graphics.drawable.GradientDrawable back = new android.graphics.drawable.GradientDrawable();
            back.setColor(fill);
            back.setCornerRadii(sheet ? new float[8] : rtl ? new float[] {0, 0, r, r, r, r, 0, 0} : new float[] {r, r, 0, 0, 0, 0, r, r});
            rail.setBackground(back);
        }
        int pad = res.getDimensionPixelSize(R.dimen.control_center_rail_pad);
        rail.setPadding(pad, pad, pad, pad);
        String[] caps = ControlCenterCards.triggerCaps(padVendor);
        hintBack = ControlCenterCards.triggerHint(context, caps[0]);
        hintForward = ControlCenterCards.triggerHint(context, caps[1]);
        railScroll = sheet ? new android.widget.HorizontalScrollView(context) : new android.widget.ScrollView(context) {
            @Override
            protected void onMeasure(int widthSpec, int heightSpec) {
                if (MeasureSpec.getMode(heightSpec) == MeasureSpec.EXACTLY) fitRail(MeasureSpec.getSize(heightSpec));
                super.onMeasure(widthSpec, heightSpec);
            }
        };
        railScroll.setHorizontalScrollBarEnabled(false);
        railScroll.setVerticalScrollBarEnabled(false);
        railScroll.setOverScrollMode(View.OVER_SCROLL_NEVER);
        // Where the rail has more modules than it shows, it fades out instead of ending in a cut name.
        railScroll.setVerticalFadingEdgeEnabled(!sheet);
        railScroll.setHorizontalFadingEdgeEnabled(sheet);
        railScroll.setFadingEdgeLength(spacing * 3);
        railList = new LinearLayout(context);
        railList.setOrientation(sheet ? LinearLayout.HORIZONTAL : LinearLayout.VERTICAL);
        railScroll.addView(railList, new FrameLayout.LayoutParams(sheet ? FrameLayout.LayoutParams.WRAP_CONTENT
                : FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.WRAP_CONTENT));
        LinearLayout.LayoutParams hint = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        hint.setMargins(pad, pad, pad, pad);
        rail.addView(hintBack, hint);
        rail.addView(railScroll, sheet ? new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1)
                : new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1));
        rail.addView(hintForward, new LinearLayout.LayoutParams(hint));
        panel.addView(rail, sheet ? new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT) : new LinearLayout.LayoutParams(
                res.getDimensionPixelSize(television ? R.dimen.nj_size_control_center_rail_tv : R.dimen.nj_size_control_center_rail),
                LinearLayout.LayoutParams.MATCH_PARENT));
        panel.addView(column, sheet ? new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1)
                : new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.MATCH_PARENT, 1));
        fillRail();
        markExits();
        frame.place(sheet, compact, television, radii, page.scroll());
    }

    private void fillRail() {
        Resources res = context.getResources();
        boolean sheet = isSheet();
        boolean had = railFocused();
        railList.removeAllViews();
        railEntries.clear();
        for (String module : modules) {
            MaterialButton entry = ControlCenterCards.railEntry(context, module, !sheet);
            entry.setOnClickListener(view -> {
                if (module.equals(selected)) {
                    // Chosen again, it leads into its page.
                    View target = first();
                    if (!panel.isInTouchMode() && target != null) target.requestFocus();
                } else if (onSelect != null) {
                    onSelect.accept(module);
                }
            });
            railList.addView(entry, sheet ? new LinearLayout.LayoutParams(
                    res.getDimensionPixelSize(R.dimen.control_center_rail_entry_width), LinearLayout.LayoutParams.WRAP_CONTENT)
                    : new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT));
            railEntries.put(module, entry);
        }
        mark();
        if (had) focusRail();
    }

    // On a dock too short for the design's height, a handheld's, the modules give up their spare room before the rail has
    // to scroll, never down to less than a touch target: {@code room} is what the rail has for them.
    private void fitRail(int room) {
        if (railEntries.isEmpty() || room <= 0) return;
        Resources res = context.getResources();
        int full = res.getDimensionPixelSize(R.dimen.control_center_rail_entry);
        int each = Math.max(res.getDimensionPixelSize(R.dimen.nj_size_touch_min), Math.min(full, room / railEntries.size()));
        for (MaterialButton entry : railEntries.values()) ControlCenterCards.fitRailEntry(entry, each, each < full);
    }

    // The selected module is lit and is the rail's one stop for the D-pad: arrows from the page always land on it.
    private void mark() {
        int at = modules.indexOf(selected);
        for (Map.Entry<String, MaterialButton> entry : railEntries.entrySet()) {
            boolean on = entry.getKey().equals(selected);
            entry.getValue().setSelected(on);
            entry.getValue().setFocusable(on);
        }
        // The caps say what the triggers do, and fade where the rail ends.
        boolean hints = capsShown && modules.size() > 1;
        hintBack.setVisibility(hints ? View.VISIBLE : View.GONE);
        hintForward.setVisibility(hints ? View.VISIBLE : View.GONE);
        hintBack.setAlpha(at <= 0 ? 0.35f : 1f);
        hintForward.setAlpha(at >= modules.size() - 1 ? 0.35f : 1f);
        MaterialButton shown = railEntries.get(selected);
        if (shown != null) railScroll.post(() -> reveal(shown));
    }

    private void reveal(View entry) {
        if (entry.getParent() != railList) return;
        if (isSheet()) railScroll.scrollTo(Math.max(0, entry.getLeft() - (railScroll.getWidth() - entry.getWidth()) / 2), 0);
        else railScroll.scrollTo(0, Math.max(0, entry.getTop() - (railScroll.getHeight() - entry.getHeight()) / 2));
    }

    /** The rail: the modules that have something, in order, and the one whose page shows. */
    void setRail(List<String> shown, String selected) {
        this.selected = selected;
        if (!shown.equals(modules)) {
            modules = new ArrayList<>(shown);
            fillRail();
        } else {
            mark();
        }
    }

    /** {@link ControlCenterModel#SHEET} or {@link ControlCenterModel#DOCK}. */
    public String placement() {
        return placement;
    }

    ControlCenterPage page() {
        return page;
    }

    ControlCenterFrame frame() {
        return frame;
    }

    /** The Control Center itself, inside its window. */
    View panel() {
        return panel;
    }

    /** A module's entry on the rail, or null for one the rail does not show. */
    MaterialButton railEntry(String module) {
        return railEntries.get(module);
    }

    /** The trigger's cap at the rail's end: the one stepping forward, or back. */
    TextView hint(boolean forward) {
        return forward ? hintForward : hintBack;
    }

    ControlCenterSurface surface() {
        return surface;
    }

    /** The blur under the Control Center while it is open and blurred; null otherwise. */
    MenuBlurWindow blurWindow() {
        return surface.blurWindow();
    }

    /** A rail entry was chosen; a trigger, a rail arrow or a swipe asked for the next module (true) or the one before. */
    void setNavigation(Consumer<String> onSelect, Consumer<Boolean> onStep) {
        this.onSelect = onSelect;
        this.onStep = onStep;
    }

    /** Y, X, LB and RB, the user's own buttons: true when the key ran something. */
    public void setKeyHandler(IntPredicate handler) {
        keyHandler = handler;
    }

    /** Runs when the Control Center closes, however it closes. */
    public void setOnClosed(Runnable onClosed) {
        this.onClosed = onClosed;
    }

    /** Runs as the Control Center starts to close, its views still in their window: the focus is still where it was. */
    void setOnClosing(Runnable onClosing) {
        this.onClosing = onClosing;
    }

    @Override
    public void dismiss() {
        if (isShowing() && onClosing != null && android.os.Looper.myLooper() == android.os.Looper.getMainLooper()) {
            Runnable closing = onClosing;
            onClosing = null;
            closing.run();
        }
        super.dismiss();
    }

    /** While set, Back, Escape and B run {@code handler} instead of closing: a page one level down returns to its module. */
    public void setBackHandler(Runnable handler) {
        backHandler = handler;
        header.back.setVisibility(handler != null ? View.VISIBLE : View.GONE);
    }

    public void setHeading(CharSequence title) {
        header.heading.setText(title);
        ViewCompat.setAccessibilityPaneTitle(panel, title);
    }

    CharSequence heading() {
        return header.heading.getText();
    }

    /** The quieter line under the title, such as the stream's mode; null or empty for none. */
    public void setSubtitle(CharSequence text) {
        header.subtitle.setText(text);
        header.subtitle.setVisibility(text != null && text.length() > 0 ? View.VISIBLE : View.GONE);
    }

    public TextView subtitle() {
        return header.subtitle;
    }

    /** The game's cover beside the title; null for none. */
    public void setCover(android.graphics.Bitmap art) {
        header.cover.setImageBitmap(art);
        header.cover.setVisibility(art == null ? View.GONE : View.VISIBLE);
    }

    android.widget.ImageView cover() {
        return header.cover;
    }

    /** Shows the controller caps, the triggers' on the rail among them, and says them; the buttons work either way. */
    void setCapsShown(boolean shown) {
        capsShown = shown;
        page.setCapsShown(shown);
        if (hintBack != null) mark();
    }

    /**
     * Starts the footer again: Return to stream, the one amber control, as wide as the footer; the ways out follow with
     * {@link #addExit}.
     */
    MaterialButton setPrimary(CharSequence label) {
        boolean had = primary != null && primary.hasFocus();
        footer.removeAllViews();
        exits.removeAllViews();
        exitMarks.clear();
        primary = ControlCenterCards.primary(context, label, capsShown ? "B" : null);
        primary.setOnClickListener(view -> dismiss());
        footer.addView(primary, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        LinearLayout.LayoutParams under = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        under.topMargin = context.getResources().getDimensionPixelSize(R.dimen.control_center_gap);
        footer.addView(exits, under);
        if (had) primary.requestFocus();
        return primary;
    }

    /** A way out under Return to stream; {@code destructive} is the one red control. */
    MaterialButton addExit(int icon, CharSequence label, boolean destructive, String hotkey) {
        MaterialButton button = ActionSheetRows.addFooterAction(context, exits, icon, label, destructive, hotkey, spacing,
                capsShown, television);
        // A finger needs the token's 48 dp, which the sheet's capsule is under.
        int touch = context.getResources().getDimensionPixelSize(R.dimen.nj_size_touch_min);
        button.setMinHeight(touch);
        button.setMinimumHeight(touch);
        exitMarks.put(button, icon);
        markExits();
        return button;
    }

    // On the compact dock the ways out are their names alone: with its mark, Quit game in most languages does not fit
    // beside Disconnect and takes a row of its own from a page that shows two.
    private void markExits() {
        boolean marks = !compact || isSheet();
        for (Map.Entry<MaterialButton, Integer> exit : exitMarks.entrySet()) {
            if (!marks) exit.getKey().setIcon(null);
            else if (exit.getKey().getIcon() == null) exit.getKey().setIconResource(exit.getValue());
        }
    }

    /** Return to stream. */
    MaterialButton primary() {
        return primary;
    }

    /** The ways out, in order. */
    ViewGroup exits() {
        return exits;
    }

    /** Where the focus goes when the Control Center opens, such as the card it was on when it closed; null for the first. */
    void setStartFocus(View view) {
        startFocus = view;
    }

    private View start() {
        if (startFocus != null && startFocus.isAttachedToWindow() && startFocus.isShown()) return startFocus;
        return first();
    }

    private View first() {
        return page.firstAction() != null ? page.firstAction() : primary;
    }

    /**
     * Opened by a controller after a touch: the window leaves touch mode and the starting card shows focus at once, as
     * after a D-pad press, instead of the first press only revealing it.
     */
    public void focusFirstActionFromController() {
        panel.post(() -> {
            View start = start();
            if (start != null && isShowing()) start.requestFocusFromTouch();
        });
    }

    /** Moves D-pad and keyboard focus to the page's first card or row, as when a page opens. */
    public void focusFirstAction() {
        View first = first();
        if (!panel.isInTouchMode() && first != null) first.requestFocus();
    }

    boolean railFocused() {
        View focus = getCurrentFocus();
        return focus != null && focus.getParent() == railList;
    }

    /** Puts the focus on the selected module's entry. */
    void focusRail() {
        MaterialButton entry = railEntries.get(selected);
        if (entry != null && !panel.isInTouchMode()) entry.requestFocus();
    }

    @Override
    public void show() {
        if (isShowing()) return;
        // Before this window, so the blur stands under it: it blurs the game, not the Control Center.
        surface.open();
        try {
            super.show();
        } catch (RuntimeException e) {
            // A stream that is going refuses the window: nothing of the Control Center may stay behind.
            surface.close();
            throw e;
        }
        View start = start();
        if (!panel.isInTouchMode() && start != null) start.requestFocus();
        panel.post(() -> {
            View now = start();
            if (!panel.isInTouchMode() && isShowing() && panel.findFocus() == null && now != null) now.requestFocus();
        });
    }

    @Override
    protected void onStop() {
        super.onStop();
        // Removed, not hidden: a stream with the Control Center closed carries nothing of it.
        surface.close();
        if (onClosed != null) onClosed.run();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        if (!hasFocus) {
            // Releases that arrive while another window has the focus never reach this one.
            dpadEchoes.reset();
            held = 0;
            chordArmed = guideDown = false;
            triggerKey[0] = triggerKey[1] = false;
        }
        super.onWindowFocusChanged(hasFocus);
        // A remote's first press must move the ring, not reveal it. A new window starts in touch mode even when a key
        // opened it, so ask the window behind: that is where the key landed.
        if (hasFocus && !focusPlaced) {
            focusPlaced = true;
            View start = start();
            if (!owner.getWindow().getDecorView().isInTouchMode() && panel.findFocus() == null && start != null) {
                start.requestFocusFromTouch();
            }
        }
    }

    @Override
    public void onBackPressed() {
        Haptics.feedback(panel, Haptics.Event.BACK);
        if (backHandler != null) backHandler.run();
        else dismiss();
    }

    // ControlCenterFrame.Listener

    @Override
    public void onOutsideTap() {
        cancel();
    }

    @Override
    public void onDraggedAway() {
        // The sheet is off the screen already: no fade of an empty window.
        if (getWindow() != null) getWindow().setWindowAnimations(0);
        cancel();
    }

    @Override
    public void onSwipe(boolean forward) {
        step(forward);
    }

    @Override
    public void onPanelMoved() {
        if (surface != null) surface.place();
    }

    @Override
    public void onResized(int width, int height) {
        if (!decide(width, height)) return;
        // Never during a layout pass. A blur under the old shape would show beside the new one: baked until it closes.
        panel.post(() -> {
            View focus = getCurrentFocus();
            boolean onRail = railFocused();
            surface.bake();
            assemble();
            if (onRail) focusRail();
            else if (focus != null && focus.isAttachedToWindow()) focus.requestFocus();
        });
    }

    private void step(boolean forward) {
        if (onStep == null || modules.size() < 2) return;
        Haptics.feedback(panel, Haptics.Event.FOCUS_TICK);
        onStep.accept(forward);
    }

    private void padSeen(int vendor) {
        boolean show = !capsShown;
        if (vendor != padVendor) {
            padVendor = vendor;
            String[] caps = ControlCenterCards.triggerCaps(vendor);
            hintBack.setText(caps[0]);
            hintForward.setText(caps[1]);
        }
        // A pad that was not there when the Control Center opened: the rail says what its triggers do from now on.
        if (show) {
            capsShown = true;
            mark();
        }
    }

    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        if (event.getAction() == KeyEvent.ACTION_DOWN) {
            // The screen behind learns what chose here: a pad's Disconnect leaves a PC list a pad can act on.
            InputSource source = InputSource.of(event);
            if (source != null) InputSource.remember(source);
            if (source == InputSource.PAD) padSeen(InputSource.vendorOf(event));
        }
        return dpadEchoes.drop(event) || steer(event.getKeyCode(), event) || super.dispatchKeyEvent(event);
    }

    private static int chordBit(int key) {
        switch (key) {
            case KeyEvent.KEYCODE_BUTTON_START: return START;
            case KeyEvent.KEYCODE_BUTTON_SELECT: return SELECT;
            case KeyEvent.KEYCODE_BUTTON_L1: return LB;
            case KeyEvent.KEYCODE_BUTTON_R1: return RB;
            default: return 0;
        }
    }

    // The keys the Control Center takes before its views: true when the event is used up.
    private boolean steer(int key, KeyEvent event) {
        boolean down = event.getAction() == KeyEvent.ACTION_DOWN, up = event.getAction() == KeyEvent.ACTION_UP;
        boolean first = down && event.getRepeatCount() == 0;
        if (key == KeyEvent.KEYCODE_ESCAPE || key == KeyEvent.KEYCODE_BUTTON_B) {
            if (!event.hasNoModifiers()) return false;
            if (up && !event.isCanceled()) onBackPressed();
            return true;
        }
        if (key == KeyEvent.KEYCODE_BUTTON_L2 || key == KeyEvent.KEYCODE_BUTTON_R2) {
            int side = key == KeyEvent.KEYCODE_BUTTON_R2 ? 1 : 0;
            if (first) pull(side, true, true);
            else if (up) pull(side, true, false);
            return true;
        }
        if (key == KeyEvent.KEYCODE_BUTTON_MODE) {
            // Guide closes what it opened, on a press of its own: the hold that opened the Control Center ends here too.
            if (first) guideDown = true;
            else if (up) {
                boolean close = guideDown && !event.isCanceled();
                guideDown = false;
                if (close) dismiss();
            }
            return true;
        }
        int bit = chordBit(key);
        if (bit != 0) {
            boolean chording = (held & (START | SELECT)) != 0;
            if (first) {
                held |= bit;
                if (held == CHORD) chordArmed = true;
            } else if (up) {
                held &= ~bit;
                // The menu chord closes it as it opened it: once every button of it is let go.
                if (chordArmed && held == 0) {
                    chordArmed = false;
                    dismiss();
                    return true;
                }
            }
            // LB and RB pressed into the chord are the chord's, not the buttons of their tiles.
            if ((bit == LB || bit == RB) && (chording || chordArmed)) return true;
        }
        if (keyHandler != null && event.hasNoModifiers()) {
            // The release of a key the handler took goes with it, so nothing below sees half a press.
            if (down) {
                if (event.getRepeatCount() > 0 ? handledKeys.contains(key) : keyHandler.test(key)) {
                    handledKeys.add(key);
                    return true;
                }
            } else if (up && handledKeys.remove(key)) {
                return true;
            }
        }
        if (onRail(key, down, first)) return true;
        // A Y no tile takes would come back as Space and press the focused card (Nightjar #81).
        return UiNavigation.swallowsPadY(key, getCurrentFocus());
    }

    // On the rail the arrows along it step through the modules; OK and the arrow toward the page enter the page.
    private boolean onRail(int key, boolean down, boolean first) {
        if (!railFocused()) return false;
        boolean sheet = isSheet();
        boolean rtl = panel.getLayoutDirection() == View.LAYOUT_DIRECTION_RTL;
        int back = sheet ? (rtl ? KeyEvent.KEYCODE_DPAD_RIGHT : KeyEvent.KEYCODE_DPAD_LEFT) : KeyEvent.KEYCODE_DPAD_UP;
        int forward = sheet ? (rtl ? KeyEvent.KEYCODE_DPAD_LEFT : KeyEvent.KEYCODE_DPAD_RIGHT) : KeyEvent.KEYCODE_DPAD_DOWN;
        int enter = sheet ? KeyEvent.KEYCODE_DPAD_DOWN : rtl ? KeyEvent.KEYCODE_DPAD_LEFT : KeyEvent.KEYCODE_DPAD_RIGHT;
        if (key == back || key == forward) {
            if (down) step(key == forward);
            return true;
        }
        if (key == enter || key == KeyEvent.KEYCODE_DPAD_CENTER || key == KeyEvent.KEYCODE_ENTER
                || key == KeyEvent.KEYCODE_NUMPAD_ENTER || key == KeyEvent.KEYCODE_BUTTON_A) {
            View target = first();
            if (first && target != null) {
                Haptics.feedback(panel, Haptics.Event.CONFIRM);
                target.requestFocus();
            }
            return true;
        }
        return false;
    }

    // One pull of a trigger is one step, whether the pad says it as a key, as an axis or as both.
    private void pull(int side, boolean key, boolean pressed) {
        boolean was = triggerKey[side] || Boolean.TRUE.equals(triggerAxis[side]);
        if (key) triggerKey[side] = pressed;
        else triggerAxis[side] = pressed;
        if (pressed && !was) step(side == 1);
    }

    @Override
    public boolean dispatchGenericMotionEvent(MotionEvent event) {
        if ((event.getSource() & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK
                && event.getActionMasked() == MotionEvent.ACTION_MOVE) {
            axis(0, Math.max(event.getAxisValue(MotionEvent.AXIS_LTRIGGER), event.getAxisValue(MotionEvent.AXIS_BRAKE)));
            axis(1, Math.max(event.getAxisValue(MotionEvent.AXIS_RTRIGGER), event.getAxisValue(MotionEvent.AXIS_GAS)));
        }
        return super.dispatchGenericMotionEvent(event);
    }

    private void axis(int side, float value) {
        if (triggerAxis[side] == null) {
            // The first reading only says where the trigger is: one held since before the Control Center opened steps nothing.
            triggerAxis[side] = value >= 0.5f;
        } else if (value >= 0.5f && !triggerAxis[side]) {
            pull(side, false, true);
        } else if (value <= 0.25f && triggerAxis[side]) {
            pull(side, false, false);
        }
    }

    @Override
    public boolean dispatchTouchEvent(MotionEvent event) {
        if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
            InputSource source = InputSource.of(event);
            if (source != null) InputSource.remember(source);
        }
        return super.dispatchTouchEvent(event);
    }
}
