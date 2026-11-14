package com.limelight.console;

import android.app.Activity;
import android.content.res.ColorStateList;
import android.graphics.drawable.GradientDrawable;
import android.view.View;
import android.view.ViewGroup;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.core.content.res.ResourcesCompat;
import androidx.core.graphics.ColorUtils;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.color.MaterialColors;
import com.limelight.R;
import com.limelight.ui.AppTheme;
import com.limelight.ui.BackdropView;
import com.limelight.ui.GlassForm;
import com.limelight.ui.GlassHost;
import com.limelight.ui.GlassProbe;
import com.limelight.ui.GlassTier;

/**
 * Home's glass around its lists: the tier, the ambient, and the chrome placed for the form factor. A phone keeps the tabs
 * in the floating bar at the foot, a handheld moves them to the top row, a TV's layout has them there already.
 */
final class HomeChrome {
    private static final float[] PHONE_STOPS = {0.42f, 0.63f, 0.78f};
    private static final float[] PHONE_ALPHAS = {0f, 0.94f, 1f};
    private static final float[] HANDHELD_STOPS = {0.32f, 0.62f, 1f};
    private static final float[] HANDHELD_ALPHAS = {0f, 0.92f, 1f};
    private static final float[] TV_STOPS = {0.34f, 0.72f, 1f};
    private static final float[] TV_ALPHAS = {0f, 0.7f, 0.92f};
    private static final float[] SOLID_ALPHAS = {1f, 1f, 1f};
    /** The fade under the tab bar: where it lands on the canvas, on the design's stops. */
    static final float[] FOOT_STOPS = {0f, 0.42f, 0.72f, 1f};
    static final float[] FOOT_ALPHAS = {0f, 0.62f, 0.92f, 1f};

    private final Activity activity;
    private final GlassForm form;
    private final GlassHost host;
    private final RecyclerView list;
    private final View topRow;
    private final View bottomRow;
    private final View pc;
    private final int listInset;
    private GlassProbe probe;
    private HomeSearch search;

    HomeChrome(Activity activity, RecyclerView list, boolean shelves) {
        this.activity = activity;
        this.list = list;
        android.content.res.Resources res = activity.getResources();
        form = GlassForm.of(activity);
        listInset = listInset(res, form, shelves);
        if (form == GlassForm.TELEVISION) {
            // Home's content doesn't clip its children, for the focus rings: the list does, at its own top, so scrolled
            // shelves never draw over the hero and the top row.
            list.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) ->
                    list.setClipBounds(new android.graphics.Rect(0, 0, r - l, b - t)));
        }
        host = activity.findViewById(R.id.glass_host);
        topRow = activity.findViewById(R.id.home_top_row);
        bottomRow = activity.findViewById(R.id.home_bottom_row);
        pc = activity.findViewById(R.id.home_pc);
        GlassTier tier = GlassTier.land(activity);
        host.setTier(tier);
        com.limelight.LimeLog.info("Glass tier: " + tier);
        placeTabs();
        fadeFoot();
        host.setSafeAreaListener(this::onSafeArea);
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.N) {
            probe = GlassProbe.start(activity, tier, this::stepDown);
        }
    }

    /** The probe failed: run the lower tier now, and if that one is live too, probe it in turn. */
    @androidx.annotation.RequiresApi(android.os.Build.VERSION_CODES.N)
    private void stepDown(GlassTier lower) {
        host.setTier(lower);
        probe = GlassProbe.continueAt(activity, lower, this::stepDown);
    }

    /**
     * The list's side padding. Shelves keep their own; a TV's grid and list stand back to the overscan edge, less the
     * inset each card already has, so headers, covers and wide cards line up with the shelves and the hero.
     */
    static int listInset(android.content.res.Resources res, GlassForm form, boolean shelves) {
        if (shelves) return 0;
        if (form == GlassForm.TELEVISION) {
            return Math.max(0, res.getDimensionPixelSize(R.dimen.shell_overscan_horizontal) - res.getDimensionPixelSize(R.dimen.cover_inset));
        }
        return res.getDimensionPixelSize(R.dimen.home_list_inset);
    }

    GlassForm form() {
        return form;
    }

    /** The search's results keep to the list's padding. */
    void setSearch(HomeSearch search) {
        this.search = search;
    }

    /** The backdrop bakes for this look and fades to the canvas where this layout's lists begin. */
    void setUpBackdrop(BackdropView backdrop, int canvas) {
        setUpBackdrop(activity, host, form, backdrop, canvas);
    }

    /** The ambient's look on this form factor, and the glass tinted from it. */
    static void setUpBackdrop(Activity activity, com.limelight.ui.GlassHost host, GlassForm form, BackdropView backdrop, int canvas) {
        AppTheme.Look look = AppTheme.look(activity);
        backdrop.setBake(look == AppTheme.Look.DAY, form == GlassForm.PHONE);
        float[] stops = form == GlassForm.PHONE ? PHONE_STOPS : form == GlassForm.HANDHELD ? HANDHELD_STOPS : TV_STOPS;
        float[] alphas = form == GlassForm.PHONE ? PHONE_ALPHAS : form == GlassForm.HANDHELD ? HANDHELD_ALPHAS : TV_ALPHAS;
        backdrop.setFade(canvas, stops, look == AppTheme.Look.HIGH_CONTRAST ? SOLID_ALPHAS : alphas);
        int fadeMs = com.limelight.ui.Motion.reduced(activity) ? 0
                : activity.getResources().getInteger(R.integer.nj_motion_ambient_ms);
        backdrop.setAmbientListener((bitmap, animate) -> host.setAmbient(bitmap, animate, fadeMs));
    }

    /** The PC capsule: its host's name and whether it answers; hidden until there is a host to name. */
    void showHost(HomeModel.HostCard card, View.OnClickListener open) {
        showHost(pc, card, open);
    }

    /** Any screen's PC capsule ({@code home_pc}, with its {@code home_pc_name} and {@code home_pc_dot}), as on Home. */
    static void showHost(View pc, HomeModel.HostCard card, View.OnClickListener open) {
        if (card == null) {
            pc.setVisibility(View.GONE);
            return;
        }
        ((TextView) pc.findViewById(R.id.home_pc_name)).setText(card.name);
        GradientDrawable dot = new GradientDrawable();
        dot.setShape(GradientDrawable.OVAL);
        dot.setColor(card.online ? MaterialColors.getColor(pc, R.attr.glassOk)
                : ResourcesCompat.getColor(pc.getResources(), R.color.nj_color_label_secondary, null));
        pc.findViewById(R.id.home_pc_dot).setBackground(dot);
        pc.setContentDescription(card.name);
        pc.setOnClickListener(open);
        pc.setVisibility(View.VISIBLE);
    }

    void destroy() {
        if (probe != null && android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.N) probe.stop();
    }

    private void placeTabs() {
        View tabs = activity.findViewById(R.id.home_tabs);
        if (form == GlassForm.HANDHELD) {
            ViewGroup row = (ViewGroup) tabs.getParent();
            row.removeView(tabs);
            int capsule = activity.getResources().getDimensionPixelSize(R.dimen.glass_capsule);
            ((ViewGroup) topRow).addView(tabs, 0, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, capsule));
            ((LinearLayout.LayoutParams) tabs.getLayoutParams()).setMarginEnd(activity.getResources().getDimensionPixelSize(R.dimen.space_s));
            bottomRow.setVisibility(View.GONE);
            activity.findViewById(R.id.home_foot_fade).setVisibility(View.GONE);
            activity.findViewById(R.id.home_search_top).setVisibility(View.VISIBLE);
            // The tabs at the start, the PC capsule at the end with search beside it.
            ((LinearLayout.LayoutParams) activity.findViewById(R.id.home_top_spacer_start).getLayoutParams()).weight = 1f;
            ((LinearLayout.LayoutParams) activity.findViewById(R.id.home_top_spacer_end).getLayoutParams()).weight = 0f;
        }
        styleTabs(activity, form);
        keepRingUnclipped(activity.findViewById(R.id.shell_bar), activity.findViewById(R.id.shell_bar_items));
    }

    /**
     * A focused tab's ring stands past the tab and the strip the tabs scroll in. It may leave that strip as long as the tabs
     * fit, which is every window but a very narrow one; tabs that have to scroll stay inside it instead.
     */
    static void keepRingUnclipped(View bar, View items) {
        if (!(bar instanceof ViewGroup) || items == null) return;
        ViewGroup strip = (ViewGroup) bar;
        strip.setClipChildren(false);
        strip.setClipToPadding(false);
        items.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> fitRing(strip, r - l));
    }

    static void fitRing(ViewGroup strip, int itemsWidth) {
        strip.setClipChildren(itemsWidth > strip.getWidth());
    }

    /** The bar's buttons: icon over word in equal shares on a phone, icon beside word on the top capsule. */
    static void styleTabs(Activity activity, GlassForm form) {
        View bar = activity.findViewById(R.id.shell_bar);
        bar.setPadding(0, 0, 0, 0);
        ViewGroup items = activity.findViewById(R.id.shell_bar_items);
        items.getLayoutParams().height = ViewGroup.LayoutParams.MATCH_PARENT;
        boolean phone = form == GlassForm.PHONE;
        float density = items.getResources().getDisplayMetrics().density;
        for (int i = 0; i < items.getChildCount(); i++) {
            MaterialButton tab = (MaterialButton) items.getChildAt(i);
            LinearLayout.LayoutParams params = phone
                    ? new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.MATCH_PARENT, 1f)
                    : new LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.MATCH_PARENT);
            tab.setLayoutParams(params);
            tab.setInsetTop(0);
            tab.setInsetBottom(0);
            tab.setMinimumHeight(0);
            tab.setMinimumWidth(0);
            tab.setMinWidth(0);
            tab.setCornerRadius(Math.round(999 * density));
            tab.setStrokeWidth(0);
            tab.setBackgroundTintList(ColorStateList.valueOf(0));
            tab.setIconTint(tab.getTextColors());
            tab.setTextSize(android.util.TypedValue.COMPLEX_UNIT_SP, phone ? 11f : 13f);
            if (phone) {
                tab.setIconGravity(MaterialButton.ICON_GRAVITY_TEXT_TOP);
                tab.setIconPadding(Math.round(2 * density));
                tab.setIconSize(Math.round(24 * density));
                tab.setPadding(0, 0, 0, 0);
            } else {
                tab.setIconGravity(MaterialButton.ICON_GRAVITY_TEXT_START);
                tab.setIconPadding(Math.round(6 * density));
                tab.setIconSize(Math.round(18 * density));
                int side = Math.round(14 * density);
                tab.setPadding(side, 0, side + Math.round(2 * density), 0);
            }
        }
    }

    private void fadeFoot() {
        View fade = activity.findViewById(R.id.home_foot_fade);
        if (fade == null || fade.getVisibility() != View.VISIBLE) return;
        int canvas = MaterialColors.getColor(fade, R.attr.glassCanvas);
        int[] colors = footColors(canvas);
        android.graphics.drawable.ShapeDrawable gradient = new android.graphics.drawable.ShapeDrawable(new android.graphics.drawable.shapes.RectShape());
        gradient.setShaderFactory(new android.graphics.drawable.ShapeDrawable.ShaderFactory() {
            @Override public android.graphics.Shader resize(int width, int height) {
                return new android.graphics.LinearGradient(0, 0, 0, height, colors, FOOT_STOPS, android.graphics.Shader.TileMode.CLAMP);
            }
        });
        fade.setBackground(gradient);
    }

    static int[] footColors(int canvas) {
        int[] colors = new int[FOOT_ALPHAS.length];
        for (int i = 0; i < colors.length; i++) colors[i] = ColorUtils.setAlphaComponent(canvas, Math.round(255 * FOOT_ALPHAS[i]));
        return colors;
    }

    /** The lists run edge to edge under the chrome; their padding and the chrome keep the safe area. */
    private void onSafeArea(int left, int top, int right, int bottom) {
        android.content.res.Resources res = activity.getResources();
        int edge = res.getDimensionPixelSize(R.dimen.glass_edge);
        int capsule = res.getDimensionPixelSize(R.dimen.glass_capsule);
        int gap = res.getDimensionPixelSize(R.dimen.space_s);
        if (form == GlassForm.TELEVISION) {
            topRow.setPadding(left, top, right, 0);
            list.setPadding(left + listInset, list.getPaddingTop(), right + listInset, list.getPaddingBottom());
            ViewGroup.MarginLayoutParams row = (ViewGroup.MarginLayoutParams) topRow.getLayoutParams();
            placeSearch(left, right, top + row.topMargin, capsule, gap, bottom + res.getDimensionPixelSize(R.dimen.space_xl), row.getMarginStart());
            return;
        }
        boolean handheld = form == GlassForm.HANDHELD;
        int rowTop = top + res.getDimensionPixelSize(handheld ? R.dimen.space_m : R.dimen.space_s);
        topRow.setPadding(left + (handheld ? res.getDimensionPixelSize(R.dimen.space_xl) : edge), rowTop,
                right + (handheld ? res.getDimensionPixelSize(R.dimen.space_xl) : edge), 0);
        ViewGroup.MarginLayoutParams loading = (ViewGroup.MarginLayoutParams) activity.findViewById(R.id.console_loading).getLayoutParams();
        loading.topMargin = top;
        android.widget.FrameLayout.LayoutParams banner = (android.widget.FrameLayout.LayoutParams) activity.findViewById(R.id.home_pairing_banner).getLayoutParams();
        if (handheld) {
            banner.topMargin = rowTop + capsule + gap;
        } else {
            // A phone's hero fills the width under the top row: over it, the banner would cover the art. It rests on the bar.
            banner.gravity = android.view.Gravity.BOTTOM | android.view.Gravity.CENTER_HORIZONTAL;
            banner.bottomMargin = bottom + res.getDimensionPixelSize(R.dimen.nj_size_tab_bar_height) + 2 * gap;
            restBannerOnHints(gap);
        }
        bottomRow.setPadding(left + res.getDimensionPixelSize(R.dimen.space_m), 0, right + res.getDimensionPixelSize(R.dimen.space_m),
                bottom + res.getDimensionPixelSize(R.dimen.space_s));
        int foot = handheld ? bottom + res.getDimensionPixelSize(R.dimen.space_xl)
                : bottom + res.getDimensionPixelSize(R.dimen.nj_size_tab_bar_height) + 3 * res.getDimensionPixelSize(R.dimen.space_m);
        list.setPadding(left + listInset, rowTop + capsule + gap, right + listInset, foot);
        // Scrolled, the list fades out under the top row: a bright button never slides whole under the PC capsule.
        if (list instanceof HomeList) {
            list.setVerticalFadingEdgeEnabled(true);
            list.setFadingEdgeLength(rowTop + capsule + gap);
        }
        View fade = activity.findViewById(R.id.home_foot_fade);
        if (fade != null) {
            // The bar floats above the safe area, and its 136 dp stand above that too.
            ViewGroup.LayoutParams params = fade.getLayoutParams();
            params.height = res.getDimensionPixelSize(R.dimen.home_foot_fade) + bottom;
            fade.setLayoutParams(params);
        }
        placeSearch(left, right, rowTop, capsule, gap, foot, res.getDimensionPixelSize(handheld ? R.dimen.space_xl : R.dimen.glass_edge));
        placeHints(rowTop, capsule, bottom, handheld);
    }

    private void placeSearch(int left, int right, int rowTop, int capsule, int gap, int foot, int side) {
        android.content.res.Resources res = activity.getResources();
        ViewGroup.MarginLayoutParams bar = (ViewGroup.MarginLayoutParams) activity.findViewById(R.id.home_search_bar).getLayoutParams();
        bar.setMargins(left + side, rowTop, right + side, 0);
        if (search == null) return;
        // The covers line up under the field: each card stands in its slot by this much on every side.
        int inset = res.getDimensionPixelSize(R.dimen.space_s);
        search.place(left + side - inset, rowTop + capsule, gap, right + side - inset, foot);
    }

    private boolean bannerRestsOnHints;

    /** A pad's hints come and go in the same place, above the bar: the banner stands on them while they show. */
    private void restBannerOnHints(int gap) {
        if (bannerRestsOnHints) return;
        bannerRestsOnHints = true;
        View hints = activity.findViewById(R.id.home_hints);
        com.limelight.ui.GlassButton banner = activity.findViewById(R.id.home_pairing_banner);
        banner.setOverArt();
        hints.getViewTreeObserver().addOnGlobalLayoutListener(() ->
                banner.setTranslationY(hints.getVisibility() == View.VISIBLE ? -(hints.getHeight() + gap) : 0));
    }

    /** On a handheld the hints sit in the top row's open middle; on a phone just above the tab bar, on a chip over the art. */
    private void placeHints(int rowTop, int capsule, int bottom, boolean handheld) {
        android.content.res.Resources res = activity.getResources();
        TextView hints = activity.findViewById(R.id.home_hints);
        android.widget.FrameLayout.LayoutParams params = (android.widget.FrameLayout.LayoutParams) hints.getLayoutParams();
        params.gravity = (handheld ? android.view.Gravity.TOP : android.view.Gravity.BOTTOM) | android.view.Gravity.CENTER_HORIZONTAL;
        if (handheld) {
            params.height = capsule;
            params.topMargin = rowTop;
        } else {
            params.bottomMargin = bottom + res.getDimensionPixelSize(R.dimen.space_s) + res.getDimensionPixelSize(R.dimen.nj_size_tab_bar_height)
                    + res.getDimensionPixelSize(R.dimen.space_s);
            hints.setBackgroundResource(R.drawable.glass_chip_over_art);
        }
        hints.setLayoutParams(params);
    }

    /** For tests: which tier the host runs. */
    GlassTier tier() {
        return host.tier();
    }
}
