package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.util.AttributeSet;

import androidx.appcompat.widget.AppCompatTextView;
import androidx.core.content.res.ResourcesCompat;

import com.limelight.R;

/**
 * A cover's state: a dot on a small dark disc in its foot corner, where it hides no title. Green while the game runs
 * ({@link #setActivated}), the secondary label's colour while suspended. The state's word stays this view's text, for
 * screen readers and for whoever asks; it is not drawn.
 */
public class StatusDisc extends AppCompatTextView {
    private final Paint disc = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint dot = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final int live;
    private final int idle;
    private final float dotRadius;

    public StatusDisc(Context context, AttributeSet attrs) {
        super(context, attrs);
        android.content.res.Resources res = context.getResources();
        disc.setColor(0xB80E1017);
        boolean highContrast = AppTheme.isHighContrast(context);
        live = ResourcesCompat.getColor(res, highContrast ? R.color.nj_hc_color_ok : R.color.nj_color_ok, null);
        idle = ResourcesCompat.getColor(res, R.color.nj_color_label_secondary, null);
        dotRadius = res.getDimension(R.dimen.glass_status_dot) / 2f;
    }

    @Override
    protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        int size = getResources().getDimensionPixelSize(R.dimen.glass_status_disc);
        setMeasuredDimension(size, size);
    }

    @Override
    protected void onDraw(Canvas canvas) {
        float c = getWidth() / 2f;
        canvas.drawCircle(c, c, c, disc);
        dot.setColor(isActivated() ? live : idle);
        canvas.drawCircle(c, c, dotRadius, dot);
    }
}
