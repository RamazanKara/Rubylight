package com.limelight.ui;

import android.content.Context;
import android.content.res.Resources;
import android.graphics.Canvas;
import android.graphics.Outline;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RectF;
import android.util.AttributeSet;
import android.view.View;
import android.view.ViewOutlineProvider;
import android.widget.FrameLayout;

import androidx.core.content.res.ResourcesCompat;

import com.limelight.R;

/**
 * A PC drawn as a monitor on its stand: the running game's cover, or the PC's own gradient and disc (the same recipe as a game
 * with no art), dark while it is away. The children lie on the screen's face: the cover, the glyph and the spinner.
 */
public class PcScreen extends FrameLayout {
    private static final float FACE = .62f;
    private static final float RADIUS = .075f;
    private static final float NECK_W = .1f;
    private static final float NECK_H = .07f;
    private static final float BASE_W = .3f;
    private static final float BASE_H = .028f;
    private static final float DISC = .7f;
    private static final float DISC_LEFT = .42f;
    private static final float DISC_TOP = .2f;

    private final Paint face = new Paint(Paint.ANTI_ALIAS_FLAG | Paint.DITHER_FLAG);
    private final Paint disc = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint rim = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint neck = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint base = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint shadowPaint = new Paint(Paint.FILTER_BITMAP_FLAG);
    private final RectF rect = new RectF();
    private final Path faceClip = new Path();
    private final Runnable invalidator = this::invalidate;
    private final float density;
    private final int offlineFace;
    private final int onlineRim;
    private final int offlineRim;
    private final boolean highContrast;
    private String title = "";
    private boolean away;
    private int shaderW;
    private int shaderH;
    private String shaderTitle;
    private int faceW;
    private int faceH;
    private float radius;

    public PcScreen(Context context) {
        this(context, null);
    }

    public PcScreen(Context context, AttributeSet attrs) {
        super(context, attrs);
        Resources res = context.getResources();
        density = res.getDisplayMetrics().density;
        highContrast = AppTheme.isHighContrast(context);
        boolean day = AppTheme.look(context) == AppTheme.Look.DAY;
        setWillNotDraw(false);
        setClipChildren(false);
        setClipToPadding(false);
        offlineFace = day ? 0x1A1A1C22 : 0x38787C8C;
        onlineRim = highContrast ? ResourcesCompat.getColor(res, R.color.nj_hc_color_rim, null) : 0x33FFFFFF;
        offlineRim = highContrast ? onlineRim : day ? 0x261A1C22 : 0x1FFFFFFF;
        disc.setColor(NoArtTile.discColor(context));
        rim.setStyle(Paint.Style.STROKE);
        rim.setStrokeWidth(highContrast ? res.getDimension(R.dimen.nj_glass_chrome_solid_rim_width) : Math.max(1f, density * .75f));
        neck.setColor(ResourcesCompat.getColor(res, day ? R.color.nj_day_tint_track_fill : R.color.nj_tint_track_fill, null));
        int label = com.google.android.material.color.MaterialColors.getColor(this, com.google.android.material.R.attr.colorOnSurfaceVariant, 0xFF8D90A0);
        base.setColor((label & 0xFFFFFF) | 0x80000000);
        shadowPaint.setColor(ResourcesCompat.getColor(res, R.color.nj_cover_shadow, null));
    }

    @Override
    protected void onFinishInflate() {
        super.onFinishInflate();
        View cover = findViewById(R.id.grid_backdrop);
        if (cover == null) return;
        cover.setOutlineProvider(new ViewOutlineProvider() {
            @Override public void getOutline(View view, Outline outline) {
                outline.setRoundRect(0, 0, view.getWidth(), view.getHeight(), radius);
            }
        });
        cover.setClipToOutline(true);
    }

    /** Which PC this is, and whether it is away (a dark face). */
    public void bind(String name, boolean isAway) {
        if (name.equals(title) && away == isAway) return;
        title = name;
        away = isAway;
        invalidate();
    }

    @Override
    protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        int w = MeasureSpec.getSize(widthMeasureSpec);
        faceW = w;
        faceH = Math.round(w * FACE);
        int stand = Math.round(w * NECK_H) + standBase(w);
        int total = faceH + stand;
        radius = Math.max(5 * density, Math.round(w * RADIUS));
        super.onMeasure(MeasureSpec.makeMeasureSpec(w, MeasureSpec.EXACTLY), MeasureSpec.makeMeasureSpec(faceH, MeasureSpec.EXACTLY));
        setMeasuredDimension(w, total);
    }

    private int standBase(int w) {
        return Math.max(Math.round(2 * density), Math.round(w * BASE_H));
    }

    @Override
    protected void onLayout(boolean changed, int left, int top, int right, int bottom) {
        // The children fill the face; the stand under it is this view's own.
        super.onLayout(changed, left, top, right, top + faceH);
    }

    @Override
    protected void dispatchDraw(Canvas canvas) {
        int w = faceW;
        int h = faceH;
        if (w == 0) {
            super.dispatchDraw(canvas);
            return;
        }
        drawStand(canvas, w, h);
        if (!highContrast) {
            GlassShadow shadow = GlassShadow.get(radius, 10 * density, invalidator);
            if (shadow != null) shadow.draw(canvas, 0, 4 * density, w, h + 4 * density, shadowPaint);
        }
        drawFace(canvas, w, h);
        super.dispatchDraw(canvas);
        float inset = rim.getStrokeWidth() / 2f;
        rect.set(inset, inset, w - inset, h - inset);
        rim.setColor(away ? offlineRim : onlineRim);
        canvas.drawRoundRect(rect, radius - inset, radius - inset, rim);
    }

    private void drawStand(Canvas canvas, int w, int h) {
        float neckW = w * NECK_W;
        float neckH = Math.round(w * NECK_H);
        canvas.drawRect((w - neckW) / 2f, h, (w + neckW) / 2f, h + neckH, neck);
        float baseW = w * BASE_W;
        float baseH = standBase(w);
        rect.set((w - baseW) / 2f, h + neckH, (w + baseW) / 2f, h + neckH + baseH);
        canvas.drawRoundRect(rect, 2 * density, 2 * density, base);
    }

    private void drawFace(Canvas canvas, int w, int h) {
        rect.set(0, 0, w, h);
        if (away) {
            face.setShader(null);
            face.setColor(offlineFace);
            shaderTitle = null;
        } else if (w != shaderW || h != shaderH || !title.equals(shaderTitle)) {
            shaderW = w;
            shaderH = h;
            shaderTitle = title;
            face.setColor(0xFF000000);
            face.setShader(NoArtTile.gradient(getContext(), title, w, h));
        }
        canvas.drawRoundRect(rect, radius, radius, face);
        if (away) return;
        int save = canvas.save();
        faceClip.rewind();
        faceClip.addRoundRect(rect, radius, radius, Path.Direction.CW);
        canvas.clipPath(faceClip);
        float d = w * DISC;
        canvas.drawCircle(w * DISC_LEFT + d / 2f, h * DISC_TOP + d / 2f, d / 2f, disc);
        canvas.restoreToCount(save);
    }

    /** The face's height as measured, for tests. */
    public int faceHeight() {
        return faceH;
    }
}
