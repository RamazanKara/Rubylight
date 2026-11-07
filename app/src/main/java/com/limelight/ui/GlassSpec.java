package com.limelight.ui;

import android.content.Context;
import android.content.res.Resources;

import androidx.core.content.res.ResourcesCompat;

import java.util.HashMap;
import java.util.Map;

/** One glass recipe from the design tokens: {@code glass.<material>.<tier>.*}, or {@code day.glass.*} by day. */
final class GlassSpec {
    final float blur;
    final float saturate;
    final float brightness;
    final int fill;
    final int sheenTop;
    final float sheenEnd;
    final int rimTop;
    final int rimMid;
    final int rimBottom;
    final float rimWidth;
    /** A dark outline day glass needs on a white canvas; 0 at night. */
    final int hairline;
    final int shadowColor;
    final float shadowY;
    final float shadowBlur;
    final float lensWidth;
    final float lensBlur;
    final float lensSaturate;
    final float lensBrightness;

    private static final Map<String, GlassSpec> CACHE = new HashMap<>();

    /** Main thread. Each recipe is read once per density. */
    static GlassSpec of(Context context, String material, GlassTier tier, boolean day) {
        Resources res = context.getResources();
        String tierName = tier.name().toLowerCase(java.util.Locale.ROOT);
        // Solid exists at night only: high contrast is always black.
        boolean dayTokens = day && tier != GlassTier.SOLID;
        return of(context, (dayTokens ? "nj_day_glass_" : "nj_glass_") + material + "_" + tierName + "_", dayTokens);
    }

    /** The stream menu over a system blur of its own: that blur has no colour filter, so this recipe has its own fill. */
    static GlassSpec menuWindow(Context context) {
        return of(context, "nj_glass_menu_window_", false);
    }

    private static GlassSpec of(Context context, String prefix, boolean dayTokens) {
        Resources res = context.getResources();
        String key = prefix + res.getDisplayMetrics().density;
        GlassSpec spec = CACHE.get(key);
        if (spec == null) {
            spec = new GlassSpec(res, context.getPackageName(), prefix, dayTokens);
            CACHE.put(key, spec);
        }
        return spec;
    }

    private GlassSpec(Resources res, String pkg, String prefix, boolean day) {
        blur = dimen(res, pkg, prefix + "blur");
        saturate = number(res, pkg, prefix + "saturate");
        brightness = number(res, pkg, prefix + "brightness");
        fill = color(res, pkg, prefix + "fill");
        sheenTop = color(res, pkg, prefix + "sheen_top");
        sheenEnd = number(res, pkg, prefix + "sheen_end");
        rimTop = color(res, pkg, prefix + "rim_top");
        rimMid = color(res, pkg, prefix + "rim_mid");
        rimBottom = color(res, pkg, prefix + "rim_bottom");
        rimWidth = dimen(res, pkg, prefix + "rim_width");
        hairline = day ? color(res, pkg, prefix + "hairline") : 0;
        shadowColor = color(res, pkg, prefix + "shadow_color");
        shadowY = dimen(res, pkg, prefix + "shadow_y");
        shadowBlur = dimen(res, pkg, prefix + "shadow_blur");
        lensWidth = dimen(res, pkg, prefix + "lens_width");
        lensBlur = dimen(res, pkg, prefix + "lens_blur");
        lensSaturate = number(res, pkg, prefix + "lens_saturate");
        lensBrightness = number(res, pkg, prefix + "lens_brightness");
    }

    private static float dimen(Resources res, String pkg, String name) {
        return res.getDimension(id(res, pkg, name, "dimen"));
    }

    private static int color(Resources res, String pkg, String name) {
        return ResourcesCompat.getColor(res, id(res, pkg, name, "color"), null);
    }

    private static float number(Resources res, String pkg, String name) {
        return ResourcesCompat.getFloat(res, id(res, pkg, name, "dimen"));
    }

    private static int id(Resources res, String pkg, String name, String type) {
        int id = res.getIdentifier(name, type, pkg);
        if (id == 0) throw new Resources.NotFoundException(name);
        return id;
    }
}
