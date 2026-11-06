package com.limelight.ui;

import android.content.Context;
import android.content.res.TypedArray;
import android.graphics.Canvas;
import android.graphics.ColorMatrixColorFilter;
import android.graphics.LinearGradient;
import android.graphics.Outline;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RecordingCanvas;
import android.graphics.Region;
import android.graphics.RenderEffect;
import android.graphics.RenderNode;
import android.graphics.RuntimeShader;
import android.graphics.Shader;
import android.os.Build;
import android.util.AttributeSet;
import android.view.ViewParent;
import android.widget.FrameLayout;

import androidx.annotation.RequiresApi;

import com.limelight.R;

/**
 * A piece of glass: back to front, shadow, the blurred backdrop or the baked ambient or an opaque fill, depending on
 * the tier, the fill, the lens band, the sheen and the rim. Its children sit on top. Must be a descendant of a
 * {@link GlassHost}, but not within the host's first child; without a host it draws its fill.
 */
public class GlassSurface extends FrameLayout {
    /** The blur works on the content at this fraction of its size. */
    static final int BLUR_SCALE = 4;

    /** The band only brightens and shifts UVs in the outer ring; p is in pixels of the surface. */
    private static final String LENS_SHADER = "uniform shader content;\n"
            + "uniform float2 size;\n"
            + "uniform float radius;\n"
            + "uniform float band;\n"
            + "half4 main(float2 p) {\n"
            + "  float2 c = size * 0.5;\n"
            + "  float2 q = abs(p - c) - (c - radius);\n"
            + "  float d = length(max(q, float2(0.0))) + min(max(q.x, q.y), 0.0) - radius;\n"
            + "  float t = clamp(1.0 + d / band, 0.0, 1.0);\n"
            + "  if (t <= 0.0) return half4(0.0);\n"
            + "  float2 outward = q.x > 0.0 && q.y > 0.0 ? normalize(q) : (q.x > q.y ? float2(1.0, 0.0) : float2(0.0, 1.0));\n"
            + "  outward *= sign(p - c);\n"
            + "  float2 uv = p - outward * t * t * band * 1.2;\n"
            + "  return content.eval(uv) * half(t);\n"
            + "}\n";

    private final String material;
    private GlassHost host;
    private GlassSpec spec;
    private GlassTier tier;
    private boolean day;
    private float radius = -1f;
    private final int[] at = new int[2];
    private final int[] hostAt = new int[2];
    private int atX;
    private int atY;

    private final Paint fillPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint sheenPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint rimPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint hairlinePaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint shadowPaint = new Paint(Paint.FILTER_BITMAP_FLAG);
    private final Paint bakedPaint = new Paint(Paint.FILTER_BITMAP_FLAG);
    private final Path shape = new Path();
    private final Path rimShape = new Path();

    private Object glassNode;
    private Object lensNode;
    private boolean nodesStale = true;

    public GlassSurface(Context context) {
        this(context, null);
    }

    public GlassSurface(Context context, AttributeSet attrs) {
        super(context, attrs);
        TypedArray a = context.obtainStyledAttributes(attrs, R.styleable.GlassSurface);
        String name = a.getString(R.styleable.GlassSurface_glassMaterial);
        material = name == null ? "chrome" : name;
        radius = a.getDimension(R.styleable.GlassSurface_glassRadius, -1f);
        a.recycle();
        setWillNotDraw(false);
        setClipChildren(false);
        setClipToPadding(false);
        rimPaint.setStyle(Paint.Style.STROKE);
        hairlinePaint.setStyle(Paint.Style.STROKE);
        setOutlineProvider(null);
    }

    public void setCornerRadius(float radius) {
        this.radius = radius;
        rebuild();
    }

    /** The current tier as this surface draws it. */
    public GlassTier tier() {
        return tier;
    }

    private GlassHost findHost() {
        ViewParent parent = getParent();
        while (parent != null) {
            if (parent instanceof GlassHost) return (GlassHost) parent;
            parent = parent.getParent();
        }
        return null;
    }

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        host = findHost();
        if (host != null) host.register(this);
        onTierChanged();
    }

    @Override
    protected void onDetachedFromWindow() {
        super.onDetachedFromWindow();
        if (host != null) host.unregister(this);
        host = null;
        glassNode = null;
        lensNode = null;
    }

    void onTierChanged() {
        GlassTier now = host != null ? host.tier() : AppTheme.isHighContrast(getContext()) ? GlassTier.SOLID : GlassTier.BAKED;
        tier = now;
        day = AppTheme.look(getContext()) == AppTheme.Look.DAY;
        spec = GlassSpec.of(getContext(), material, now, day);
        fillPaint.setColor(spec.fill);
        shadowPaint.setColor(spec.shadowColor);
        hairlinePaint.setColor(spec.hairline);
        hairlinePaint.setStrokeWidth(Math.max(1f, getResources().getDisplayMetrics().density * 0.5f));
        rimPaint.setStrokeWidth(spec.rimWidth);
        bakedPaint.setColorFilter(new ColorMatrixColorFilter(GlassHost.glassMatrix(1f, spec.brightness)));
        nodesStale = true;
        rebuild();
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        rebuild();
    }

    @Override
    protected void onLayout(boolean changed, int left, int top, int right, int bottom) {
        super.onLayout(changed, left, top, right, bottom);
        if (host == null) return;
        getLocationInWindow(at);
        host.getLocationInWindow(hostAt);
        // A transform in flight on this view is not part of where its backdrop lies.
        int x = at[0] - hostAt[0] - Math.round(getTranslationX());
        int y = at[1] - hostAt[1] - Math.round(getTranslationY());
        if (x != atX || y != atY) {
            atX = x;
            atY = y;
            nodesStale = true;
            invalidate();
        }
    }

    private float cornerRadius() {
        float half = Math.min(getWidth(), getHeight()) / 2f;
        return radius < 0f ? half : Math.min(radius, half);
    }

    private void rebuild() {
        int w = getWidth();
        int h = getHeight();
        if (w == 0 || h == 0 || spec == null) return;
        float r = cornerRadius();
        shape.reset();
        shape.addRoundRect(0, 0, w, h, r, r, Path.Direction.CW);
        float inset = spec.rimWidth / 2f;
        rimShape.reset();
        rimShape.addRoundRect(inset, inset, w - inset, h - inset, Math.max(0f, r - inset), Math.max(0f, r - inset), Path.Direction.CW);
        if (spec.sheenEnd > 0f) {
            sheenPaint.setShader(new LinearGradient(0, 0, 0, h, new int[] {spec.sheenTop, spec.sheenTop & 0xFFFFFF, spec.sheenTop & 0xFFFFFF},
                    new float[] {0f, spec.sheenEnd, 1f}, Shader.TileMode.CLAMP));
        } else {
            sheenPaint.setShader(null);
        }
        // The rim is lit from above and slightly from the left, as CSS 168deg: mostly vertical.
        double angle = Math.toRadians(168);
        float dx = (float) Math.sin(angle);
        float dy = (float) -Math.cos(angle);
        float length = Math.abs(w * dx) + Math.abs(h * dy);
        float cx = w / 2f;
        float cy = h / 2f;
        rimPaint.setShader(new LinearGradient(cx - dx * length / 2f, cy - dy * length / 2f, cx + dx * length / 2f, cy + dy * length / 2f,
                new int[] {spec.rimTop, spec.rimMid, spec.rimMid, spec.rimBottom}, new float[] {0f, 0.34f, 0.62f, 1f}, Shader.TileMode.CLAMP));
        nodesStale = true;
        invalidate();
    }

    private final Runnable invalidator = this::invalidate;

    @Override
    protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        if (spec == null || getWidth() == 0) return;
        drawShadow(canvas);
        if (isFocused()) focusRing().drawHalo(canvas, 0, 0, getWidth(), getHeight(), cornerRadius());
        drawBack(canvas);
        canvas.drawPath(shape, fillPaint);
        if (spec.sheenEnd > 0f) canvas.drawPath(shape, sheenPaint);
        canvas.drawPath(rimShape, rimPaint);
        if (spec.hairline != 0) canvas.drawPath(shape, hairlinePaint);
    }

    @Override
    public void onDrawForeground(Canvas canvas) {
        super.onDrawForeground(canvas);
        if (isFocused()) focusRing().drawRing(canvas, 0, 0, getWidth(), getHeight(), cornerRadius());
    }

    private FocusRing ring;

    private FocusRing focusRing() {
        if (ring == null) ring = new FocusRing(this, false);
        return ring;
    }

    @Override
    protected void onFocusChanged(boolean gainFocus, int direction, android.graphics.Rect previously) {
        super.onFocusChanged(gainFocus, direction, previously);
        retarget();
        invalidate();
    }

    @Override
    public void setPressed(boolean pressed) {
        boolean changed = pressed != isPressed();
        super.setPressed(pressed);
        if (changed && isClickable()) retarget();
    }

    private void retarget() {
        float focus = androidx.core.content.res.ResourcesCompat.getFloat(getResources(), R.dimen.nj_focus_scale_control);
        Springs.scale(this, isPressed() ? 0.97f : isFocused() ? focus : 1f, isPressed() ? Springs.Kind.PRESS : Springs.Kind.FOCUS);
    }

    private void drawShadow(Canvas canvas) {
        if (spec.shadowBlur <= 0f) return;
        GlassShadow shadow = GlassShadow.get(cornerRadius(), spec.shadowBlur, invalidator);
        if (shadow == null) return;
        canvas.save();
        // The surface is see-through: its own shadow must not show inside it.
        canvas.clipPath(shape, Region.Op.DIFFERENCE);
        shadow.draw(canvas, 0, spec.shadowY, getWidth(), getHeight() + spec.shadowY, shadowPaint);
        canvas.restore();
    }

    private void drawBack(Canvas canvas) {
        if (host == null || tier == GlassTier.SOLID) return;
        if (tier == GlassTier.BAKED) {
            canvas.save();
            canvas.clipPath(shape);
            host.drawBaked(canvas, atX, atY, getWidth(), getHeight(), bakedPaint);
            canvas.restore();
            return;
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && canvas.isHardwareAccelerated()) {
            drawLive(canvas);
        }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private void drawLive(Canvas canvas) {
        RenderNode blur = host.blurNode(spec);
        if (blur == null) {
            // The very first frame: fill only. The next one has the blur.
            post(invalidator);
            return;
        }
        if (nodesStale || glassNode == null) {
            recordGlass(blur);
            if (tier == GlassTier.LENS && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) recordLens();
            nodesStale = false;
        }
        canvas.drawRenderNode((RenderNode) glassNode);
        if (lensNode != null && tier == GlassTier.LENS) canvas.drawRenderNode((RenderNode) lensNode);
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private void recordGlass(RenderNode blur) {
        RenderNode node = glassNode == null ? new RenderNode("glass") : (RenderNode) glassNode;
        glassNode = node;
        clip(node);
        RecordingCanvas recording = node.beginRecording(getWidth(), getHeight());
        recording.translate(-atX, -atY);
        recording.scale(BLUR_SCALE, BLUR_SCALE);
        recording.drawRenderNode(blur);
        node.endRecording();
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private void clip(RenderNode node) {
        node.setPosition(0, 0, getWidth(), getHeight());
        Outline outline = new Outline();
        outline.setRoundRect(0, 0, getWidth(), getHeight(), cornerRadius());
        node.setOutline(outline);
        node.setClipToOutline(true);
    }

    /** The outer band: its own view of the content, a touch blurred and brightened, with shifted UVs. */
    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private void recordLens() {
        RenderNode content = host.contentNode();
        if (content == null) return;
        RenderNode node = lensNode == null ? new RenderNode("glass-lens") : (RenderNode) lensNode;
        try {
            RuntimeShader shader = new RuntimeShader(LENS_SHADER);
            shader.setFloatUniform("size", getWidth(), getHeight());
            shader.setFloatUniform("radius", cornerRadius());
            shader.setFloatUniform("band", spec.lensWidth);
            float sigma = spec.lensBlur;
            node.setRenderEffect(RenderEffect.createChainEffect(
                    RenderEffect.createRuntimeShaderEffect(shader, "content"),
                    RenderEffect.createChainEffect(
                            RenderEffect.createColorFilterEffect(new ColorMatrixColorFilter(
                                    GlassHost.glassMatrix(spec.lensSaturate, spec.lensBrightness * spec.brightness))),
                            RenderEffect.createBlurEffect(GlassHost.radiusForSigma(sigma), GlassHost.radiusForSigma(sigma), Shader.TileMode.CLAMP))));
        } catch (RuntimeException failed) {
            // A driver that cannot run the band keeps the blur: that is the live tier's look.
            lensNode = null;
            return;
        }
        lensNode = node;
        clip(node);
        RecordingCanvas recording = node.beginRecording(getWidth(), getHeight());
        recording.translate(-atX, -atY);
        recording.drawRenderNode(content);
        node.endRecording();
    }
}
