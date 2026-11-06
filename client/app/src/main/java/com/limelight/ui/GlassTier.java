package com.limelight.ui;

import android.app.ActivityManager;
import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.os.PowerManager;

import com.limelight.BuildConfig;

/**
 * How real a glass surface can afford to be on this device, chosen by capability and by a frame probe, never by the
 * device's name. {@link #decide} is the rule; what the probe found is kept per app version in its own small file.
 */
public enum GlassTier {
    /** Live blur plus the refracted edge band (API 33+). */
    LENS,
    /** Live blur (API 31+). */
    LIVE,
    /** The static ambient, sampled in screen space under the fill: a complete design at no GPU cost. */
    BAKED,
    /** High contrast: an opaque fill and a light rim. */
    SOLID;

    private static final String FILE = "glass_tier";
    private static final String OVERRIDE = "debug_override";

    /** One tier down, as a failed frame probe steps. */
    public GlassTier stepDown() {
        return this == LENS ? LIVE : BAKED;
    }

    /**
     * The tier from what is known now.
     *
     * @param stored what the probe left for this app version, or null before it has run
     */
    static GlassTier decide(int api, boolean lowRam, boolean powerSave, boolean hot, boolean highContrast, GlassTier stored) {
        if (highContrast) return SOLID;
        if (api < Build.VERSION_CODES.S || lowRam || powerSave || hot) return BAKED;
        GlassTier top = api >= Build.VERSION_CODES.TIRAMISU ? LENS : LIVE;
        if (stored == null) return top;
        return stored.ordinal() > top.ordinal() ? stored : top;
    }

    /** Where this device lands right now. Battery saver and heat are read each time and never stored. */
    public static GlassTier land(Context context) {
        Context app = context.getApplicationContext();
        SharedPreferences prefs = app.getSharedPreferences(FILE, Context.MODE_PRIVATE);
        if (BuildConfig.DEBUG) {
            GlassTier forced = parse(prefs.getString(OVERRIDE, null));
            if (forced != null) return forced;
        }
        ActivityManager activities = (ActivityManager) app.getSystemService(Context.ACTIVITY_SERVICE);
        PowerManager power = (PowerManager) app.getSystemService(Context.POWER_SERVICE);
        boolean hot = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && power != null
                && power.getCurrentThermalStatus() >= PowerManager.THERMAL_STATUS_SEVERE;
        return decide(Build.VERSION.SDK_INT, activities != null && activities.isLowRamDevice(),
                power != null && power.isPowerSaveMode(), hot, AppTheme.isHighContrast(context),
                parse(prefs.getString(versionKey(), null)));
    }

    /** Whether a probe's verdict for this app version is already kept. A forced tier is never probed. */
    public static boolean probed(Context context) {
        SharedPreferences prefs = context.getApplicationContext().getSharedPreferences(FILE, Context.MODE_PRIVATE);
        return prefs.contains(versionKey()) || (BuildConfig.DEBUG && prefs.contains(OVERRIDE));
    }

    /** Keeps the probe's verdict for this app version. */
    public static void remember(Context context, GlassTier tier) {
        context.getApplicationContext().getSharedPreferences(FILE, Context.MODE_PRIVATE).edit()
                .putString(versionKey(), tier.name()).apply();
    }

    /** Debug builds only: pins the tier, for the demo. Null clears it. */
    public static void force(Context context, GlassTier tier) {
        SharedPreferences.Editor editor = context.getApplicationContext()
                .getSharedPreferences(FILE, Context.MODE_PRIVATE).edit();
        if (tier == null) editor.remove(OVERRIDE);
        else editor.putString(OVERRIDE, tier.name());
        editor.apply();
    }

    static GlassTier parse(String name) {
        if (name == null) return null;
        for (GlassTier tier : values()) if (tier.name().equals(name)) return tier;
        return null;
    }

    private static String versionKey() {
        return "v" + BuildConfig.VERSION_CODE;
    }
}
