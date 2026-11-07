package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Path;
import android.graphics.Region;
import android.util.AttributeSet;
import android.view.View;

import com.limelight.R;

/**
 * The focus glow behind a see-through card: its background only draws outside the card, so the sky stays the card's colour and
 * the glow does not fill it. The view reaches past the card by tile_glow_back on every side.
 */
public class OutsideGlow extends View {
    private final Path card = new Path();
    private final float reach;
    private final float radius;
    private int cutW;
    private int cutH;

    public OutsideGlow(Context context, AttributeSet attrs) {
        super(context, attrs);
        reach = -context.getResources().getDimension(R.dimen.tile_glow_back);
        radius = context.getResources().getDimension(R.dimen.nj_radius_panel);
    }

    @Override
    public void draw(Canvas canvas) {
        int w = getWidth();
        int h = getHeight();
        if (w != cutW || h != cutH) {
            cutW = w;
            cutH = h;
            card.rewind();
            card.addRoundRect(reach, reach, w - reach, h - reach, radius, radius, Path.Direction.CW);
        }
        int save = canvas.save();
        canvas.clipPath(card, Region.Op.DIFFERENCE);
        super.draw(canvas);
        canvas.restoreToCount(save);
    }
}
