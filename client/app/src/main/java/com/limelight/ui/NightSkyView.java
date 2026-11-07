package com.limelight.ui;

import android.content.Context;
import android.content.res.Resources;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.LinearGradient;
import android.graphics.Paint;
import android.graphics.RadialGradient;
import android.graphics.Shader;
import android.util.AttributeSet;
import android.view.View;

import androidx.core.content.res.ResourcesCompat;

import com.limelight.R;

/**
 * The PCs page's sky: the gradient, the moon with its glow and orbit rings, and a few stars, all from the design tokens. Day gets
 * a low sun with no stars, OLED a black sky, and high contrast only the moon. It draws once for each size and never animates, so
 * an idle page repaints nothing.
 */
public class NightSkyView extends View {
    private static final int STARS = 26;
    private static final float STAR_FIELD_DP = 118f;
    private static final float[] RING_AT = {1.7f, 2.5f, 3.3f, 4.1f};
    private static final float GLOW_REACH = 6f;
    private static final int BAKED_WIDTH = 64;
    private static final int BAKED_HEIGHT = 96;

    private final Paint fill = new Paint(Paint.DITHER_FLAG);
    private final Paint glow = new Paint(Paint.ANTI_ALIAS_FLAG | Paint.DITHER_FLAG);
    private final Paint ring = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint moon = new Paint(Paint.ANTI_ALIAS_FLAG | Paint.DITHER_FLAG);
    private final Paint star = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final float[][] stars = new float[STARS][4];
    private final AppTheme.Look look;
    private final GlassForm form;
    private final float density;
    private int shadedW;
    private int shadedH;
    private float moonX;
    private float moonY;
    private float moonR;
    private int safeTop;

    public NightSkyView(Context context) {
        this(context, null);
    }

    public NightSkyView(Context context, AttributeSet attrs) {
        super(context, attrs);
        look = AppTheme.look(context);
        form = GlassForm.of(context);
        density = context.getResources().getDisplayMetrics().density;
        ring.setStyle(Paint.Style.STROKE);
        ring.setStrokeWidth(Math.max(1f, density * 0.75f));
        ring.setColor(color(context, look == AppTheme.Look.DAY ? R.color.nj_day_sky_ring : R.color.nj_sky_ring));
        star.setColor(0xFFFFFFFF);
        seedStars();
        setWillNotDraw(false);
    }

    private static int color(Context context, int id) {
        return ResourcesCompat.getColor(context.getResources(), id, null);
    }

    /** The flat fill of a PC's card or group on this look's sky. */
    public static int groupFill(Context context) {
        switch (AppTheme.look(context)) {
            case DAY:
                return color(context, R.color.nj_day_tint_group_on_sky_fill);
            case OLED:
                return color(context, R.color.nj_oled_tint_group_on_sky_fill);
            case HIGH_CONTRAST:
                return color(context, R.color.nj_solid_group_fill);
            default:
                return color(context, R.color.nj_tint_group_on_sky_fill);
        }
    }

    /** The lines between a group's rows on this look. */
    public static int separator(Context context) {
        return AppTheme.look(context) == AppTheme.Look.DAY ? color(context, R.color.nj_day_tint_separator) : color(context, R.color.nj_tint_separator);
    }

    /** The moon's own ink, for an action that is not filled with it. */
    public static int moonInk(Context context) {
        switch (AppTheme.look(context)) {
            case DAY:
                return color(context, R.color.nj_day_color_moon_ink);
            case HIGH_CONTRAST:
                return color(context, R.color.nj_hc_color_moon);
            default:
                return color(context, R.color.nj_color_moon_ink);
        }
    }

    /** The design's star field: the same seeded generator as the prototype, so the same stars on every run. */
    private void seedStars() {
        long seed = 7;
        for (int i = 0; i < STARS; i++) {
            for (int part = 0; part < 4; part++) {
                seed = seed * 16807 % 2147483647L;
                stars[i][part] = (seed - 1) / 2147483646f;
            }
        }
    }

    /** How far down the window's safe area starts: the moon keeps its place under a status bar or a notch. */
    public void setSafeTop(int top) {
        if (safeTop == top) return;
        safeTop = top;
        shadedW = 0;
        invalidate();
    }

    /** Where the moon stands in a window of this size. */
    private void placeMoon(int w, int h) {
        switch (form) {
            case TELEVISION:
                // Drawn for 720dp; a 1080p TV at 320dpi has 540dp.
                float scale = Math.min(1f, h / (720 * density));
                moonX = w * .8125f;
                moonY = 150 * density * scale;
                moonR = 86 * density * scale;
                break;
            case HANDHELD:
                moonX = w * .79f;
                moonY = 92 * density;
                moonR = 58 * density;
                break;
            default:
                moonX = w * .563f;
                moonY = safeTop + 60 * density;
                moonR = 40 * density;
                break;
        }
    }

    private void shade(int w, int h) {
        Context context = getContext();
        placeMoon(w, h);
        boolean day = look == AppTheme.Look.DAY;
        boolean black = look == AppTheme.Look.OLED || look == AppTheme.Look.HIGH_CONTRAST;
        if (black) {
            fill.setShader(null);
            fill.setColor(0xFF000000);
        } else {
            fill.setShader(new LinearGradient(0, 0, 0, h,
                    new int[] {color(context, day ? R.color.nj_day_sky_top : R.color.nj_sky_top),
                            color(context, day ? R.color.nj_day_sky_mid : R.color.nj_sky_mid),
                            color(context, day ? R.color.nj_day_sky_bottom : R.color.nj_sky_bottom)},
                    new float[] {0f, .46f, 1f}, Shader.TileMode.CLAMP));
        }
        if (look != AppTheme.Look.HIGH_CONTRAST) {
            int full = color(context, day ? R.color.nj_day_sky_glow : R.color.nj_sky_glow);
            int part = (Math.round((full >>> 24) * .36f) << 24) | (full & 0xFFFFFF);
            glow.setShader(new RadialGradient(moonX, moonY, moonR * GLOW_REACH,
                    new int[] {full, part, full & 0xFFFFFF}, new float[] {0f, .25f, 1f}, Shader.TileMode.CLAMP));
        }
        if (look == AppTheme.Look.HIGH_CONTRAST) {
            moon.setShader(null);
            moon.setColor(color(context, R.color.nj_hc_color_moon));
        } else {
            moon.setShader(new LinearGradient(0, moonY - moonR, 0, moonY + moonR, 0xFFFFD27A, 0xFFEE9F2E, Shader.TileMode.CLAMP));
        }
    }

    @Override
    protected void onDraw(Canvas canvas) {
        int w = getWidth();
        int h = getHeight();
        if (w != shadedW || h != shadedH) {
            shadedW = w;
            shadedH = h;
            shade(w, h);
        }
        paint(canvas, w, h);
    }

    private void paint(Canvas canvas, int w, int h) {
        canvas.drawRect(0, 0, w, h, fill);
        if (look == AppTheme.Look.HIGH_CONTRAST) {
            canvas.drawCircle(moonX, moonY, moonR, moon);
            return;
        }
        canvas.drawRect(0, 0, w, h, glow);
        if (look != AppTheme.Look.DAY) {
            float field = STAR_FIELD_DP * density * (form == GlassForm.PHONE ? 1f : 1.5f);
            for (float[] s : stars) {
                star.setAlpha(Math.round((.12f + s[3] * .5f) * 255f));
                canvas.drawCircle(s[0] * w, s[1] * field, (.4f + s[2] * .9f) * density, star);
            }
        }
        for (float at : RING_AT) canvas.drawCircle(moonX, moonY, moonR * at, ring);
        canvas.drawCircle(moonX, moonY, moonR, moon);
    }

    /** A small picture of this sky for the glass of the baked tier, which samples a bitmap instead of the page. */
    public Bitmap bake(int windowW, int windowH) {
        if (windowW <= 0 || windowH <= 0) return null;
        shade(windowW, windowH);
        shadedW = windowW;
        shadedH = windowH;
        Bitmap made = Bitmap.createBitmap(BAKED_WIDTH, BAKED_HEIGHT, Bitmap.Config.ARGB_8888);
        Canvas canvas = new Canvas(made);
        canvas.scale(BAKED_WIDTH / (float) windowW, BAKED_HEIGHT / (float) windowH);
        paint(canvas, windowW, windowH);
        return made;
    }
}
