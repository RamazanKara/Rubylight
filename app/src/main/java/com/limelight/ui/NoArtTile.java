package com.limelight.ui;

import android.content.Context;
import android.content.res.Resources;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.LinearGradient;
import android.graphics.Paint;
import android.graphics.Shader;
import android.text.TextUtils;
import android.util.AttributeSet;
import android.util.LruCache;
import android.view.Gravity;

import androidx.appcompat.widget.AppCompatTextView;
import androidx.core.content.ContextCompat;
import androidx.core.content.res.ResourcesCompat;
import androidx.core.widget.TextViewCompat;

import com.limelight.R;

import java.nio.charset.StandardCharsets;
import java.util.Locale;

/**
 * What a game with no art shows in its cover's slot: a gradient pair and a soft disc picked by the title's hash, and the title.
 * The same title looks the same on Android and the desktop client (client/design/README.md, "No-art tile"). The loaders write the
 * title and show or hide this view like the plain text it replaces; the cover around it gives the sheen, the rim and the shadow.
 */
public class NoArtTile extends AppCompatTextView {
    private static final int FNV_OFFSET = 0x811C9DC5;
    private static final int FNV_PRIME = 0x01000193;
    private static final int PAIRS = 12;
    private static final float[][] DISC_AT = {{.74f, .62f}, {.26f, .7f}, {.8f, .42f}, {.5f, .78f}};
    /** CSS's 165 degrees: down and a little to the right. */
    private static final float GRADIENT_X = (float) Math.sin(Math.toRadians(165));
    private static final float GRADIENT_Y = (float) -Math.cos(Math.toRadians(165));
    private static final float LINE_HEIGHT = 1.08f;
    private static final float TRACKING = -.01f;
    private static final int[][] PAIR = {
            {R.color.nj_tile_pair0_top, R.color.nj_tile_pair0_bottom}, {R.color.nj_tile_pair1_top, R.color.nj_tile_pair1_bottom},
            {R.color.nj_tile_pair2_top, R.color.nj_tile_pair2_bottom}, {R.color.nj_tile_pair3_top, R.color.nj_tile_pair3_bottom},
            {R.color.nj_tile_pair4_top, R.color.nj_tile_pair4_bottom}, {R.color.nj_tile_pair5_top, R.color.nj_tile_pair5_bottom},
            {R.color.nj_tile_pair6_top, R.color.nj_tile_pair6_bottom}, {R.color.nj_tile_pair7_top, R.color.nj_tile_pair7_bottom},
            {R.color.nj_tile_pair8_top, R.color.nj_tile_pair8_bottom}, {R.color.nj_tile_pair9_top, R.color.nj_tile_pair9_bottom},
            {R.color.nj_tile_pair10_top, R.color.nj_tile_pair10_bottom}, {R.color.nj_tile_pair11_top, R.color.nj_tile_pair11_bottom},
    };
    private static final int AMBIENT_W = 64;
    private static final int AMBIENT_H = 96;
    private static final LruCache<Integer, Bitmap> AMBIENTS = new LruCache<>(PAIRS);

    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint disc = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final float titleScale;
    private final float titleMinScale;
    private final float insetScale;
    private final float discScale;
    private int shaderW;
    private int shaderH;
    private String shaderTitle;
    private int fittedWidth;
    private String fittedTitle;

    public NoArtTile(Context context) {
        this(context, null);
    }

    public NoArtTile(Context context, AttributeSet attrs) {
        super(context, attrs);
        Resources res = context.getResources();
        titleScale = ResourcesCompat.getFloat(res, R.dimen.nj_tile_title_scale);
        titleMinScale = ResourcesCompat.getFloat(res, R.dimen.nj_tile_title_min_scale);
        insetScale = ResourcesCompat.getFloat(res, R.dimen.nj_tile_inset_scale);
        discScale = ResourcesCompat.getFloat(res, R.dimen.nj_tile_disc_scale);
        disc.setColor(discColor(context));
        TextViewCompat.setTextAppearance(this, R.style.TextAppearance_Nightjar_Title);
        setTextColor(Color.WHITE);
        setGravity(Gravity.TOP | Gravity.START);
        setMaxLines(3);
        setEllipsize(TextUtils.TruncateAt.END);
        setIncludeFontPadding(false);
        setLetterSpacing(TRACKING);
    }

    /** FNV-1a over the lowercase title's UTF-8. */
    public static int hash(String title) {
        int hash = FNV_OFFSET;
        for (byte b : title.toLowerCase(Locale.ROOT).getBytes(StandardCharsets.UTF_8)) {
            hash ^= b & 0xFF;
            hash *= FNV_PRIME;
        }
        return hash;
    }

    /** Which of the twelve gradient pairs this title gets. */
    public static int pairOf(String title) {
        return Integer.remainderUnsigned(hash(title), PAIRS);
    }

    /** Which of the four disc places this title gets. */
    public static int discOf(String title) {
        return Integer.remainderUnsigned(hash(title) >>> 8, DISC_AT.length);
    }

    /** The disc's centre as fractions of the tile's width and height. */
    public static float[] discCentre(String title) {
        return DISC_AT[discOf(title)].clone();
    }

    /** The title's gradient across a {@code w} by {@code h} face. */
    public static Shader gradient(Context context, String title, int w, int h) {
        int[] pair = PAIR[pairOf(title)];
        return gradient(ContextCompat.getColor(context, pair[0]), ContextCompat.getColor(context, pair[1]), w, h);
    }

    private static Shader gradient(int top, int bottom, int w, int h) {
        // The ends of a CSS gradient line, so the corners get the pure colours.
        float half = (Math.abs(w * GRADIENT_X) + Math.abs(h * GRADIENT_Y)) / 2f;
        float cx = w / 2f;
        float cy = h / 2f;
        return new LinearGradient(cx - GRADIENT_X * half, cy - GRADIENT_Y * half, cx + GRADIENT_X * half, cy + GRADIENT_Y * half,
                top, bottom, Shader.TileMode.CLAMP);
    }

    /** White at the token's strength, for the disc here and on a PC's screen. */
    public static int discColor(Context context) {
        float alpha = ResourcesCompat.getFloat(context.getResources(), R.dimen.nj_tile_disc_alpha);
        return (Math.round(alpha * 255f) << 24) | 0xFFFFFF;
    }

    /**
     * What a tile gives the backdrop where a cover would give its art: its gradient, alone. One small bitmap per pair, the blur
     * reads it like a cover.
     */
    public static Bitmap ambientSource(Context context, String title) {
        int pair = pairOf(title);
        Bitmap made = AMBIENTS.get(pair);
        if (made != null) return made;
        made = Bitmap.createBitmap(AMBIENT_W, AMBIENT_H, Bitmap.Config.ARGB_8888);
        Paint paint = new Paint();
        paint.setShader(gradient(context, title, AMBIENT_W, AMBIENT_H));
        new Canvas(made).drawRect(0, 0, AMBIENT_W, AMBIENT_H, paint);
        AMBIENTS.put(pair, made);
        return made;
    }

    @Override
    protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        int width = MeasureSpec.getSize(widthMeasureSpec);
        String title = String.valueOf(getText());
        if (width > 0 && (width != fittedWidth || !title.equals(fittedTitle))) fit(width, title);
        super.onMeasure(widthMeasureSpec, heightMeasureSpec);
    }

    /** Sizes come from the width: a tile is the same picture in a list as in a hero. */
    private void fit(int width, String title) {
        fittedWidth = width;
        fittedTitle = title;
        int inset = Math.round(width * insetScale);
        setPadding(inset, inset, inset, inset);
        float size = width * titleScale;
        Paint measure = new Paint(getPaint());
        measure.setTextSize(size);
        if (measure.measureText(longestWord(title)) > width - 2 * inset) size = width * titleMinScale;
        setTextSize(android.util.TypedValue.COMPLEX_UNIT_PX, size);
        TextViewCompat.setLineHeight(this, Math.max(1, Math.round(size * LINE_HEIGHT)));
    }

    static String longestWord(String title) {
        String longest = "";
        for (String word : title.trim().split("\\s+")) {
            if (word.length() > longest.length()) longest = word;
        }
        return longest;
    }

    /** The size the title is set in now, for tests. */
    float titleSize() {
        return getTextSize();
    }

    @Override
    protected void onDraw(Canvas canvas) {
        int w = getWidth();
        int h = getHeight();
        String title = String.valueOf(getText());
        if (w != shaderW || h != shaderH || !title.equals(shaderTitle)) {
            shaderW = w;
            shaderH = h;
            shaderTitle = title;
            fill.setShader(gradient(getContext(), title, w, h));
        }
        canvas.drawRect(0, 0, w, h, fill);
        float[] centre = DISC_AT[discOf(title)];
        canvas.drawCircle(w * centre[0], h * centre[1], w * discScale / 2f, disc);
        super.onDraw(canvas);
    }
}
