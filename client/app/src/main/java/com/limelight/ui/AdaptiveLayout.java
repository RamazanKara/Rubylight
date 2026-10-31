package com.limelight.ui;

import android.app.Activity;
import android.content.res.Configuration;
import android.graphics.Rect;
import android.os.Build;
import android.view.View;

import androidx.core.content.ContextCompat;
import androidx.core.graphics.Insets;
import androidx.core.util.Consumer;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.lifecycle.Lifecycle;
import androidx.lifecycle.LifecycleEventObserver;
import androidx.lifecycle.LifecycleOwner;
import androidx.window.core.layout.WindowSizeClass;
import androidx.window.core.layout.WindowWidthSizeClass;
import androidx.window.core.layout.WindowHeightSizeClass;
import androidx.window.java.layout.WindowInfoTrackerCallbackAdapter;
import androidx.window.layout.DisplayFeature;
import androidx.window.layout.FoldingFeature;
import androidx.window.layout.WindowInfoTracker;
import androidx.window.layout.WindowLayoutInfo;

import com.limelight.R;

public final class AdaptiveLayout {
    private final Activity activity;
    private final View root;
    private final boolean streaming;
    private final boolean handleSystemInsets;
    private final WindowInfoTrackerCallbackAdapter tracker;
    private final Consumer<WindowLayoutInfo> listener;
    private final Runnable deferredUpdate = this::update;
    private WindowInsetsCompat windowInsets = WindowInsetsCompat.CONSUMED;
    private FoldingFeature fold;
    private Consumer<WindowInsetsCompat> insetsObserver;

    private AdaptiveLayout(Activity activity, View root, boolean handleSystemInsets) {
        this.activity = activity;
        this.root = root;
        this.handleSystemInsets = handleSystemInsets;
        streaming = activity instanceof StreamWindow && root == activity.findViewById(android.R.id.content);
        tracker = new WindowInfoTrackerCallbackAdapter(
                WindowInfoTracker.getOrCreate(activity));
        listener = info -> {
            fold = null;
            for (DisplayFeature feature : info.getDisplayFeatures()) {
                if (feature instanceof FoldingFeature) {
                    FoldingFeature candidate = (FoldingFeature) feature;
                    // A flat panel is one screen for video: only a half-opened fold may move the stream
                    // to one side, so a fold reported as occluding while flat never splits or shrinks it.
                    if (streaming && candidate.getState() == FoldingFeature.State.FLAT) continue;
                    if (candidate.isSeparating() || candidate.getOcclusionType() == FoldingFeature.OcclusionType.FULL) {
                        fold = candidate;
                        break;
                    }
                }
            }
            update();
        };
        if (activity instanceof LifecycleOwner) {
            Lifecycle lifecycle = ((LifecycleOwner) activity).getLifecycle();
            LifecycleEventObserver observer = (owner, event) -> {
                if (event == Lifecycle.Event.ON_START) start();
                else if (event == Lifecycle.Event.ON_STOP) stop();
            };
            lifecycle.addObserver(observer);
            root.addOnAttachStateChangeListener(new View.OnAttachStateChangeListener() {
                @Override public void onViewAttachedToWindow(View view) { lifecycle.addObserver(observer); }
                @Override public void onViewDetachedFromWindow(View view) {
                    stop();
                    lifecycle.removeObserver(observer);
                }
            });
        } else if (!streaming) {
            root.addOnAttachStateChangeListener(new View.OnAttachStateChangeListener() {
                @Override public void onViewAttachedToWindow(View view) { start(); }
                @Override public void onViewDetachedFromWindow(View view) { stop(); }
            });
            if (ViewCompat.isAttachedToWindow(root)) start();
        }
        // Insets are dispatched before the hierarchy is measured, so padding set here is laid out in
        // the same traversal. This is the only place the safe area is applied synchronously.
        ViewCompat.setOnApplyWindowInsetsListener(root, (view, appliedInsets) -> {
            windowInsets = appliedInsets;
            update();
            if (insetsObserver != null) insetsObserver.accept(appliedInsets);
            return handleSystemInsets ? WindowInsetsCompat.CONSUMED : appliedInsets;
        });
        // Padding changed from inside a layout pass is lost: View.layout() clears the forced-layout flag
        // after it runs these listeners, so the children kept their old frames and the toolbar stayed
        // under the status bar on cold start. Hinges and picture-in-picture depend on the laid-out
        // frame, so re-evaluate on the next frame instead.
        root.addOnLayoutChangeListener((view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> {
            if (left == oldLeft && top == oldTop && right == oldRight && bottom == oldBottom) return;
            if (root.isInLayout()) root.post(deferredUpdate);
            else update();
        });
        root.addOnAttachStateChangeListener(new View.OnAttachStateChangeListener() {
            @Override public void onViewAttachedToWindow(View view) { ViewCompat.requestApplyInsets(view); }
            @Override public void onViewDetachedFromWindow(View view) { root.removeCallbacks(deferredUpdate); }
        });
    }

    public static AdaptiveLayout attach(Activity activity, View root) {
        return attach(activity, root, true);
    }

    /** Preserve hinge and TV safe areas while Material handles the sheet's system-bar insets. */
    public static AdaptiveLayout attachBottomSheet(Activity activity, View root) {
        return attach(activity, root, false);
    }

    private static AdaptiveLayout attach(Activity activity, View root, boolean handleSystemInsets) {
        if (root.getTag(R.id.adaptive_layout) == null) {
            root.setTag(R.id.adaptive_layout, new AdaptiveLayout(activity, root, handleSystemInsets));
        }
        // TV margins must be present before a dialog's first measurement and enter animation.
        ((AdaptiveLayout) root.getTag(R.id.adaptive_layout)).update();
        ViewCompat.requestApplyInsets(root);
        return (AdaptiveLayout) root.getTag(R.id.adaptive_layout);
    }

    /**
     * Hears every inset dispatch to the root, after its safe area is applied; the root consumes
     * them, so its children never do. Game follows the soft keyboard with it.
     */
    public void setInsetsObserver(Consumer<WindowInsetsCompat> observer) {
        insetsObserver = observer;
        ViewCompat.requestApplyInsets(root);
    }

    public void start() {
        tracker.addWindowLayoutInfoListener(activity, ContextCompat.getMainExecutor(activity), listener);
    }

    public void stop() {
        tracker.removeWindowLayoutInfoListener(listener);
    }

    private void update() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && activity.isInPictureInPictureMode()) {
            root.setPadding(0, 0, 0, 0);
            return;
        }
        float density = root.getResources().getDisplayMetrics().density;
        boolean television = (root.getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK)
                == Configuration.UI_MODE_TYPE_TELEVISION;
        int types;
        if (!handleSystemInsets) {
            types = 0;
        } else if (!streaming) {
            types = WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout() |
                    WindowInsetsCompat.Type.ime();
        } else if (((StreamWindow) activity).isWindowedStream()) {
            types = WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout();
        } else {
            // Transient bars overlay fullscreen video instead of resizing its aspect-ratio box.
            // Native/fill video extends beneath the cutout after either landscape rotation.
            types = ((StreamWindow) activity).usesFullDisplayForVideo() ? 0 : WindowInsetsCompat.Type.displayCutout();
        }
        Insets insets = windowInsets.getInsets(types);
        if (!handleSystemInsets) {
            // Material accounts for system bars, but a landscape camera cutout may extend farther.
            @SuppressWarnings("deprecation") int materialTop = windowInsets.getSystemWindowInsetTop();
            insets = sheetSafeArea(windowInsets.getInsets(WindowInsetsCompat.Type.displayCutout()),
                    windowInsets.getInsets(WindowInsetsCompat.Type.systemBars()), materialTop);
        }
        int margin = television && !streaming ? Math.round(24 * density) : 0;
        // A focused card scales up and draws its outline past its frame: let it use the margin, not be cut by it.
        if (margin > 0 && root instanceof android.view.ViewGroup) ((android.view.ViewGroup) root).setClipToPadding(false);
        int width = root.getWidth();
        int height = root.getHeight();
        if (fold == null || width == 0 || height == 0) {
            // The safe area alone needs no frame, so the first inset dispatch already gets it right.
            root.setPadding(insets.left + margin, insets.top + margin, insets.right + margin, insets.bottom + margin);
            return;
        }
        Rect available = new Rect(insets.left + margin, insets.top + margin,
                width - insets.right - margin, height - insets.bottom - margin);
        int[] origin = new int[2];
        int[] windowOrigin = new int[2];
        root.getLocationOnScreen(origin);
        activity.getWindow().getDecorView().getLocationOnScreen(windowOrigin);
        Rect hinge = new Rect(fold.getBounds());
        hinge.offset(windowOrigin[0] - origin[0], windowOrigin[1] - origin[1]);
        Rect pane = usablePane(available, hinge, fold.getOrientation() == FoldingFeature.Orientation.VERTICAL,
                ViewCompat.getLayoutDirection(root) == ViewCompat.LAYOUT_DIRECTION_RTL);
        root.setPadding(pane.left, pane.top, width - pane.right, height - pane.bottom);
    }

    /**
     * The part of a camera cutout a bottom sheet must still keep clear of. Material passes the system bars through,
     * and pads an expanded sheet's top by the legacy system-window inset, which already holds a top cutout. Over an
     * immersive stream the status bar is hidden, so subtracting only the bars counted the cutout twice: a phone's
     * portrait stream menu began a second cutout's height (128 px on a Pixel-size screen) below the camera.
     */
    static Insets sheetSafeArea(Insets cutout, Insets bars, int materialTop) {
        return Insets.of(Math.max(0, cutout.left - bars.left), Math.max(0, cutout.top - Math.max(bars.top, materialTop)),
                Math.max(0, cutout.right - bars.right), Math.max(0, cutout.bottom - bars.bottom));
    }

    static Rect usablePane(Rect available, Rect hinge, boolean vertical, boolean rtl) {
        Rect pane = new Rect(available);
        if (hinge == null) return pane;
        // Keep every control in one contiguous pane, including zero-width separating folds.
        if (vertical && hinge.left <= pane.right && hinge.right >= pane.left &&
                hinge.bottom > pane.top && hinge.top < pane.bottom) {
            int leftEdge = Math.max(pane.left, Math.min(pane.right, hinge.left));
            int rightEdge = Math.max(pane.left, Math.min(pane.right, hinge.right));
            int left = leftEdge - pane.left;
            int right = pane.right - rightEdge;
            if (right > left || (right == left && rtl)) pane.left = rightEdge;
            else pane.right = leftEdge;
        } else if (!vertical && hinge.top <= pane.bottom && hinge.bottom >= pane.top &&
                hinge.right > pane.left && hinge.left < pane.right) {
            int topEdge = Math.max(pane.top, Math.min(pane.bottom, hinge.top));
            int bottomEdge = Math.max(pane.top, Math.min(pane.bottom, hinge.bottom));
            if (pane.bottom - bottomEdge > topEdge - pane.top) pane.top = bottomEdge;
            else pane.bottom = topEdge;
        }
        return pane;
    }

    public static int gridColumns(float widthDp, float heightDp, boolean computers, boolean smallIcons, boolean television) {
        WindowSizeClass size = WindowSizeClass.compute(widthDp, heightDp);
        if (computers && size.getWindowWidthSizeClass() == WindowWidthSizeClass.COMPACT &&
                size.getWindowHeightSizeClass() != WindowHeightSizeClass.COMPACT) return 1;
        int cellWidth = cellWidthDp(computers, smallIcons, television);
        // A TV's posters and PC tiles have no gap of their own: each pads itself with its glow (see poster_glow).
        int spacing = television ? 0 : 8;
        return Math.max(1, (int) ((widthDp + spacing) / (cellWidth + spacing)));
    }

    /** The width a grid column is sized for, in dp; the grid stretches columns to fill the row. */
    public static int cellWidthDp(boolean computers, boolean smallIcons, boolean television) {
        // A TV PC tile is 248dp with its 16dp glow on each side (dimen tile_cell).
        if (television) return computers ? 280 : (smallIcons ? 144 : 200);
        return computers ? 280 : (smallIcons ? 112 : 168);
    }
}
