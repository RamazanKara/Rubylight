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
import android.widget.AbsListView;
import android.widget.FrameLayout;

import androidx.core.content.res.ResourcesCompat;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;

/**
 * A PC's row in a phone's grouped list. The list has one spacing, so a row draws its share of the group's shape: rounded where
 * the group starts and ends, a line where another row follows, and the gap to the next group under it. Focus is the glass ring,
 * drawn around the row's own shape: the grid's selector could only frame the row and the gap together.
 */
public class PcRow extends FrameLayout {
    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint line = new Paint();
    private final Paint rim = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint press = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Path shape = new Path();
    private final RectF rect = new RectF();
    private final float[] corners = new float[8];
    private final FocusRing ring;
    private final float groupRadius;
    private final float rowRadius;
    private final int gap;
    private final boolean highContrast;
    private boolean startsGroup = true;
    private boolean endsGroup = true;
    private boolean gapBelow;
    private float lineInset;
    private int shapedW;
    private int shapedH;

    public PcRow(Context context) {
        this(context, null);
    }

    public PcRow(Context context, AttributeSet attrs) {
        super(context, attrs);
        Resources res = context.getResources();
        float density = res.getDisplayMetrics().density;
        highContrast = AppTheme.isHighContrast(context);
        setWillNotDraw(false);
        setClipChildren(false);
        setClipToPadding(false);
        groupRadius = res.getDimension(R.dimen.nj_radius_panel);
        rowRadius = 14 * density;
        gap = res.getDimensionPixelSize(R.dimen.pc_group_gap);
        fill.setColor(NightSkyView.groupFill(context));
        line.setColor(NightSkyView.separator(context));
        line.setStrokeWidth(Math.max(1f, density * .5f));
        rim.setStyle(Paint.Style.STROKE);
        rim.setStrokeWidth(res.getDimension(R.dimen.nj_glass_chrome_solid_rim_width));
        rim.setColor(ResourcesCompat.getColor(res, R.color.nj_hc_color_rim, null));
        press.setColor(MaterialColors.getColor(context, R.attr.glassControlFill, 0x22FFFFFF));
        ring = new FocusRing(this, false);
    }

    @Override
    protected void onFinishInflate() {
        super.onFinishInflate();
        View cover = findViewById(R.id.grid_cover_frame);
        if (cover == null) return;
        float radius = 14 * getResources().getDisplayMetrics().density;
        cover.setOutlineProvider(new ViewOutlineProvider() {
            @Override public void getOutline(View view, Outline outline) {
                outline.setRoundRect(0, 0, view.getWidth(), view.getHeight(), radius);
            }
        });
        cover.setClipToOutline(true);
    }

    /**
     * Where this row stands in its group.
     *
     * @param lineInset where the line above this row starts, from the row's start edge
     */
    public void setPlace(boolean starts, boolean ends, boolean moreBelow, float lineInset) {
        boolean belowChanged = (ends && moreBelow) != gapBelow;
        startsGroup = starts;
        endsGroup = ends;
        gapBelow = ends && moreBelow;
        this.lineInset = lineInset;
        shapedW = 0;
        if (belowChanged) setPadding(getPaddingLeft(), getPaddingTop(), getPaddingRight(), gapBelow ? gap : 0);
        invalidate();
    }

    private int shapeBottom() {
        return getHeight() - (gapBelow ? gap : 0);
    }

    private void shape(int w, int h) {
        shapedW = w;
        shapedH = h;
        float top = startsGroup ? groupRadius : 0f;
        float bottom = endsGroup ? groupRadius : 0f;
        corners[0] = corners[1] = corners[2] = corners[3] = top;
        corners[4] = corners[5] = corners[6] = corners[7] = bottom;
        rect.set(0, 0, w, h);
        shape.rewind();
        shape.addRoundRect(rect, corners, Path.Direction.CW);
    }

    /** The grid keeps a selection while focus is elsewhere; the ring is only for what the keys would act on now. */
    private boolean showsRing() {
        if (!isSelected() && !isHovered()) return false;
        if (isHovered()) return true;
        return getParent() instanceof AbsListView && ((AbsListView) getParent()).hasFocus();
    }

    private float ringRadius() {
        return startsGroup && endsGroup ? groupRadius : rowRadius;
    }

    @Override
    public void setSelected(boolean selected) {
        boolean changed = selected != isSelected();
        super.setSelected(selected);
        if (changed) {
            // Its ring lies over the rows on either side.
            setTranslationZ(selected ? getResources().getDisplayMetrics().density : 0f);
            invalidate();
        }
    }

    @Override
    public void setPressed(boolean pressed) {
        boolean changed = pressed != isPressed();
        super.setPressed(pressed);
        if (changed) invalidate();
    }

    @Override
    protected void dispatchDraw(Canvas canvas) {
        int w = getWidth();
        int h = shapeBottom();
        if (w != shapedW || h != shapedH) shape(w, h);
        boolean ringed = showsRing();
        if (ringed) {
            // The row's fill is see-through: the halo stays outside its shape.
            int save = canvas.save();
            canvas.clipPath(shape, android.graphics.Region.Op.DIFFERENCE);
            ring.drawHalo(canvas, 0, 0, w, h, ringRadius());
            canvas.restoreToCount(save);
        }
        canvas.drawPath(shape, fill);
        if (highContrast) canvas.drawPath(shape, rim);
        if (!startsGroup) {
            boolean rtl = getLayoutDirection() == LAYOUT_DIRECTION_RTL;
            canvas.drawLine(rtl ? 0 : lineInset, 0, rtl ? w - lineInset : w, 0, line);
        }
        if (isPressed()) canvas.drawPath(shape, press);
        super.dispatchDraw(canvas);
        if (ringed) ring.drawRing(canvas, 0, 0, w, h, ringRadius());
    }

    @Override
    public boolean hasOverlappingRendering() {
        return false;
    }

    /** For tests: whether the ring would draw now. */
    public boolean ringShown() {
        return showsRing();
    }

    /** For tests: the height of the row's shape, without the gap under it. */
    public int shapeHeight() {
        return shapeBottom();
    }
}
