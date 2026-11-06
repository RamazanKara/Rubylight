package com.limelight.ui;

import android.content.Context;
import android.util.AttributeSet;
import android.view.View;
import android.widget.FrameLayout;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;

/**
 * The frame of a cover's art. A cover that steps back fades this view, and a dozen covers do so on every frame of a
 * focus move: so it tells the renderer that its children need no offscreen layer to fade. Its chips sit on the art and
 * show a little more of it while dimmed, which costs nothing to the eye. The placeholder is the poster's, drawn only
 * where no opaque art covers it.
 */
public class CoverFrame extends FrameLayout {
    public CoverFrame(Context context) {
        super(context);
    }

    public CoverFrame(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    @Override
    public boolean hasOverlappingRendering() {
        return false;
    }

    @Override
    protected void onFinishInflate() {
        super.onFinishInflate();
        if (getBackground() != null) {
            // A frame shaped by its own background (a small cover in a panel) rounds its art with it.
            setClipToOutline(true);
            return;
        }
        View art = findViewById(R.id.console_cover);
        int placeholder = MaterialColors.getColor(this, R.attr.glassPlaceholder);
        if (art instanceof PosterImageView) ((PosterImageView) art).setPlaceholder(placeholder);
        else setBackgroundColor(placeholder);
    }
}
