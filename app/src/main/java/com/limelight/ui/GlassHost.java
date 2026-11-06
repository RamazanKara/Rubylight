package com.limelight.ui;

import android.animation.ValueAnimator;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.ColorMatrix;
import android.graphics.ColorMatrixColorFilter;
import android.graphics.Paint;
import android.graphics.RecordingCanvas;
import android.graphics.RenderEffect;
import android.graphics.RenderNode;
import android.graphics.Shader;
import android.os.Build;
import android.util.AttributeSet;
import android.view.Choreographer;
import android.view.View;
import android.view.animation.LinearInterpolator;
import android.widget.FrameLayout;

import androidx.annotation.RequiresApi;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * The root of a screen with glass. Its first child is the content the glass floats over; every later child is chrome.
 *
 * <p>On the live tiers the first child is recorded into one shared {@link RenderNode} per window, and each recipe
 * blurs that at quarter scale in its own node. A {@link GlassSurface} only draws those nodes clipped to its shape, so
 * scrolling costs no re-recording here: a descendant's redraw reaches the blur through the node tree. Live blur starts
 * on the frame after the first, so a screen's first draw has none of this work.
 *
 * <p>On the baked tier nothing is recorded: surfaces sample the static ambient in screen space.
 */
public class GlassHost extends FrameLayout {
    private GlassTier tier = GlassTier.BAKED;
    private boolean liveReady;
    private boolean liveScheduled;
    private final List<GlassSurface> surfaces = new ArrayList<>();
    private Live live;

    private Bitmap ambient;
    private Bitmap ambientBefore;
    private float ambientMix = 1f;
    private ValueAnimator ambientFade;
    private int ambientFadeMs = 180;

    public GlassHost(Context context) {
        this(context, null);
    }

    public GlassHost(Context context, AttributeSet attrs) {
        super(context, attrs);
        setClipChildren(false);
    }

    public GlassTier tier() {
        return tier;
    }

    /** Sets the tier. A live tier takes effect on the frame after the first draw. */
    public void setTier(GlassTier tier) {
        if (this.tier == tier) return;
        this.tier = tier;
        if (!isLive() && live != null) live = null;
        for (GlassSurface surface : surfaces) surface.onTierChanged();
        invalidate();
    }

    boolean isLive() {
        return (tier == GlassTier.LENS || tier == GlassTier.LIVE) && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S;
    }

    /** The ambient the baked tier samples; {@code animate} crossfades to it. */
    public void setAmbient(Bitmap bitmap, boolean animate, int fadeMs) {
        if (bitmap == ambient) return;
        ambientFadeMs = fadeMs;
        if (ambientFade != null) ambientFade.cancel();
        boolean wanted = tier == GlassTier.BAKED;
        ambientBefore = animate && wanted ? ambient : null;
        ambient = bitmap;
        ambientMix = ambientBefore == null ? 1f : 0f;
        if (!wanted) return;
        if (ambientBefore != null) {
            ValueAnimator fade = ValueAnimator.ofFloat(0f, 1f);
            fade.setDuration(ambientFadeMs);
            fade.setInterpolator(new LinearInterpolator());
            fade.addUpdateListener(a -> {
                ambientMix = (float) a.getAnimatedValue();
                invalidateSurfaces();
            });
            ambientFade = fade;
            fade.start();
        }
        invalidateSurfaces();
    }

    private void invalidateSurfaces() {
        for (GlassSurface surface : surfaces) surface.invalidate();
    }

    /** Told the window's safe area: a screen with glass lays out edge to edge and keeps its chrome and text inside it. */
    public interface SafeAreaListener {
        void onSafeArea(int left, int top, int right, int bottom);
    }

    private SafeAreaListener safeAreaListener;
    private final int[] safeArea = new int[4];

    public void setSafeAreaListener(SafeAreaListener listener) {
        safeAreaListener = listener;
        listener.onSafeArea(safeArea[0], safeArea[1], safeArea[2], safeArea[3]);
    }

    void setSafeArea(int left, int top, int right, int bottom) {
        safeArea[0] = left;
        safeArea[1] = top;
        safeArea[2] = right;
        safeArea[3] = bottom;
        if (safeAreaListener != null) safeAreaListener.onSafeArea(left, top, right, bottom);
    }

    void register(GlassSurface surface) {
        surfaces.add(surface);
    }

    void unregister(GlassSurface surface) {
        surfaces.remove(surface);
    }

    /** Draws the ambient into a surface at its place in the window, through the surface's paint (brightness). */
    void drawBaked(Canvas canvas, int atX, int atY, int surfaceWidth, int surfaceHeight, Paint paint) {
        if (ambient == null) return;
        int alpha = paint.getAlpha();
        if (ambientBefore != null && ambientMix < 1f) {
            drawAmbient(canvas, ambientBefore, atX, atY, surfaceWidth, surfaceHeight, paint);
            paint.setAlpha(Math.round(ambientMix * 255f));
        }
        drawAmbient(canvas, ambient, atX, atY, surfaceWidth, surfaceHeight, paint);
        paint.setAlpha(alpha);
    }

    /** The part of a centre-cropped bitmap that lies under the surface, as {@link BackdropView} shows it. */
    private void drawAmbient(Canvas canvas, Bitmap bitmap, int atX, int atY, int w, int h, Paint paint) {
        int hostW = getWidth();
        int hostH = getHeight();
        if (hostW == 0 || hostH == 0) return;
        float scale = Math.max(hostW / (float) bitmap.getWidth(), hostH / (float) bitmap.getHeight());
        float dx = (hostW - bitmap.getWidth() * scale) / 2f;
        float dy = (hostH - bitmap.getHeight() * scale) / 2f;
        canvas.save();
        canvas.translate(dx - atX, dy - atY);
        canvas.scale(scale, scale);
        canvas.drawBitmap(bitmap, 0, 0, paint);
        canvas.restore();
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        if (live != null) live.resize(w, h);
    }

    @Override
    protected void dispatchDraw(Canvas canvas) {
        super.dispatchDraw(canvas);
        if (!liveReady && !liveScheduled && isLive()) {
            // The first frame is out; the next one may blur.
            liveScheduled = true;
            Choreographer.getInstance().postFrameCallback(frame -> {
                liveReady = true;
                invalidate();
                invalidateSurfaces();
            });
        }
    }

    @Override
    protected boolean drawChild(Canvas canvas, View child, long drawingTime) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && liveReady && isLive()
                && child == getChildAt(0) && canvas.isHardwareAccelerated()) {
            return recordContent(canvas, child, drawingTime);
        }
        return super.drawChild(canvas, child, drawingTime);
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private boolean recordContent(Canvas canvas, View child, long drawingTime) {
        if (live == null) {
            live = new Live(this);
            live.resize(getWidth(), getHeight());
        }
        RecordingCanvas recording = live.content.beginRecording(getWidth(), getHeight());
        boolean more;
        try {
            more = super.drawChild(recording, child, drawingTime);
        } finally {
            live.content.endRecording();
        }
        canvas.drawRenderNode(live.content);
        return more;
    }

    /** The shared nodes. Null until a live tier first draws; surfaces fall back to their fill until then. */
    @RequiresApi(Build.VERSION_CODES.S)
    RenderNode contentNode() {
        return live == null ? null : live.content;
    }

    /** The blurred, coloured content at quarter scale for this recipe. */
    @RequiresApi(Build.VERSION_CODES.S)
    RenderNode blurNode(GlassSpec spec) {
        return live == null ? null : live.blur(spec);
    }

    static ColorMatrix glassMatrix(float saturate, float brightness) {
        ColorMatrix matrix = new ColorMatrix();
        matrix.setSaturation(saturate);
        ColorMatrix scale = new ColorMatrix();
        scale.setScale(brightness, brightness, brightness, 1f);
        matrix.postConcat(scale);
        return matrix;
    }

    /** A Gaussian's sigma in pixels as RenderEffect's radius. */
    static float radiusForSigma(float sigma) {
        return Math.max(0.5f, (sigma - 0.5f) / 0.57735f);
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private static final class Live {
        static final int SCALE = GlassSurface.BLUR_SCALE;
        final RenderNode content = new RenderNode("glass-content");
        final Map<GlassSpec, RenderNode> blurs = new HashMap<>();
        int width;
        int height;

        Live(GlassHost host) { }

        void resize(int w, int h) {
            width = w;
            height = h;
            content.setPosition(0, 0, w, h);
            for (Map.Entry<GlassSpec, RenderNode> entry : blurs.entrySet()) record(entry.getValue());
        }

        RenderNode blur(GlassSpec spec) {
            RenderNode node = blurs.get(spec);
            if (node != null) return node;
            node = new RenderNode("glass-blur");
            float sigma = spec.blur / SCALE;
            node.setRenderEffect(RenderEffect.createChainEffect(
                    RenderEffect.createColorFilterEffect(new ColorMatrixColorFilter(glassMatrix(spec.saturate, spec.brightness))),
                    RenderEffect.createBlurEffect(radiusForSigma(sigma), radiusForSigma(sigma), Shader.TileMode.CLAMP)));
            blurs.put(spec, node);
            record(node);
            return node;
        }

        private void record(RenderNode node) {
            int w = Math.max(1, width / SCALE);
            int h = Math.max(1, height / SCALE);
            node.setPosition(0, 0, w, h);
            RecordingCanvas canvas = node.beginRecording(w, h);
            canvas.scale(1f / SCALE, 1f / SCALE);
            canvas.drawRenderNode(content);
            node.endRecording();
        }
    }
}
