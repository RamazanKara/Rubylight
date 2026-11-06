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

    private final Activity activity;
    private final GlassForm form;
    private final GlassHost host;
    private final RecyclerView list;
    private final View topRow;
    private final View bottomRow;
    private final View pc;
    private final TextView pcName;
    private final View pcDot;
    private final int listInset;
    private GlassProbe probe;

    HomeChrome(Activity activity, RecyclerView list, boolean shelves) {
        this.activity = activity;
        this.list = list;
        listInset = shelves ? 0 : activity.getResources().getDimensionPixelSize(R.dimen.home_list_inset);
        form = GlassForm.of(activity);
        host = activity.findViewById(R.id.glass_host);
        topRow = activity.findViewById(R.id.home_top_row);
        bottomRow = activity.findViewById(R.id.home_bottom_row);
        pc = activity.findViewById(R.id.home_pc);
        pcName = activity.findViewById(R.id.home_pc_name);
        pcDot = activity.findViewById(R.id.home_pc_dot);
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
    private void stepDown(GlassTier lower) {
        host.setTier(lower);
        probe = GlassProbe.continueAt(activity, lower, this::stepDown);
    }

    GlassForm form() {
        return form;
    }

    /** The backdrop bakes for this look and fades to the canvas where this layout's lists begin. */
    void setUpBackdrop(BackdropView backdrop, int canvas) {
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
        if (card == null) {
            pc.setVisibility(View.GONE);
            return;
        }
        pcName.setText(card.name);
        GradientDrawable dot = new GradientDrawable();
        dot.setShape(GradientDrawable.OVAL);
        dot.setColor(card.online ? MaterialColors.getColor(pc, R.attr.glassOk)
                : ResourcesCompat.getColor(activity.getResources(), R.color.nj_color_label_secondary, null));
        pcDot.setBackground(dot);
        pc.setContentDescription(card.name);
        pc.setOnClickListener(open);
        pc.setVisibility(View.VISIBLE);
    }

    void destroy() {
        if (probe != null) probe.stop();
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
        }
        styleTabs(activity, form);
    }

    /** The bar's buttons: icon over word in equal shares on a phone, icon beside word on the top capsule. */
    static void styleTabs(Activity activity, GlassForm form) {
        View bar = activity.findViewById(R.id.shell_bar);
        bar.setPadding(0, 0, 0, 0);
        ViewGroup items = activity.findViewById(R.id.shell_bar_items);
        items.getLayoutParams().height = ViewGroup.LayoutParams.MATCH_PARENT;
        boolean phone = form == GlassForm.PHONE;
        float density = activity.getResources().getDisplayMetrics().density;
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
        fade.setBackground(new GradientDrawable(GradientDrawable.Orientation.TOP_BOTTOM, new int[] {
                ColorUtils.setAlphaComponent(canvas, 0),
                ColorUtils.setAlphaComponent(canvas, 158),
                ColorUtils.setAlphaComponent(canvas, 235),
                canvas}));
    }

    /** The lists run edge to edge under the chrome; their padding and the chrome keep the safe area. */
    private void onSafeArea(int left, int top, int right, int bottom) {
        android.content.res.Resources res = activity.getResources();
        int edge = res.getDimensionPixelSize(R.dimen.glass_edge);
        int capsule = res.getDimensionPixelSize(R.dimen.glass_capsule);
        int gap = res.getDimensionPixelSize(R.dimen.space_s);
        if (form == GlassForm.TELEVISION) {
            topRow.setPadding(left, top, right, 0);
            list.setPadding(left + list.getPaddingLeft(), list.getPaddingTop(), right + list.getPaddingRight(), list.getPaddingBottom());
            return;
        }
        boolean handheld = form == GlassForm.HANDHELD;
        int rowTop = top + res.getDimensionPixelSize(handheld ? R.dimen.space_m : R.dimen.space_s);
        topRow.setPadding(left + (handheld ? res.getDimensionPixelSize(R.dimen.space_xl) : edge), rowTop,
                right + (handheld ? res.getDimensionPixelSize(R.dimen.space_xl) : edge), 0);
        ViewGroup.MarginLayoutParams loading = (ViewGroup.MarginLayoutParams) activity.findViewById(R.id.console_loading).getLayoutParams();
        loading.topMargin = top;
        ViewGroup.MarginLayoutParams banner = (ViewGroup.MarginLayoutParams) activity.findViewById(R.id.home_pairing_banner).getLayoutParams();
        banner.topMargin = rowTop + capsule + gap;
        bottomRow.setPadding(left + res.getDimensionPixelSize(R.dimen.space_m), 0, right + res.getDimensionPixelSize(R.dimen.space_m),
                bottom + res.getDimensionPixelSize(R.dimen.space_s));
        int foot = handheld ? bottom + res.getDimensionPixelSize(R.dimen.space_xl)
                : bottom + res.getDimensionPixelSize(R.dimen.nj_size_tab_bar_height) + 3 * res.getDimensionPixelSize(R.dimen.space_m);
        list.setPadding(left + listInset, rowTop + capsule + gap, right + listInset, foot);
    }

    /** For tests: which tier the host runs. */
    GlassTier tier() {
        return host.tier();
    }
}
