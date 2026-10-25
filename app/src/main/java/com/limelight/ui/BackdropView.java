package com.limelight.ui;

import android.animation.Animator;
import android.animation.AnimatorListenerAdapter;
import android.animation.AnimatorSet;
import android.animation.ObjectAnimator;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.LightingColorFilter;
import android.os.Handler;
import android.os.Looper;
import android.util.AttributeSet;
import android.util.LruCache;
import android.view.View;
import android.view.animation.AnimationUtils;
import android.view.animation.LinearInterpolator;
import android.widget.FrameLayout;
import android.widget.ImageView;

import com.limelight.R;

import java.util.concurrent.Executor;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;

/**
 * The ambient backdrop behind a TV screen: the focused game's cover, blurred and darkened (see {@link BackdropBlur}),
 * full bleed. Two ImageViews take turns, so a change is a crossfade with a short settle zoom on the incoming one.
 *
 * <p>It is asked for a backdrop with {@link #focus} on every selection change and only acts on the last ask: the
 * change starts {@link #SETTLE_MS} after the selection settles, {@link #FAST_SETTLE_MS} while a key is held, so a run
 * across the library costs one change and no work per key repeat. Thumbnails are built on a worker thread and kept in
 * a small LruCache. With Reduce motion the new backdrop is swapped in at once.
 *
 * <p>It lays itself out over the whole window, under the safe-area padding and the TV overscan of its parents.
 *
 * <p>The last backdrop shown on any screen is kept (a 96 px thumbnail), so the next screen that is about the same
 * game, such as its game page opened from Home, can open on the same picture with {@link #adopt} instead of fading
 * in from the canvas.
 */
public class BackdropView extends FrameLayout {
    /** A backdrop starts changing this long after the last request. */
    public static final long SETTLE_MS = 90;
    /** And this long while the selection is moving fast (a held key). */
    public static final long FAST_SETTLE_MS = 250;
    static final long FADE_MS = 280;
    static final long FIRST_FADE_MS = 300;
    static final long ZOOM_MS = 600;
    static final float ZOOM_FROM = 1.04f;
    private static final int CACHE_ENTRIES = 48;
    private static final LinearInterpolator LINEAR = new LinearInterpolator();

    private static final Executor WORKER = worker();
    /** The cache key and thumbnail of the backdrop the last screen showed, for {@link #adopt}. Main thread only. */
    private static String handoffKey;
    private static Bitmap handoffBitmap;

    private final ImageView back;
    private final ImageView front;
    private final LruCache<String, Bitmap> cache = new LruCache<>(CACHE_ENTRIES);
    private final Handler handler = new Handler(Looper.getMainLooper());
    private Executor executor = WORKER;

    private int canvas = 0xFF0B0D12;
    private String wantedKey;
    private Bitmap wantedCover;
    private boolean wantedNoArt;
    private String shownKey;
    private android.view.animation.Interpolator settleInterpolator;
    private final android.view.ViewTreeObserver.OnGlobalLayoutListener aligner = this::align;
    private int request;
    private int changes;
    private Bitmap shown;
    private AnimatorSet transition;
    private Runnable onCovered;
    private final Runnable apply = this::applyWanted;

    public BackdropView(Context context) {
        this(context, null);
    }

    public BackdropView(Context context, AttributeSet attrs) {
        super(context, attrs);
        back = newLayer();
        front = newLayer();
        front.setVisibility(GONE);
        setFocusable(false);
        setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO);
    }

    private ImageView newLayer() {
        ImageView layer = new ImageView(getContext());
        layer.setScaleType(ImageView.ScaleType.CENTER_CROP);
        layer.setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO);
        addView(layer, new LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT));
        return layer;
    }

    private static Executor worker() {
        ThreadPoolExecutor pool = new ThreadPoolExecutor(1, 1, 5, TimeUnit.SECONDS, new LinkedBlockingQueue<>(), runnable -> {
            Thread thread = new Thread(runnable, "backdrop-blur");
            thread.setDaemon(true);
            thread.setPriority(Thread.MIN_PRIORITY);
            return thread;
        });
        pool.allowCoreThreadTimeOut(true);
        return pool;
    }

    /** The screen's own colour: a game with no art gets it, and the blur fades into it. */
    public void setCanvasColor(int color) {
        canvas = color | 0xFF000000;
    }

    /** High contrast dims the backdrop further, as a multiply on the layers, not another layer on top. */
    public void setDim(float brightness) {
        LightingColorFilter filter = brightness >= 1f ? null : new LightingColorFilter(
                0xFF000000 | (Math.round(brightness * 255) * 0x010101), 0);
        back.setColorFilter(filter);
        front.setColorFilter(filter);
    }

    /** Called once, when the backdrop first covers the whole window, so the window's own background can go. */
    public void setOnCovered(Runnable onCovered) {
        this.onCovered = onCovered;
    }

    /** For tests: runs the blur in the caller's thread. */
    void setWorker(Executor executor) {
        this.executor = executor;
    }

    /** How many times the backdrop has started to change. */
    int changeCount() {
        return changes;
    }

    /** A game whose art is {@code cover}, or none when the cover is null: the plain canvas. */
    public void focus(String key, Bitmap cover, boolean fast) {
        focus(key, cover, cover == null, fast);
    }

    /**
     * The backdrop of this game, from this cover. A null cover that is not {@code noArt} is one still loading: the
     * backdrop stays as it is, and is asked again when the cover arrives. {@code noArt} with no cover shows the plain
     * canvas. Takes effect {@link #SETTLE_MS} after the last call, or {@link #FAST_SETTLE_MS} when {@code fast}.
     */
    public void focus(String key, Bitmap cover, boolean noArt, boolean fast) {
        wantedKey = key;
        wantedCover = cover;
        wantedNoArt = noArt;
        handler.removeCallbacks(apply);
        handler.postDelayed(apply, fast ? FAST_SETTLE_MS : SETTLE_MS);
    }

    /**
     * Shows at once, with no fade, the backdrop the last screen showed for {@code key}, when that is what it showed.
     * A later {@link #focus} with the same key and cover then changes nothing.
     *
     * @return false when the last backdrop was of something else, and this one stays as it is
     */
    public boolean adopt(String key) {
        String last = handoffKey;
        Bitmap bitmap = handoffBitmap;
        if (key == null || last == null || bitmap == null || !last.startsWith(key + "@")) return false;
        handler.removeCallbacks(apply);
        endTransition();
        cache.put(last, bitmap);
        shownKey = last;
        back.setImageBitmap(bitmap);
        shown = bitmap;
        covered();
        return true;
    }

    /** For tests: forgets the backdrop the last screen showed. */
    static void clearHandoff() {
        handoffKey = null;
        handoffBitmap = null;
    }

    private void applyWanted() {
        if (wantedCover == null && !wantedNoArt) {
            // Still loading: keep what is shown rather than flashing the canvas, and drop a blur that is in flight.
            request++;
            return;
        }
        String key = cacheKey(wantedKey, wantedCover);
        if (key.equals(shownKey)) return;
        Bitmap cached = cache.get(key);
        int id = ++request;
        if (cached != null) {
            display(key, cached);
            return;
        }
        Bitmap cover = wantedCover;
        executor.execute(() -> {
            Bitmap made = BackdropBlur.make(cover, canvas);
            handler.post(() -> {
                cache.put(key, made);
                // A newer request, even one still waiting out its settle time, has the say.
                if (id == request) display(key, made);
            });
        });
    }

    private static String cacheKey(String key, Bitmap cover) {
        return cover == null ? key + "@canvas" : key + "@" + System.identityHashCode(cover);
    }

    private void display(String key, Bitmap bitmap) {
        shownKey = key;
        handoffKey = key;
        handoffBitmap = bitmap;
        changes++;
        endTransition();
        if (Motion.reduced(getContext()) || getWindowToken() == null) {
            back.setImageBitmap(bitmap);
            shown = bitmap;
            covered();
            return;
        }
        boolean first = shown == null;
        front.setImageBitmap(bitmap);
        front.setAlpha(0f);
        front.setScaleX(first ? 1f : ZOOM_FROM);
        front.setScaleY(first ? 1f : ZOOM_FROM);
        front.setVisibility(VISIBLE);
        ObjectAnimator fade = ObjectAnimator.ofFloat(front, View.ALPHA, 0f, 1f);
        fade.setDuration(first ? FIRST_FADE_MS : FADE_MS);
        fade.setInterpolator(LINEAR);
        AnimatorSet set = new AnimatorSet();
        if (first) {
            set.play(fade);
        } else {
            android.view.animation.Interpolator settle = settleInterpolator();
            ObjectAnimator zoomX = ObjectAnimator.ofFloat(front, View.SCALE_X, ZOOM_FROM, 1f);
            ObjectAnimator zoomY = ObjectAnimator.ofFloat(front, View.SCALE_Y, ZOOM_FROM, 1f);
            zoomX.setDuration(ZOOM_MS);
            zoomY.setDuration(ZOOM_MS);
            zoomX.setInterpolator(settle);
            zoomY.setInterpolator(settle);
            set.playTogether(fade, zoomX, zoomY);
        }
        transition = set;
        set.addListener(new AnimatorListenerAdapter() {
            private boolean cancelled;

            @Override public void onAnimationCancel(Animator animation) {
                cancelled = true;
            }

            @Override public void onAnimationEnd(Animator animation) {
                if (cancelled) return;
                transition = null;
                commit(bitmap);
            }
        });
        set.start();
    }

    /** The incoming layer becomes the resting one, and the front layer is free for the next change. */
    private void commit(Bitmap bitmap) {
        back.setImageBitmap(bitmap);
        shown = bitmap;
        front.setVisibility(GONE);
        front.setImageDrawable(null);
        covered();
    }

    /** A change that is cut short by the next one lands where it is headed, so the next fade starts from a whole picture. */
    private void endTransition() {
        if (transition == null) return;
        AnimatorSet running = transition;
        transition = null;
        running.cancel();
        if (front.getDrawable() instanceof android.graphics.drawable.BitmapDrawable) {
            Bitmap target = ((android.graphics.drawable.BitmapDrawable) front.getDrawable()).getBitmap();
            commit(target);
        } else {
            front.setVisibility(GONE);
        }
    }

    private void covered() {
        Runnable callback = onCovered;
        onCovered = null;
        if (callback != null) callback.run();
    }

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        getViewTreeObserver().addOnGlobalLayoutListener(aligner);
    }

    @Override
    protected void onDetachedFromWindow() {
        super.onDetachedFromWindow();
        getViewTreeObserver().removeOnGlobalLayoutListener(aligner);
        handler.removeCallbacks(apply);
        request++;
        if (transition != null) {
            AnimatorSet running = transition;
            transition = null;
            running.cancel();
        }
    }

    @Override
    protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        View root = getRootView();
        if (root != this && root.getWidth() > 0 && root.getHeight() > 0) {
            widthMeasureSpec = MeasureSpec.makeMeasureSpec(root.getWidth(), MeasureSpec.EXACTLY);
            heightMeasureSpec = MeasureSpec.makeMeasureSpec(root.getHeight(), MeasureSpec.EXACTLY);
        }
        super.onMeasure(widthMeasureSpec, heightMeasureSpec);
    }

    @Override
    protected void onLayout(boolean changed, int left, int top, int right, int bottom) {
        super.onLayout(changed, left, top, right, bottom);
        align();
    }

    /**
     * Shifts to the window's corner, whatever padding the parents add for cutouts and the TV's overscan. A parent's
     * padding can change without this view being laid out again (the safe area arrives after the first pass), and the
     * root can still be unmeasured at the first measure, so this also runs after every layout pass of the window.
     */
    private void align() {
        View root = getRootView();
        if (root != this && root.getWidth() > 0 && root.getHeight() > 0
                && (getWidth() != root.getWidth() || getHeight() != root.getHeight())) {
            requestLayout();
        }
        int[] at = new int[2];
        getLocationInWindow(at);
        if (at[0] != 0) setTranslationX(getTranslationX() - at[0]);
        if (at[1] != 0) setTranslationY(getTranslationY() - at[1]);
    }

    private android.view.animation.Interpolator settleInterpolator() {
        if (settleInterpolator == null) settleInterpolator = AnimationUtils.loadInterpolator(getContext(), R.interpolator.tv_emph_dec);
        return settleInterpolator;
    }
}
