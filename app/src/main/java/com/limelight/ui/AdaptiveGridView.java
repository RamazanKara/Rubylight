package com.limelight.ui;

import android.content.Context;
import android.content.res.Configuration;
import android.util.AttributeSet;
import android.widget.GridView;

/**
 * A grid that picks its column count from the space it is measured with.
 *
 * <p>The count used to be set from a layout listener. GridView.setNumColumns() removes every child
 * and requests a layout, and a layout requested from inside a layout pass is dropped, so a rotation
 * that changed the count left the library blank until something else relaid it out (for example a
 * search query). Measuring is the one place where changing the count is followed by a layout of the
 * new children.
 */
public class AdaptiveGridView extends GridView {
    /** Below this height, cards switch to their compact artwork so a whole row stays visible. */
    private static final int SHORT_VIEWPORT_DP = 400;

    private boolean policySet;
    private boolean computers;
    private boolean smallIcons;

    public AdaptiveGridView(Context context) {
        super(context);
    }

    public AdaptiveGridView(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    public AdaptiveGridView(Context context, AttributeSet attrs, int defStyleAttr) {
        super(context, attrs, defStyleAttr);
    }

    /** Columns follow {@link AdaptiveLayout#gridColumns} for PC cards or app cards. */
    public void setColumnPolicy(boolean computers, boolean smallIcons) {
        if (policySet && this.computers == computers && this.smallIcons == smallIcons) return;
        policySet = true;
        this.computers = computers;
        this.smallIcons = smallIcons;
        requestLayout();
    }

    /** True when the grid is too short for full-height artwork, as on a phone in landscape. */
    public boolean hasShortViewport() {
        int height = getMeasuredHeight();
        return height > 0 && height / getResources().getDisplayMetrics().density < SHORT_VIEWPORT_DP;
    }

    @Override
    protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        if (policySet && MeasureSpec.getMode(widthMeasureSpec) != MeasureSpec.UNSPECIFIED) {
            float density = getResources().getDisplayMetrics().density;
            Configuration configuration = getResources().getConfiguration();
            float fontScale = Math.max(1, configuration.fontScale);
            boolean television = (configuration.uiMode & Configuration.UI_MODE_TYPE_MASK)
                    == Configuration.UI_MODE_TYPE_TELEVISION;
            int width = Math.max(0, MeasureSpec.getSize(widthMeasureSpec) - getPaddingLeft() - getPaddingRight());
            int columns = AdaptiveLayout.gridColumns(width / density / fontScale,
                    MeasureSpec.getSize(heightMeasureSpec) / density, computers, smallIcons, television);
            // A no-op when unchanged; otherwise the children are rebuilt by the layout that follows.
            setNumColumns(columns);
        }
        super.onMeasure(widthMeasureSpec, heightMeasureSpec);
    }
}
