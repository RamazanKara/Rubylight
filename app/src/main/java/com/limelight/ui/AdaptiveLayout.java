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
import com.limelight.Game;

public final class AdaptiveLayout {
    private final Activity activity;
    private final View root;
    private final boolean streaming;
    private final WindowInfoTrackerCallbackAdapter tracker;
    private final Consumer<WindowLayoutInfo> listener;
    private WindowInsetsCompat windowInsets = WindowInsetsCompat.CONSUMED;
    private FoldingFeature fold;

    private AdaptiveLayout(Activity activity, View root) {
        this.activity = activity;
        this.root = root;
        streaming = activity instanceof Game && root == activity.findViewById(android.R.id.content);
        tracker = new WindowInfoTrackerCallbackAdapter(
                WindowInfoTracker.getOrCreate(activity));
        listener = info -> {
            fold = null;
            for (DisplayFeature feature : info.getDisplayFeatures()) {
                if (feature instanceof FoldingFeature) {
                    FoldingFeature candidate = (FoldingFeature) feature;
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
        ViewCompat.setOnApplyWindowInsetsListener(root, (view, appliedInsets) -> {
            windowInsets = appliedInsets;
            update();
            return WindowInsetsCompat.CONSUMED;
        });
        root.addOnLayoutChangeListener((view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> update());
    }

    public static AdaptiveLayout attach(Activity activity, View root) {
        if (root.getTag(R.id.adaptive_layout) == null) {
            root.setTag(R.id.adaptive_layout, new AdaptiveLayout(activity, root));
        }
        ViewCompat.requestApplyInsets(root);
        return (AdaptiveLayout) root.getTag(R.id.adaptive_layout);
    }

    public void start() {
        tracker.addWindowLayoutInfoListener(activity, ContextCompat.getMainExecutor(activity), listener);
    }

    public void stop() {
        tracker.removeWindowLayoutInfoListener(listener);
    }

    private void update() {
        if (root.getWidth() == 0 || root.getHeight() == 0) return;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && activity.isInPictureInPictureMode()) {
            root.setPadding(0, 0, 0, 0);
            return;
        }
        float density = root.getResources().getDisplayMetrics().density;
        boolean television = (root.getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK)
                == Configuration.UI_MODE_TYPE_TELEVISION;
        int types;
        if (!streaming) {
            types = WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout() |
                    WindowInsetsCompat.Type.ime();
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && activity.isInMultiWindowMode()) {
            types = WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout();
        } else {
            // Transient bars overlay fullscreen video instead of resizing its aspect-ratio box.
            // Native/fill video extends beneath the cutout after either landscape rotation.
            types = ((Game) activity).usesFullDisplayForVideo() ? 0 : WindowInsetsCompat.Type.displayCutout();
        }
        Insets insets = windowInsets.getInsets(types);
        int margin = television && !streaming ? Math.round(24 * density) : 0;
        Rect available = new Rect(insets.left + margin, insets.top + margin,
                root.getWidth() - insets.right - margin, root.getHeight() - insets.bottom - margin);
        Rect hinge = null;
        if (fold != null) {
            int[] origin = new int[2];
            int[] windowOrigin = new int[2];
            root.getLocationOnScreen(origin);
            activity.getWindow().getDecorView().getLocationOnScreen(windowOrigin);
            hinge = new Rect(fold.getBounds());
            hinge.offset(windowOrigin[0] - origin[0], windowOrigin[1] - origin[1]);
        }
        Rect pane = usablePane(available, hinge,
                fold != null && fold.getOrientation() == FoldingFeature.Orientation.VERTICAL,
                ViewCompat.getLayoutDirection(root) == ViewCompat.LAYOUT_DIRECTION_RTL);
        if (root.findViewById(R.id.stream_settings) != null || root.findViewById(R.id.host_profile_settings) != null ||
                root.findViewById(R.id.hostTextView) != null) {
            int gutter = Math.max(0, (pane.width() - Math.round(840 * density)) / 2);
            pane.inset(gutter, 0);
        }
        root.setPadding(pane.left, pane.top, root.getWidth() - pane.right, root.getHeight() - pane.bottom);
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
        int cellWidth = computers ? 280 : (smallIcons ? 112 : 168);
        if (television) cellWidth = computers ? 320 : 200;
        return Math.max(1, (int) ((widthDp + 8) / (cellWidth + 8)));
    }
}
