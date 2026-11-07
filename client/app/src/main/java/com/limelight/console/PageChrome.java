package com.limelight.console;

import android.app.Activity;
import android.content.res.Resources;
import android.view.View;
import android.view.ViewGroup;
import android.widget.LinearLayout;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;
import com.limelight.ui.GlassForm;
import com.limelight.ui.GlassHost;
import com.limelight.ui.GlassProbe;
import com.limelight.ui.GlassTier;

/**
 * The glass around a list page (PCs, a PC's games), as on Home: the tier and its probe, and the tabs placed for the form factor.
 * A phone keeps them in a bar at the foot; a handheld and a TV carry them at the start of the top row.
 */
public final class PageChrome {
    /** The page's content takes its place from this, as the chrome leaves it. */
    public interface Layout {
        void onSafeArea(int left, int top, int right, int bottom);
    }

    private final Activity activity;
    private final GlassHost host;
    private final GlassForm form;
    private GlassProbe probe;

    /**
     * @param tabsId the capsule around the shell bar
     * @param topRowId the row that takes the capsule on a handheld and a TV
     * @param bottomRowId the bar at the foot on a phone
     * @param fadeId the fade under that bar
     */
    public PageChrome(Activity activity, GlassHost host, int tabsId, int topRowId, int bottomRowId, int fadeId, Layout layout) {
        this.activity = activity;
        this.host = host;
        this.form = GlassForm.of(activity);
        GlassTier tier = GlassTier.land(activity);
        host.setTier(tier);
        placeTabs(tabsId, topRowId, bottomRowId, fadeId);
        host.setSafeAreaListener(layout::onSafeArea);
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.N) {
            probe = GlassProbe.start(activity, tier, this::stepDown);
        }
    }

    @androidx.annotation.RequiresApi(android.os.Build.VERSION_CODES.N)
    private void stepDown(GlassTier lower) {
        host.setTier(lower);
        probe = GlassProbe.continueAt(activity, lower, this::stepDown);
    }

    public GlassForm form() {
        return form;
    }

    public void stop() {
        if (probe != null && android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.N) probe.stop();
    }

    private void placeTabs(int tabsId, int topRowId, int bottomRowId, int fadeId) {
        View tabs = activity.findViewById(tabsId);
        if (form != GlassForm.PHONE) {
            Resources res = activity.getResources();
            ((ViewGroup) tabs.getParent()).removeView(tabs);
            int capsule = res.getDimensionPixelSize(R.dimen.glass_capsule);
            ((ViewGroup) activity.findViewById(topRowId)).addView(tabs, 0, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, capsule));
            activity.findViewById(bottomRowId).setVisibility(View.GONE);
            activity.findViewById(fadeId).setVisibility(View.GONE);
        } else {
            View fade = activity.findViewById(fadeId);
            fade.setBackground(footFade(MaterialColors.getColor(fade, R.attr.glassCanvas)));
        }
        HomeChrome.styleTabs(activity, form);
        HomeChrome.keepRingUnclipped(activity.findViewById(R.id.shell_bar), activity.findViewById(R.id.shell_bar_items));
    }

    private static android.graphics.drawable.Drawable footFade(int canvas) {
        int[] colors = HomeChrome.footColors(canvas);
        android.graphics.drawable.ShapeDrawable gradient = new android.graphics.drawable.ShapeDrawable(new android.graphics.drawable.shapes.RectShape());
        gradient.setShaderFactory(new android.graphics.drawable.ShapeDrawable.ShaderFactory() {
            @Override public android.graphics.Shader resize(int width, int height) {
                return new android.graphics.LinearGradient(0, 0, 0, height, colors, HomeChrome.FOOT_STOPS, android.graphics.Shader.TileMode.CLAMP);
            }
        });
        return gradient;
    }
}
