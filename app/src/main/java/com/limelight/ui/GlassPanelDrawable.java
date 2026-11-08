package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.ColorFilter;
import android.graphics.LinearGradient;
import android.graphics.Outline;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.PixelFormat;
import android.graphics.Rect;
import android.graphics.RectF;
import android.graphics.Shader;
import android.graphics.drawable.Drawable;

import androidx.core.content.res.ResourcesCompat;
import androidx.core.graphics.ColorUtils;

import com.limelight.R;

/**
 * The glass panel of a window of its own, a dialog or a sheet: the design's panel recipe over the page's canvas. There is no
 * backdrop to sample from another window, so the fill stands on the canvas, near-opaque, and the rim and sheen say glass.
 * Draws nothing per frame beyond its own three shapes: no blur, no layer.
 */
public final class GlassPanelDrawable extends Drawable {
    private GlassSpec spec;
    private final float[] radii;
    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint sheen = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint rim = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint hairline = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Path shape = new Path();
    private final Path rimShape = new Path();
    // Set only for the stream menu, whose recipe can change while it is open.
    private Context menuContext;
    // Over a stream: the menu's recipes, or the HUD's.
    private boolean overStream;
    private int baseFill;
    /** The user's opacity for the fill, or below 0 for the recipe's own. */
    private float fillOpacity = -1f;

    private GlassPanelDrawable(Context context, float[] radii) {
        this.radii = radii;
        AppTheme.Look look = AppTheme.look(context);
        boolean day = isDay(context);
        spec = GlassSpec.of(context, "panel", AppTheme.isHighContrast(context) ? GlassTier.SOLID : GlassTier.BAKED, day);
        fill.setColor(fillOn(canvas(context, look), spec.fill));
        rim.setStyle(Paint.Style.STROKE);
        rim.setStrokeWidth(spec.rimWidth);
        hairline.setStyle(Paint.Style.STROKE);
        hairline.setStrokeWidth(Math.max(1f, context.getResources().getDisplayMetrics().density * 0.5f));
        hairline.setColor(spec.hairline);
    }

    private GlassPanelDrawable(Context context, float radius, boolean overBlur) {
        radii = new float[] {radius, radius, radius, radius, radius, radius, radius, radius};
        menuContext = context;
        overStream = true;
        useMenuRecipe(overBlur);
        rim.setStyle(Paint.Style.STROKE);
        hairline.setStyle(Paint.Style.STROKE);
    }

    private GlassPanelDrawable(Context context, float[] radii, GlassSpec spec) {
        this.radii = radii;
        this.spec = spec;
        overStream = true;
        setFill(spec.fill);
        rim.setStyle(Paint.Style.STROKE);
        rim.setStrokeWidth(spec.rimWidth);
        hairline.setStyle(Paint.Style.STROKE);
        hairline.setStrokeWidth(Math.max(1f, context.getResources().getDisplayMetrics().density * 0.5f));
        hairline.setColor(spec.hairline);
    }

    /**
     * What stays up over a stream while it plays: the statistics and the keyboards. See-through on every tier and never
     * over a blur, which would compose each stream frame on the GPU; high contrast is solid.
     */
    public static GlassPanelDrawable hud(Context context, float[] radii) {
        return new GlassPanelDrawable(context, radii, AppTheme.isHighContrast(context)
                ? GlassSpec.of(context, "menu", GlassTier.SOLID, false) : GlassSpec.hud(context));
    }

    /** A modal surface over the stream with no blur of its own, such as the controller editor's panel: the menu's near-opaque glass. */
    public static GlassPanelDrawable stream(Context context, float[] radii) {
        return new GlassPanelDrawable(context, radii, GlassSpec.of(context, "menu",
                AppTheme.isHighContrast(context) ? GlassTier.SOLID : GlassTier.BAKED, false));
    }

    /** Whether this is a surface over a stream rather than over a page. */
    boolean overStream() {
        return overStream;
    }

    /**
     * A user's opacity setting for the panel (0 to 1): the fill's own alpha, so 1 is opaque and the recipe's colour stays.
     * Only the fill: what stands on the panel keeps its own.
     */
    public void setFillOpacity(float opacity) {
        fillOpacity = Math.max(0f, Math.min(1f, opacity));
        setFill(baseFill);
        invalidateSelf();
    }

    private void setFill(int color) {
        baseFill = color;
        fill.setColor(fillOpacity < 0f ? color : ColorUtils.setAlphaComponent(color, Math.round(255 * fillOpacity)));
    }

    /**
     * The stream menu: night in every theme. Over a system blur it is the see-through window recipe, otherwise near-opaque over
     * the stream; high contrast is solid.
     */
    public static GlassPanelDrawable menu(Context context, boolean overBlur) {
        return new GlassPanelDrawable(context, context.getResources().getDimension(R.dimen.nj_radius_panel), overBlur);
    }

    /** The system turned its blur on or off while the menu is open. */
    void setOverBlur(boolean overBlur) {
        if (menuContext == null) return;
        useMenuRecipe(overBlur);
        onBoundsChange(getBounds());
        invalidateSelf();
    }

    private void useMenuRecipe(boolean overBlur) {
        spec = AppTheme.isHighContrast(menuContext) ? GlassSpec.of(menuContext, "menu", GlassTier.SOLID, false)
                : overBlur ? GlassSpec.menuWindow(menuContext) : GlassSpec.of(menuContext, "menu", GlassTier.BAKED, false);
        setFill(spec.fill);
        rim.setStrokeWidth(spec.rimWidth);
    }

    /** What the panel is filled with now. */
    int fillColor() {
        return fill.getColor();
    }

    /** All corners round: a dialog. */
    public static GlassPanelDrawable dialog(Context context) {
        float r = context.getResources().getDimension(R.dimen.nj_radius_panel);
        return new GlassPanelDrawable(context, new float[] {r, r, r, r, r, r, r, r});
    }

    /** The top corners round: a sheet along the bottom edge. */
    public static GlassPanelDrawable sheet(Context context) {
        float r = context.getResources().getDimension(R.dimen.nj_radius_sheet);
        return new GlassPanelDrawable(context, new float[] {r, r, r, r, 0, 0, 0, 0});
    }

    private static int canvas(Context context, AppTheme.Look look) {
        int id = isDay(context) ? R.color.nj_day_color_canvas : R.color.nj_color_canvas;
        return AppTheme.isOledBlack(context) ? 0xFF000000 : ResourcesCompat.getColor(context.getResources(), id, null);
    }

    /** By day only on a light theme: a stream stays night in every theme, and its dialogs with it. */
    static boolean isDay(Context context) {
        android.util.TypedValue light = new android.util.TypedValue();
        return AppTheme.look(context) == AppTheme.Look.DAY
                && context.getTheme().resolveAttribute(androidx.appcompat.R.attr.isLightTheme, light, true) && light.data != 0;
    }

    /** The material's fill, which is translucent over a backdrop, as it stands on a plain canvas. */
    static int fillOn(int canvas, int materialFill) {
        return ColorUtils.compositeColors(materialFill, canvas);
    }

    /**
     * How dark the page behind a centred dialog goes. A TV's night page goes deeper than under a sheet: the dialog stands in
     * the middle of a page seen from across the room, and the title and cards it covers must read as behind it, not cut.
     */
    public static float dialogBackdropDim(Context context) {
        boolean television = (context.getResources().getConfiguration().uiMode & android.content.res.Configuration.UI_MODE_TYPE_MASK)
                == android.content.res.Configuration.UI_MODE_TYPE_TELEVISION;
        if (!television || isDay(context)) return backdropDim(context);
        return 1f - ResourcesCompat.getFloat(context.getResources(), R.dimen.nj_dialog_tv_backdrop_brightness);
    }

    /** How dark the page behind a dialog goes: from the design's backdrop brightness, barely by day. */
    public static float backdropDim(Context context) {
        return 1f - ResourcesCompat.getFloat(context.getResources(),
                isDay(context) ? R.dimen.nj_day_sheet_backdrop_brightness : R.dimen.nj_sheet_backdrop_brightness);
    }

    @Override
    protected void onBoundsChange(Rect bounds) {
        float inset = spec.rimWidth / 2f;
        shape.reset();
        shape.addRoundRect(new RectF(bounds), radii, Path.Direction.CW);
        float[] inner = new float[8];
        for (int i = 0; i < 8; i++) inner[i] = Math.max(0f, radii[i] - inset);
        rimShape.reset();
        rimShape.addRoundRect(new RectF(bounds.left + inset, bounds.top + inset, bounds.right - inset, bounds.bottom - inset),
                inner, Path.Direction.CW);
        int h = bounds.height();
        if (h == 0) return;
        sheen.setShader(spec.sheenEnd <= 0f ? null : new LinearGradient(0, bounds.top, 0, bounds.bottom,
                new int[] {spec.sheenTop, spec.sheenTop & 0xFFFFFF, spec.sheenTop & 0xFFFFFF}, new float[] {0f, spec.sheenEnd, 1f},
                Shader.TileMode.CLAMP));
        // Lit from above and slightly from the left, as the glass surfaces are.
        double angle = Math.toRadians(168);
        float dx = (float) Math.sin(angle);
        float dy = (float) -Math.cos(angle);
        float length = Math.abs(bounds.width() * dx) + Math.abs(h * dy);
        float cx = bounds.exactCenterX();
        float cy = bounds.exactCenterY();
        rim.setShader(new LinearGradient(cx - dx * length / 2f, cy - dy * length / 2f, cx + dx * length / 2f, cy + dy * length / 2f,
                new int[] {spec.rimTop, spec.rimMid, spec.rimMid, spec.rimBottom}, new float[] {0f, 0.34f, 0.62f, 1f}, Shader.TileMode.CLAMP));
    }

    @Override
    public void draw(Canvas canvas) {
        canvas.drawPath(shape, fill);
        if (spec.sheenEnd > 0f) canvas.drawPath(shape, sheen);
        canvas.drawPath(rimShape, rim);
        if (spec.hairline != 0) canvas.drawPath(shape, hairline);
    }

    /** The rim keeps a black panel apart from a black page. */
    float rimWidth() {
        return spec.rimWidth;
    }

    /** Its shape, so the window's shadow and any clip follow the corners. */
    @Override
    public void getOutline(Outline outline) {
        Rect bounds = getBounds();
        float radius = radii[0];
        // A sheet's square bottom edge lies below the window: a round rect is the one outline every Android can clip to.
        outline.setRoundRect(bounds.left, bounds.top, bounds.right, bounds.bottom + (radii[4] > 0f ? 0 : Math.round(radius)), radius);
    }

    @Override
    public void setAlpha(int alpha) {
        fill.setAlpha(alpha);
    }

    @Override
    public void setColorFilter(ColorFilter filter) {
        fill.setColorFilter(filter);
    }

    @Override
    public int getOpacity() {
        return PixelFormat.TRANSLUCENT;
    }
}
